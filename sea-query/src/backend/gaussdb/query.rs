use super::*;
#[cfg(feature = "postgres-array")]
use crate::ArrayType;
use crate::extension::postgres::*;

impl OperLeftAssocDecider for GaussDBQueryBuilder {
    fn well_known_left_associative(&self, op: &BinOper) -> bool {
        let common_answer = common_well_known_left_associative(op);
        let pg_specific_answer = matches!(op, BinOper::PgOperator(PgBinOper::Concatenate));
        common_answer || pg_specific_answer
    }
}

impl PrecedenceDecider for GaussDBQueryBuilder {
    fn inner_expr_well_known_greater_precedence(&self, inner: &Expr, outer_oper: &Oper) -> bool {
        let common_answer = common_inner_expr_well_known_greater_precedence(inner, outer_oper);
        let pg_specific_answer = match inner {
            Expr::Binary(_, inner_bin_oper, _) => {
                let inner_oper: Oper = (*inner_bin_oper).into();
                if inner_oper.is_arithmetic() || inner_oper.is_shift() {
                    is_ilike(inner_bin_oper)
                } else if is_pg_comparison(inner_bin_oper) {
                    outer_oper.is_logical()
                } else {
                    false
                }
            }
            _ => false,
        };
        common_answer || pg_specific_answer
    }
}

impl QueryBuilder for GaussDBQueryBuilder {
    fn placeholder(&self) -> (&'static str, bool) {
        ("$", true)
    }

    fn prepare_expr(&self, simple_expr: &Expr, sql: &mut impl SqlWriter) {
        match simple_expr {
            Expr::AsEnum(type_name, expr) => {
                sql.write_str("CAST(").unwrap();
                self.prepare_expr_common(expr, sql);
                let q = self.quote();
                let type_name = &type_name.0;
                let (ty, sfx) = if let Some(base) = type_name.strip_suffix("[]") {
                    (base, "[]")
                } else {
                    (type_name.as_ref(), "")
                };
                sql.write_str(" AS ").unwrap();
                sql.write_char(q.left()).unwrap();
                sql.write_str(ty).unwrap();
                sql.write_char(q.right()).unwrap();
                sql.write_str(sfx).unwrap();
                sql.write_char(')').unwrap();
            }
            _ => QueryBuilder::prepare_expr_common(self, simple_expr, sql),
        }
    }

    fn write_value(&self, buf: &mut impl Write, value: &Value) -> std::fmt::Result {
        match value {
            Value::Enum(v) => match v {
                OptionEnum::Some(v) => {
                    self.write_string_quoted(v.value.as_ref(), buf);
                    self.write_enum_type_hint(v.type_name.as_ref(), buf);
                    Ok(())
                }
                OptionEnum::None(_) => buf.write_str("NULL"),
            },
            #[cfg(feature = "postgres-array")]
            Value::Array(ArrayType::Enum(type_name), values) => {
                match values {
                    None => return buf.write_str("NULL"),
                    Some(values) => {
                        if values.is_empty() {
                            buf.write_str("'{}'")?;
                        } else {
                            // TODO: refactor this with JoinWrite after #1055 merged
                            // TODO: refactor this if deref_pattern is stabilize
                            macro_rules! validate_value {
                                ($name:ident) => {
                                    {
                                        let Value::Enum(OptionEnum::Some(value)) = $name else {
                                            panic!(
                                                "Value::Array(ArrayType::Enum) should contain Value::Enum"
                                            );
                                        };
                                        self.write_string_quoted(value.value.as_ref(), buf);
                                    }
                                };
                            }
                            buf.write_str("ARRAY [")?;
                            let mut iter = values.iter();
                            if let Some(element) = iter.next() {
                                validate_value!(element);
                                for element in iter {
                                    buf.write_str(",")?;
                                    validate_value!(element)
                                }
                            }
                            buf.write_str("]")?;
                        }
                    }
                }
                self.write_enum_type_hint(type_name.as_ref(), buf);
                Ok(())
            }
            _ => self.write_value_common(buf, value),
        }
    }

    fn prepare_select_distinct(&self, select_distinct: &SelectDistinct, sql: &mut impl SqlWriter) {
        match select_distinct {
            SelectDistinct::All => sql.write_str("ALL").unwrap(),
            SelectDistinct::Distinct => sql.write_str("DISTINCT").unwrap(),
            SelectDistinct::DistinctOn(cols) => {
                sql.write_str("DISTINCT ON (").unwrap();

                let mut cols = cols.iter();
                join_io!(
                    cols,
                    col,
                    join {
                        sql.write_str(", ").unwrap();
                    },
                    do {
                        self.prepare_column_ref(col, sql);
                    }
                );

                sql.write_str(")").unwrap();
            }
            _ => {}
        };
    }

    fn prepare_select_into(&self, into_table: &SelectInto, sql: &mut impl SqlWriter) {
        sql.write_str(" INTO ").unwrap();

        if let Some(modifier) = &into_table.target_table_modifier {
            sql.write_fmt(format_args!("{modifier} ")).unwrap();
        }

        sql.write_str("TABLE ").unwrap();
        self.prepare_iden(&into_table.target_table, sql);
    }

    fn prepare_bin_oper(&self, bin_oper: &BinOper, sql: &mut impl SqlWriter) {
        match bin_oper {
            BinOper::PgOperator(oper) => sql
                .write_str(match oper {
                    PgBinOper::ILike => "ILIKE",
                    PgBinOper::NotILike => "NOT ILIKE",
                    PgBinOper::Matches => "@@",
                    PgBinOper::Contains => "@>",
                    PgBinOper::Contained => "<@",
                    PgBinOper::Concatenate => "||",
                    PgBinOper::Overlap => "&&",
                    PgBinOper::Similarity => "%",
                    PgBinOper::WordSimilarity => "<%",
                    PgBinOper::StrictWordSimilarity => "<<%",
                    PgBinOper::SimilarityDistance => "<->",
                    PgBinOper::WordSimilarityDistance => "<<->",
                    PgBinOper::StrictWordSimilarityDistance => "<<<->",
                    PgBinOper::GetJsonField => "->",
                    PgBinOper::CastJsonField => "->>",
                    PgBinOper::Regex => "~",
                    PgBinOper::RegexCaseInsensitive => "~*",
                    #[cfg(feature = "postgres-vector")]
                    PgBinOper::EuclideanDistance => "<->",
                    #[cfg(feature = "postgres-vector")]
                    PgBinOper::NegativeInnerProduct => "<#>",
                    #[cfg(feature = "postgres-vector")]
                    PgBinOper::CosineDistance => "<=>",
                })
                .unwrap(),
            _ => self.prepare_bin_oper_common(bin_oper, sql),
        }
    }

    fn prepare_query_statement(&self, query: &SubQueryStatement, sql: &mut impl SqlWriter) {
        query.prepare_statement(self, sql);
    }

    fn prepare_explain_statement(&self, explain: &ExplainStatement, sql: &mut impl SqlWriter) {
        fn write_sep(sql: &mut impl SqlWriter, first: &mut bool) {
            if !*first {
                sql.write_str(", ").unwrap();
            } else {
                *first = false;
            }
        }

        // https://www.postgresql.org/docs/current/sql-explain.html
        // Specifies whether the selected option should be turned on or off. You can write TRUE, ON, or 1 to enable the option, and FALSE, OFF, or 0 to disable it. The boolean value can also be omitted, in which case TRUE is assumed.
        fn write_false(sql: &mut impl SqlWriter, value: bool) {
            if !value {
                sql.write_str(" 0").unwrap();
            }
        }

        sql.write_str("EXPLAIN").unwrap();

        let has_options = explain.analyze.is_some()
            || explain.pg_opts.verbose.is_some()
            || explain.pg_opts.costs.is_some()
            || explain.pg_opts.settings.is_some()
            || explain.pg_opts.generic_plan.is_some()
            || explain.pg_opts.buffers.is_some()
            || explain.pg_opts.serialize.is_some()
            || explain.pg_opts.wal.is_some()
            || explain.pg_opts.timing.is_some()
            || explain.pg_opts.summary.is_some()
            || explain.pg_opts.memory.is_some()
            || explain.format.is_some();

        if has_options {
            sql.write_str(" (").unwrap();
            let mut first = true;

            if let Some(analyze) = explain.analyze {
                write_sep(sql, &mut first);
                sql.write_str("ANALYZE").unwrap();
                write_false(sql, analyze);
            }

            if let Some(verbose) = explain.pg_opts.verbose {
                write_sep(sql, &mut first);
                sql.write_str("VERBOSE").unwrap();
                write_false(sql, verbose);
            }

            if let Some(costs) = explain.pg_opts.costs {
                write_sep(sql, &mut first);
                sql.write_str("COSTS").unwrap();
                write_false(sql, costs);
            }

            if let Some(settings) = explain.pg_opts.settings {
                write_sep(sql, &mut first);
                sql.write_str("SETTINGS").unwrap();
                write_false(sql, settings);
            }

            if let Some(generic_plan) = explain.pg_opts.generic_plan {
                write_sep(sql, &mut first);
                sql.write_str("GENERIC_PLAN").unwrap();
                write_false(sql, generic_plan);
            }

            if let Some(buffers) = explain.pg_opts.buffers {
                write_sep(sql, &mut first);
                sql.write_str("BUFFERS").unwrap();
                write_false(sql, buffers);
            }

            if let Some(serialize) = explain.pg_opts.serialize {
                write_sep(sql, &mut first);
                sql.write_str("SERIALIZE ").unwrap();
                sql.write_str(serialize.as_str()).unwrap();
            }

            if let Some(wal) = explain.pg_opts.wal {
                write_sep(sql, &mut first);
                sql.write_str("WAL").unwrap();
                write_false(sql, wal);
            }

            if let Some(timing) = explain.pg_opts.timing {
                write_sep(sql, &mut first);
                sql.write_str("TIMING").unwrap();
                write_false(sql, timing);
            }

            if let Some(summary) = explain.pg_opts.summary {
                write_sep(sql, &mut first);
                sql.write_str("SUMMARY").unwrap();
                write_false(sql, summary);
            }

            if let Some(memory) = explain.pg_opts.memory {
                write_sep(sql, &mut first);
                sql.write_str("MEMORY").unwrap();
                write_false(sql, memory);
            }

            if let Some(format) = explain.format {
                write_sep(sql, &mut first);
                sql.write_str("FORMAT ").unwrap();
                sql.write_str(format.as_str()).unwrap();
            }
            sql.write_str(")").unwrap();
        }

        if let Some(statement) = &explain.statement {
            sql.write_str(" ").unwrap();
            statement.write_to(self, sql);
        }
    }

    fn prepare_function_name(&self, function: &Func, sql: &mut impl SqlWriter) {
        match function {
            Func::PgFunction(function) => sql
                .write_str(match function {
                    PgFunc::ToTsquery => "TO_TSQUERY",
                    PgFunc::ToTsvector => "TO_TSVECTOR",
                    PgFunc::PhrasetoTsquery => "PHRASETO_TSQUERY",
                    PgFunc::PlaintoTsquery => "PLAINTO_TSQUERY",
                    PgFunc::WebsearchToTsquery => "WEBSEARCH_TO_TSQUERY",
                    PgFunc::TsRank => "TS_RANK",
                    PgFunc::TsRankCd => "TS_RANK_CD",
                    PgFunc::StartsWith => "STARTS_WITH",
                    PgFunc::GenRandomUUID => "GEN_RANDOM_UUID",
                    PgFunc::JsonBuildObject => "JSON_BUILD_OBJECT",
                    PgFunc::JsonAgg => "JSON_AGG",
                    PgFunc::ArrayAgg => "ARRAY_AGG",
                    PgFunc::DateTrunc => "DATE_TRUNC",
                    PgFunc::Any => "ANY",
                    PgFunc::Some => "SOME",
                    PgFunc::All => "ALL",
                    PgFunc::AdvisoryLock => "PG_ADVISORY_LOCK",
                    PgFunc::AdvisoryLockShared => "PG_ADVISORY_LOCK_SHARED",
                    PgFunc::TryAdvisoryLock => "PG_TRY_ADVISORY_LOCK",
                    PgFunc::TryAdvisoryLockShared => "PG_TRY_ADVISORY_LOCK_SHARED",
                    PgFunc::AdvisoryUnlock => "PG_ADVISORY_UNLOCK",
                    PgFunc::AdvisoryUnlockShared => "PG_ADVISORY_UNLOCK_SHARED",
                    PgFunc::AdvisoryUnlockAll => "PG_ADVISORY_UNLOCK_ALL",
                    PgFunc::AdvisoryXactLock => "PG_ADVISORY_XACT_LOCK",
                    PgFunc::AdvisoryXactLockShared => "PG_ADVISORY_XACT_LOCK_SHARED",
                    PgFunc::TryAdvisoryXactLock => "PG_TRY_ADVISORY_XACT_LOCK",
                    PgFunc::TryAdvisoryXactLockShared => "PG_TRY_ADVISORY_XACT_LOCK_SHARED",
                })
                .unwrap(),
            _ => self.prepare_function_name_common(function, sql),
        }
    }

    fn prepare_table_sample(&self, select: &SelectStatement, sql: &mut impl SqlWriter) {
        let Some(table_sample) = select.table_sample else {
            return;
        };

        match table_sample.method {
            SampleMethod::BERNOULLI => sql.write_str(" TABLESAMPLE BERNOULLI").unwrap(),
            SampleMethod::SYSTEM => sql.write_str(" TABLESAMPLE SYSTEM").unwrap(),
        }
        sql.write_str(" (").unwrap();
        write!(sql, "{}", table_sample.percentage).unwrap();
        sql.write_char(')').unwrap();
        if let Some(repeatable) = table_sample.repeatable {
            sql.write_str(" REPEATABLE (").unwrap();
            write!(sql, "{repeatable}").unwrap();
            sql.write_char(')').unwrap();
        }
    }

    fn prepare_order_expr(&self, order_expr: &OrderExpr, sql: &mut impl SqlWriter) {
        if !matches!(order_expr.order, Order::Field(_)) {
            self.prepare_expr(&order_expr.expr, sql);
        }
        self.prepare_order(order_expr, sql);
        match order_expr.nulls {
            None => (),
            Some(NullOrdering::Last) => sql.write_str(" NULLS LAST").unwrap(),
            Some(NullOrdering::First) => sql.write_str(" NULLS FIRST").unwrap(),
        }
    }

    fn prepare_value(&self, value: Value, sql: &mut impl SqlWriter) {
        match value {
            Value::Enum(value) => match value {
                OptionEnum::Some(value) => {
                    let Enum { type_name, value } = *value;
                    sql.push_param(Value::String(Some(value.into_owned())), self as _);
                    self.write_enum_type_hint(type_name, sql);
                }
                OptionEnum::None(type_name) => {
                    sql.push_param(Value::String(None), self as _);
                    self.write_enum_type_hint(type_name, sql);
                }
            },
            #[cfg(feature = "postgres-array")]
            Value::Array(ArrayType::Enum(type_name), values) => {
                let values = values.map(|values| {
                    Box::new(
                        values
                            .into_iter()
                            .map(|value| match value {
                                Value::Enum(OptionEnum::Some(value)) => {
                                    Value::String(Some(value.value.into_owned()))
                                }
                                _ => {
                                    panic!(
                                        "Value::Array(ArrayType::Enum) should contain Value::Enum"
                                    );
                                }
                            })
                            .collect(),
                    )
                });
                sql.push_param(Value::Array(ArrayType::String, values), self as _);
                self.write_enum_type_hint(type_name.as_ref(), sql);
                sql.write_str("[]").unwrap();
            }
            _ => sql.push_param(value, self as _),
        }
    }

    fn write_string_quoted(&self, string: &str, buffer: &mut impl Write) {
        if self.needs_escape(string) {
            buffer.write_str("E'").unwrap();
        } else {
            buffer.write_str("'").unwrap();
        }
        self.write_escaped(buffer, string);
        buffer.write_str("'").unwrap();
    }

    fn write_bytes(&self, bytes: &[u8], buffer: &mut impl Write) {
        buffer.write_str("'\\x").unwrap();
        for b in bytes {
            write!(buffer, "{b:02X}").unwrap();
        }
        buffer.write_str("'").unwrap();
    }

    fn if_null_function(&self) -> &str {
        "COALESCE"
    }

    // GaussDB A-mode (Oracle/MySQL compatible) does not support the Postgres
    // `ON CONFLICT [target] [WHERE ...] DO NOTHING / DO UPDATE SET ...` syntax
    // (a real 507 A-mode instance rejects it with error 42601). The pinned
    // A-mode upsert syntax is:
    //
    //     INSERT ... ON DUPLICATE KEY UPDATE col = excluded.col, ...
    //
    // The conflicting row is decided by the primary key / unique index, i.e.
    // this clause never carries a conflict target. The overrides below take
    // over the whole ON CONFLICT rendering accordingly.

    /// Translate [`InsertStatement`] into SQL, rendered in A-mode syntax.
    ///
    /// A DO NOTHING on-conflict is intercepted here instead of being handled
    /// by `prepare_on_conflict`: emulating DO NOTHING requires the INSERT
    /// column list (see `render_do_nothing_as_noop_update`), which is not
    /// reachable from the `prepare_on_conflict` signature. The statement is
    /// cloned with `on_conflict`/`returning` lifted out so the common body
    /// renderer stays untouched, then the no-op update clause (and a
    /// returning clause, if any) are appended in the same order the common
    /// renderer would emit them.
    fn prepare_insert_statement(&self, insert: &InsertStatement, sql: &mut impl SqlWriter) {
        if matches!(
            insert
                .on_conflict
                .as_ref()
                .and_then(|oc| oc.action.as_ref()),
            Some(OnConflictAction::DoNothing(_))
        ) {
            let mut body = insert.clone();
            let on_conflict = body.on_conflict.take().unwrap();
            let returning = body.returning.take();
            self.prepare_insert_statement_common(&body, sql);
            self.render_do_nothing_as_noop_update(&on_conflict, insert, sql);
            if returning.is_some() {
                self.prepare_returning(&returning, sql);
            }
            return;
        }
        self.prepare_insert_statement_common(insert, sql);
    }

    /// Write the ON CONFLICT expression, rendered as A-mode
    /// `ON DUPLICATE KEY UPDATE`.
    ///
    /// Unlike the default implementation, the conflict target and the target
    /// WHERE clause (Postgres index predicate) have no A-mode counterpart and
    /// are silently dropped instead of rendered, to avoid generating invalid
    /// SQL. The action WHERE clause (`DO UPDATE ... WHERE`) is likewise
    /// unsupported by A-mode and dropped (semantic relaxation: conditional
    /// updates become unconditional).
    ///
    /// A DO NOTHING action never reaches this point through
    /// [`QueryBuilder::prepare_insert_statement`]: it is intercepted there
    /// and rendered by `render_do_nothing_as_noop_update`. If one arrives
    /// here anyway the pipeline invariant is broken, so panic instead of
    /// silently rendering SQL the kernel rejects.
    fn prepare_on_conflict(&self, on_conflict: &Option<OnConflict>, sql: &mut impl SqlWriter) {
        let Some(on_conflict) = on_conflict else {
            return;
        };
        if let Some(OnConflictAction::DoNothing(_)) = &on_conflict.action {
            panic!(
                "GaussDB A-mode ON CONFLICT DO NOTHING must be intercepted by \
                 prepare_insert_statement and rendered via \
                 render_do_nothing_as_noop_update"
            );
        }
        // DO UPDATE: `ON DUPLICATE KEY UPDATE col = excluded.col, ...`
        self.prepare_on_conflict_keywords(sql);
        self.prepare_on_conflict_action(&on_conflict.action, sql);
    }

    #[doc(hidden)]
    /// Write ON CONFLICT keywords in A-mode syntax
    fn prepare_on_conflict_keywords(&self, sql: &mut impl SqlWriter) {
        sql.write_str(" ON DUPLICATE KEY").unwrap();
    }

    #[doc(hidden)]
    /// Write ON CONFLICT do-update keywords in A-mode syntax
    fn prepare_on_conflict_do_update_keywords(&self, sql: &mut impl SqlWriter) {
        sql.write_str(" UPDATE ").unwrap();
    }

    #[doc(hidden)]
    /// Write the reference to the candidate (excluded) row of an update
    /// column. The pinned A-mode syntax writes `excluded` unquoted:
    /// `col = excluded.col`.
    fn prepare_on_conflict_excluded_table(&self, col: &DynIden, sql: &mut impl SqlWriter) {
        sql.write_str("excluded.").unwrap();
        self.prepare_iden(col, sql);
    }
}

impl GaussDBQueryBuilder {
    fn write_enum_type_hint(&self, type_name: impl AsRef<str>, sql: &mut impl Write) {
        sql.write_str("::").unwrap();
        let q = self.quote();
        sql.write_char(q.left()).unwrap();
        sql.write_str(type_name.as_ref()).unwrap();
        sql.write_char(q.right()).unwrap();
    }

    /// Render the A-mode emulation of an ON CONFLICT DO NOTHING action as a
    /// no-op update over the **non-key** inserted columns, e.g.
    ///     ON DUPLICATE KEY UPDATE "v" = "v"
    ///
    /// A real 507 A-mode instance rejects every ON DUPLICATE KEY UPDATE
    /// assignment to a key column — including self-assignment
    /// (`"id" = "id"` fails with "INSERT ON DUPLICATE KEY UPDATE cannot be
    /// used to update primary key or unique key") — while a non-key column
    /// self-assignment succeeds and leaves the value unchanged. Hence the
    /// self-assignment set is the INSERT column list minus the key columns
    /// known at the rendering point: the primary keys carried by
    /// `OnConflictAction::DoNothing` (filled by `do_nothing_on`, which
    /// SeaORM fills from the entity) plus the conflict target columns
    /// (also key columns; the kernel error covers primary key and unique
    /// key alike). When neither source provides key columns (bare
    /// `OnConflict::new().do_nothing()`), all inserted columns are
    /// self-assigned and keeping key columns out of the statement is the
    /// caller's responsibility. If no non-key inserted column remains
    /// (e.g. a single-column primary key table), DO NOTHING cannot be
    /// emulated at all and this panics instead of rendering SQL the kernel
    /// rejects.
    fn render_do_nothing_as_noop_update(
        &self,
        on_conflict: &OnConflict,
        insert: &InsertStatement,
        sql: &mut impl SqlWriter,
    ) {
        let mut key_cols: Vec<&DynIden> = Vec::new();
        if let Some(OnConflictAction::DoNothing(pk_cols)) = &on_conflict.action {
            key_cols.extend(pk_cols.iter());
        }
        if let OnConflictTarget::Identifiers(identifiers) = &on_conflict.targets {
            key_cols.extend(identifiers.iter().filter_map(|i| match i {
                OnConflictIdentifier::Column(col) => Some(col),
                OnConflictIdentifier::Expr(_) => None,
            }));
        }
        let noop_cols: Vec<&DynIden> = insert
            .columns
            .iter()
            .filter(|col| !key_cols.contains(col))
            .collect();
        if noop_cols.is_empty() {
            panic!(
                "GaussDB A-mode cannot emulate ON CONFLICT DO NOTHING for this insert: \
                 no non-key inserted column is available (the kernel rejects any \
                 ON DUPLICATE KEY UPDATE assignment to a primary key / unique key \
                 column, including self-assignment). Use OnConflict::do_update(..) \
                 instead, or adjust the table schema so the insert carries at \
                 least one non-key column."
            );
        }
        self.prepare_on_conflict_keywords(sql);
        self.prepare_on_conflict_do_update_keywords(sql);
        let mut cols_iter = noop_cols.into_iter();
        join_io!(
            cols_iter,
            col,
            join {
                sql.write_str(", ").unwrap();
            },
            do {
                self.prepare_iden(col, sql);
                sql.write_str(" = ").unwrap();
                self.prepare_iden(col, sql);
            }
        );
    }
}

fn is_pg_comparison(b: &BinOper) -> bool {
    matches!(
        b,
        BinOper::PgOperator(PgBinOper::Contained)
            | BinOper::PgOperator(PgBinOper::Contains)
            | BinOper::PgOperator(PgBinOper::Similarity)
            | BinOper::PgOperator(PgBinOper::WordSimilarity)
            | BinOper::PgOperator(PgBinOper::StrictWordSimilarity)
            | BinOper::PgOperator(PgBinOper::Matches)
    )
}

fn is_ilike(b: &BinOper) -> bool {
    matches!(
        b,
        BinOper::PgOperator(PgBinOper::ILike) | BinOper::PgOperator(PgBinOper::NotILike)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gaussdb_insert_on_duplicate_key_update() {
        // DO UPDATE with a conflict target: the target ("id") has no A-mode
        // counterpart and must be dropped from the rendered SQL.
        assert_eq!(
            Query::insert()
                .into_table(Alias::new("t"))
                .columns([Alias::new("id"), Alias::new("v")])
                .values_panic([1i32.into(), "a".into()])
                .on_conflict(
                    OnConflict::column(Alias::new("id"))
                        .update_column(Alias::new("v"))
                        .to_owned(),
                )
                .to_string(GaussDBQueryBuilder),
            [
                r#"INSERT INTO "t" ("id", "v") VALUES (1, 'a')"#,
                // A-mode upsert: no conflict target, `excluded` unquoted
                r#"ON DUPLICATE KEY UPDATE "v" = excluded."v""#,
            ]
            .join(" ")
        );
    }

    #[test]
    fn test_gaussdb_insert_on_duplicate_key_update_do_nothing() {
        // DO NOTHING with primary keys from `do_nothing_on`: the kernel
        // rejects key-column self-assignment (`"id" = "id"` fails on a real
        // 507 A-mode instance), so the no-op emulation self-assigns the
        // non-key inserted columns instead.
        assert_eq!(
            Query::insert()
                .into_table(Alias::new("t"))
                .columns([Alias::new("id"), Alias::new("v")])
                .values_panic([1i32.into(), "a".into()])
                .on_conflict(
                    OnConflict::columns([Alias::new("id")])
                        .do_nothing_on([Alias::new("id")])
                        .to_owned(),
                )
                .to_string(GaussDBQueryBuilder),
            [
                r#"INSERT INTO "t" ("id", "v") VALUES (1, 'a')"#,
                r#"ON DUPLICATE KEY UPDATE "v" = "v""#,
            ]
            .join(" ")
        );
    }

    #[test]
    fn test_gaussdb_insert_on_duplicate_key_update_do_nothing_target_keys() {
        // Bare `do_nothing()` without `do_nothing_on`: the conflict target
        // columns still carry key info and are excluded from the
        // self-assignment.
        assert_eq!(
            Query::insert()
                .into_table(Alias::new("t"))
                .columns([Alias::new("id"), Alias::new("v")])
                .values_panic([1i32.into(), "a".into()])
                .on_conflict(
                    OnConflict::columns([Alias::new("id")])
                        .do_nothing()
                        .to_owned()
                )
                .to_string(GaussDBQueryBuilder),
            [
                r#"INSERT INTO "t" ("id", "v") VALUES (1, 'a')"#,
                r#"ON DUPLICATE KEY UPDATE "v" = "v""#,
            ]
            .join(" ")
        );
    }

    #[test]
    fn test_gaussdb_insert_on_duplicate_key_update_do_nothing_fallback() {
        // Bare `OnConflict::new().do_nothing()`: no key columns are known at
        // the rendering point, so all inserted columns are self-assigned and
        // keeping key columns out is the caller's responsibility.
        assert_eq!(
            Query::insert()
                .into_table(Alias::new("t"))
                .columns([Alias::new("id"), Alias::new("v")])
                .values_panic([1i32.into(), "a".into()])
                .on_conflict(OnConflict::new().do_nothing().to_owned())
                .to_string(GaussDBQueryBuilder),
            [
                r#"INSERT INTO "t" ("id", "v") VALUES (1, 'a')"#,
                r#"ON DUPLICATE KEY UPDATE "id" = "id", "v" = "v""#,
            ]
            .join(" ")
        );
    }

    #[test]
    #[should_panic(expected = "GaussDB A-mode cannot emulate ON CONFLICT DO NOTHING")]
    fn test_gaussdb_insert_on_duplicate_key_update_do_nothing_single_primary_key_panics() {
        // Single-column primary key table: after excluding the key column no
        // non-key inserted column remains, so A-mode cannot emulate DO
        // NOTHING and must fail loudly instead of rendering SQL the kernel
        // rejects.
        Query::insert()
            .into_table(Alias::new("t"))
            .columns([Alias::new("id")])
            .values_panic([1i32.into()])
            .on_conflict(
                OnConflict::columns([Alias::new("id")])
                    .do_nothing_on([Alias::new("id")])
                    .to_owned(),
            )
            .to_string(GaussDBQueryBuilder);
    }

    #[test]
    fn test_gaussdb_insert_on_duplicate_key_update_where_dropped() {
        // WHERE clauses (Postgres index predicate / DO UPDATE ... WHERE) have
        // no A-mode counterpart: they are dropped instead of rendered.
        assert_eq!(
            Query::insert()
                .into_table(Alias::new("t"))
                .columns([Alias::new("id"), Alias::new("v")])
                .values_panic([1i32.into(), "a".into()])
                .on_conflict(
                    OnConflict::column(Alias::new("id"))
                        .update_column(Alias::new("v"))
                        .target_and_where(Expr::col(Alias::new("v")).is_null())
                        .action_and_where(Expr::col(Alias::new("id")).is_null())
                        .to_owned(),
                )
                .to_string(GaussDBQueryBuilder),
            [
                r#"INSERT INTO "t" ("id", "v") VALUES (1, 'a')"#,
                r#"ON DUPLICATE KEY UPDATE "v" = excluded."v""#,
            ]
            .join(" ")
        );
    }
}
