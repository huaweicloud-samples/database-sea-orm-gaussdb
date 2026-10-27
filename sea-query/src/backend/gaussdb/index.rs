use super::*;

impl IndexBuilder for GaussDBQueryBuilder {
    // Overridden due to different "NULLS NOT UNIQUE" position in table index expression
    // (as opposed to the regular index expression)
    fn prepare_table_index_expression(
        &self,
        create: &IndexCreateStatement,
        sql: &mut impl SqlWriter,
    ) {
        if let Some(name) = &create.index.name {
            sql.write_str("CONSTRAINT ").unwrap();
            sql.write_char(self.quote().left()).unwrap();
            sql.write_str(name).unwrap();
            sql.write_char(self.quote().right()).unwrap();
            sql.write_str(" ").unwrap();
        }

        self.prepare_index_prefix(create, sql);

        if create.nulls_not_distinct {
            sql.write_str("NULLS NOT DISTINCT ").unwrap();
        }

        self.prepare_index_columns(&create.index.columns, sql);

        if !create.include_columns.is_empty() {
            sql.write_str(" ").unwrap();
            self.prepare_include_columns(&create.include_columns, sql);
        }
    }

    fn prepare_index_create_statement(
        &self,
        create: &IndexCreateStatement,
        sql: &mut impl SqlWriter,
    ) {
        sql.write_str("CREATE ").unwrap();
        self.prepare_index_prefix(create, sql);
        sql.write_str("INDEX ").unwrap();

        if create.concurrently {
            sql.write_str("CONCURRENTLY ").unwrap();
        }

        if create.if_not_exists {
            sql.write_str("IF NOT EXISTS ").unwrap();
        }

        if let Some(name) = &create.index.name {
            sql.write_char(self.quote().left()).unwrap();
            sql.write_str(name).unwrap();
            sql.write_char(self.quote().right()).unwrap();
        }

        sql.write_str(" ON ").unwrap();
        if let Some(table) = &create.table {
            self.prepare_table_ref_index_stmt(table, sql);
        }

        self.prepare_index_type(&create.index_type, sql);
        sql.write_str(" ").unwrap();
        self.prepare_index_columns(&create.index.columns, sql);
        self.prepare_index_with(&create.index_with, sql);

        if !create.include_columns.is_empty() {
            sql.write_str(" ").unwrap();
            self.prepare_include_columns(&create.include_columns, sql);
        }

        if create.nulls_not_distinct {
            sql.write_str(" NULLS NOT DISTINCT").unwrap();
        }
        self.prepare_filter(&create.r#where, sql);
    }

    fn prepare_table_ref_index_stmt(&self, table_ref: &TableRef, sql: &mut impl SqlWriter) {
        // Support only `table` and `schema.table` forms.
        // No `database.schema.table` or aliases.
        let TableRef::Table(table_name, None) = table_ref else {
            panic!("Not supported");
        };
        match table_name.as_iden_tuple() {
            (Some(_db), _schema, _table) => panic!("Not supported"),
            (None, _schema, _table) => self.prepare_table_ref_iden(table_ref, sql),
        }
    }

    fn prepare_index_drop_statement(&self, drop: &IndexDropStatement, sql: &mut impl SqlWriter) {
        sql.write_str("DROP INDEX ").unwrap();

        if drop.concurrently {
            sql.write_str("CONCURRENTLY ").unwrap();
        }

        if drop.if_exists {
            sql.write_str("IF EXISTS ").unwrap();
        }

        if let Some(table) = &drop.table {
            // Support only `table` and `schema.table` forms.
            // No `database.schema.table` or aliases.
            let TableRef::Table(table_name, None) = table else {
                panic!("Not supported");
            };
            match table_name.as_iden_tuple() {
                (None, None, _table) => {}
                (None, Some(schema), _table) => {
                    self.prepare_iden(schema, sql);
                    sql.write_str(".").unwrap();
                }
                (Some(_db), _schema, _table) => panic!("Not supported"),
            }
        }
        if let Some(name) = &drop.index.name {
            sql.write_char(self.quote().left()).unwrap();
            sql.write_str(name).unwrap();
            sql.write_char(self.quote().right()).unwrap();
        }
    }

    fn prepare_index_type(&self, col_index_type: &Option<IndexType>, sql: &mut impl SqlWriter) {
        if let Some(index_type) = col_index_type {
            sql.write_str(" USING ").unwrap();
            match index_type {
                IndexType::BTree => sql.write_str("BTREE"),
                IndexType::FullText => sql.write_str("GIN"),
                IndexType::Hash => sql.write_str("HASH"),
                IndexType::Custom(custom) => sql.write_str(&custom.0),
            }
            .unwrap()
        }
    }

    fn prepare_index_prefix(&self, create: &IndexCreateStatement, sql: &mut impl SqlWriter) {
        if create.primary {
            sql.write_str("PRIMARY KEY ").unwrap();
        }
        if create.unique {
            sql.write_str("UNIQUE ").unwrap();
        }
    }

    fn prepare_index_columns(&self, columns: &[IndexColumn], sql: &mut impl SqlWriter) {
        sql.write_str("(").unwrap();

        let mut cols = columns.iter();
        join_io!(
            cols,
            col,
            join {
                sql.write_str(", ").unwrap();
            },
            do {
                match col {
                    IndexColumn::TableColumn(column) => {
                        self.prepare_index_column_with_table_column(column, sql);
                    }
                    IndexColumn::Expr(column) => {
                        sql.write_str("(").unwrap();
                        self.prepare_expr(&column.expr, sql);
                        sql.write_str(")").unwrap();
                        if let Some(order) = &column.order {
                            match order {
                                IndexOrder::Asc => sql.write_str(" ASC").unwrap(),
                                IndexOrder::Desc => sql.write_str(" DESC").unwrap(),
                            }
                        }
                    }
                }

                if let Some(operator_class) = col.operator_class() {
                    sql.write_str(" ").unwrap();
                    sql.write_str(&operator_class.0).unwrap();
                }
            }
        );

        sql.write_str(")").unwrap();
    }

    fn prepare_filter(&self, condition: &ConditionHolder, sql: &mut impl SqlWriter) {
        self.prepare_condition(condition, "WHERE", sql);
    }
}

impl GaussDBQueryBuilder {
    fn prepare_include_columns(&self, columns: &[DynIden], sql: &mut impl SqlWriter) {
        sql.write_str("INCLUDE (").unwrap();

        let mut cols = columns.iter();
        join_io!(
            cols,
            col,
            join {
                sql.write_str(", ").unwrap();
            },
            do {
                self.prepare_iden(col, sql);
            }
        );

        sql.write_str(")").unwrap();
    }

    /// Write the `WITH (key = value)` index storage parameters
    /// ([`GaussDBIndexExt::gaussdb_index_with`](crate::extension::gaussdb::GaussDBIndexExt)),
    /// e.g. `WITH (IVF_NLIST = 16)` for `USING GSIVFFLAT` vector indexes.
    fn prepare_index_with(&self, options: &[(String, String)], sql: &mut impl SqlWriter) {
        if options.is_empty() {
            return;
        }

        sql.write_str(" WITH (").unwrap();

        let mut options = options.iter();
        if let Some((key, value)) = options.next() {
            sql.write_str(key).unwrap();
            sql.write_str(" = ").unwrap();
            sql.write_str(value).unwrap();
        }
        for (key, value) in options {
            sql.write_str(", ").unwrap();
            sql.write_str(key).unwrap();
            sql.write_str(" = ").unwrap();
            sql.write_str(value).unwrap();
        }

        sql.write_str(")").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use crate::{extension::gaussdb::GaussDBIndexExt, tests_cfg::*, *};

    // A 模式向量索引定档语法：
    // `CREATE INDEX <名> ON <表> USING GSIVFFLAT (<列> <操作符>) WITH (IVF_NLIST = <n>)`
    #[test]
    fn test_gaussdb_index_create_gsivfflat_l2_with() {
        assert_eq!(
            Index::create()
                .name("seaorm_vec_idx_t_r_idx")
                .table("seaorm_vec_idx_t")
                .col(IndexColumn::from(Alias::new("repr")).with_operator_class("L2"))
                .gaussdb_index_method("GSIVFFLAT")
                .gaussdb_index_with("IVF_NLIST", 16)
                .to_owned()
                .to_string(GaussDBQueryBuilder),
            r#"CREATE INDEX "seaorm_vec_idx_t_r_idx" ON "seaorm_vec_idx_t" USING GSIVFFLAT ("repr" L2) WITH (IVF_NLIST = 16)"#
        );
    }

    // 多个 WITH 参数累积渲染
    #[test]
    fn test_gaussdb_index_create_gsivfflat_multiple_with() {
        assert_eq!(
            Index::create()
                .name("idx-emb")
                .table("item")
                .col(IndexColumn::from(Alias::new("embedding")).with_operator_class("IP"))
                .gaussdb_index_method("GSIVFFLAT")
                .gaussdb_index_with("IVF_NLIST", 256)
                .gaussdb_index_with("IVF_NPROBE", 8)
                .to_owned()
                .to_string(GaussDBQueryBuilder),
            r#"CREATE INDEX "idx-emb" ON "item" USING GSIVFFLAT ("embedding" IP) WITH (IVF_NLIST = 256, IVF_NPROBE = 8)"#
        );
    }

    // GSDISKANN 无操作符形态（不加 operator class）
    #[test]
    fn test_gaussdb_index_create_gsdiskann_no_operator() {
        assert_eq!(
            Index::create()
                .name("idx-emb")
                .table("item")
                .col(Alias::new("embedding"))
                .gaussdb_index_method("GSDISKANN")
                .to_owned()
                .to_string(GaussDBQueryBuilder),
            r#"CREATE INDEX "idx-emb" ON "item" USING GSDISKANN ("embedding")"#
        );
    }

    // 普通 btree 索引不受影响（无 USING / 无 WITH）
    #[test]
    fn test_gaussdb_index_create_btree_unchanged() {
        assert_eq!(
            Index::create()
                .name("idx-aspect")
                .table(Glyph::Table)
                .col(Glyph::Aspect)
                .to_owned()
                .to_string(GaussDBQueryBuilder),
            r#"CREATE INDEX "idx-aspect" ON "glyph" ("aspect")"#
        );

        // 显式 BTREE 也保持原渲染
        assert_eq!(
            Index::create()
                .name("idx-aspect")
                .table(Glyph::Table)
                .col(Glyph::Aspect)
                .index_type(IndexType::BTree)
                .to_owned()
                .to_string(GaussDBQueryBuilder),
            r#"CREATE INDEX "idx-aspect" ON "glyph" USING BTREE ("aspect")"#
        );
    }

    // 仅 gaussdb backend 消费：其他 backend 忽略 WITH 参数（USING 部分为
    // IndexType::Custom 的通用逃逸口，本来就渲染）
    #[test]
    fn test_gaussdb_index_with_ignored_by_other_backends() {
        assert_eq!(
            Index::create()
                .name("idx-emb")
                .table("item")
                .col(IndexColumn::from(Alias::new("embedding")).with_operator_class("L2"))
                .gaussdb_index_method("GSIVFFLAT")
                .gaussdb_index_with("IVF_NLIST", 16)
                .to_owned()
                .to_string(PostgresQueryBuilder),
            r#"CREATE INDEX "idx-emb" ON "item" USING GSIVFFLAT ("embedding" L2)"#
        );
    }
}
