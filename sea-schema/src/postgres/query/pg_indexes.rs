use super::SchemaQueryBuilder;
use crate::sqlx_types::SqlxRow;
use sea_query::{
    Condition, DynIden, Expr, ExprTrait, Iden, JoinType, Order, Query, SelectStatement,
};

#[derive(Debug, Iden)]
pub enum PgIndexes {
    Table,
    #[iden = "tablename"]
    TableName,
    #[iden = "schemaname"]
    SchemaName,
    #[iden = "indexname"]
    IndexName,
}

#[derive(Debug, Iden)]
pub enum PgIndex {
    Table,
    #[iden = "indexrelid"]
    IndexRelId,
    #[iden = "indrelid"]
    IndRelId,
    #[iden = "indisunique"]
    IndIsUnique,
    #[iden = "indisprimary"]
    IndIsPrimary,
    #[iden = "indpred"]
    IndPred,
    #[iden = "indkey"]
    IndKey,
}

#[derive(Debug, Iden)]
pub enum PgClass {
    Table,
    Oid,
    #[iden = "relnamespace"]
    RelNamespace,
    #[iden = "relname"]
    RelName,
}

#[derive(Debug, Iden)]
pub enum PgNamespace {
    Table,
    Oid,
    #[iden = "nspname"]
    NspName,
}

#[derive(Debug, Iden)]
pub enum PgAttribute {
    Table,
    Oid,
    #[iden = "attrelid"]
    AttRelId,
    #[iden = "attname"]
    AttName,
    #[iden = "attnum"]
    AttNum,
}

#[derive(Debug, Default)]
pub struct UniqueIndexQueryResult {
    pub index_name: String,
    pub table_schema: String,
    pub table_name: String,
    pub column_name: String,
    pub is_partial: bool,
}

/// Upper bound for the `indkey` subscript expansion, equal to PostgreSQL's
/// `INDEX_MAX_KEYS` (a stored `indkey` never has more entries than this).
/// Positions beyond a row's actual `indkey` length subscript to NULL and are
/// discarded by the join condition, so the bound never changes the result.
/// Keeping the expansion uncorrelated avoids `LATERAL`, which GaussDB (A
/// compatibility mode) rejects with 42601.
const PG_INDEX_MAX_KEYS: u32 = 32;

impl SchemaQueryBuilder {
    pub fn query_table_unique_indexes(&self, schema: DynIden, table: DynIden) -> SelectStatement {
        let idx = "idx";
        let insp = "insp";
        let tbl = "tbl";
        let tnsp = "tnsp";
        let col = "col";
        let partially = "partially";

        Query::select()
            .column((idx, PgClass::RelName))
            .column((insp, PgNamespace::NspName))
            .column((tbl, PgClass::RelName))
            .column((col, PgAttribute::AttName))
            .expr_as(Expr::col(PgIndex::IndPred).is_not_null(), partially)
            .from(PgIndex::Table)
            .join_as(
                JoinType::Join,
                PgClass::Table,
                idx,
                Expr::col((idx, PgClass::Oid)).equals((PgIndex::Table, PgIndex::IndexRelId)),
            )
            .join_as(
                JoinType::Join,
                PgNamespace::Table,
                insp,
                Expr::col((insp, PgNamespace::Oid)).equals((idx, PgClass::RelNamespace)),
            )
            .join_as(
                JoinType::Join,
                PgClass::Table,
                tbl,
                Expr::col((tbl, PgClass::Oid)).equals((PgIndex::Table, PgIndex::IndRelId)),
            )
            .join_as(
                JoinType::Join,
                PgNamespace::Table,
                tnsp,
                Expr::col((tnsp, PgNamespace::Oid)).equals((tbl, PgClass::RelNamespace)),
            )
            .join_as(
                JoinType::Join,
                PgAttribute::Table,
                col,
                Expr::col((col, PgAttribute::AttRelId)).equals((tbl, PgAttribute::Oid)),
            )
            .join_subquery(
                JoinType::Join,
                Query::select()
                    .expr_as(
                        Expr::cust(format!("generate_series(1, {PG_INDEX_MAX_KEYS})")),
                        "subscript",
                    )
                    .take(),
                "sub",
                Expr::col((col, PgAttribute::AttNum))
                    .eq(Expr::cust("pg_index.indkey[sub.subscript]")),
            )
            .cond_where(
                Condition::all()
                    .add(Expr::col((PgIndex::Table, PgIndex::IndIsUnique)).eq(true))
                    .add(Expr::col((PgIndex::Table, PgIndex::IndIsPrimary)).eq(false))
                    .add(Expr::col((tbl, PgClass::RelName)).eq(table.to_string()))
                    .add(Expr::col((tnsp, PgNamespace::NspName)).eq(schema.to_string())),
            )
            .order_by((PgIndex::Table, PgIndex::IndexRelId), Order::Asc)
            .order_by(("sub", "subscript"), Order::Asc)
            .take()
    }
}

#[cfg(feature = "sqlx-postgres")]
impl From<SqlxRow> for UniqueIndexQueryResult {
    fn from(row: SqlxRow) -> Self {
        use crate::sqlx_types::Row;
        let row = row.postgres();
        Self {
            index_name: row.get(0),
            table_schema: row.get(1),
            table_name: row.get(2),
            column_name: row.get(3),
            is_partial: row.get(4),
        }
    }
}

#[cfg(not(feature = "sqlx-postgres"))]
impl From<SqlxRow> for UniqueIndexQueryResult {
    fn from(_: SqlxRow) -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{Alias, PostgresQueryBuilder};

    #[test]
    fn render_table_unique_indexes_without_lateral() {
        let query = SchemaQueryBuilder {}
            .query_table_unique_indexes(Alias::new("public").into(), Alias::new("booking").into());

        let sql = query.to_string(PostgresQueryBuilder);

        // GaussDB (A compatibility mode) rejects LATERAL with 42601, and the
        // capability gate in sea-orm blocks it outright.
        assert!(!sql.contains("LATERAL"));
        // The subscript expansion must stay: constant bound, per-row filtering
        // through the array subscript in the join condition, stable ordering.
        assert_eq!(
            sql,
            r#"SELECT "idx"."relname", "insp"."nspname", "tbl"."relname", "col"."attname", "indpred" IS NOT NULL AS "partially" FROM "pg_index" JOIN "pg_class" AS "idx" ON "idx"."oid" = "pg_index"."indexrelid" JOIN "pg_namespace" AS "insp" ON "insp"."oid" = "idx"."relnamespace" JOIN "pg_class" AS "tbl" ON "tbl"."oid" = "pg_index"."indrelid" JOIN "pg_namespace" AS "tnsp" ON "tnsp"."oid" = "tbl"."relnamespace" JOIN "pg_attribute" AS "col" ON "col"."attrelid" = "tbl"."oid" JOIN (SELECT generate_series(1, 32) AS "subscript") AS "sub" ON "col"."attnum" = (pg_index.indkey[sub.subscript]) WHERE "pg_index"."indisunique" = TRUE AND "pg_index"."indisprimary" = FALSE AND "tbl"."relname" = 'booking' AND "tnsp"."nspname" = 'public' ORDER BY "pg_index"."indexrelid" ASC, "sub"."subscript" ASC"#
        );
    }
}
