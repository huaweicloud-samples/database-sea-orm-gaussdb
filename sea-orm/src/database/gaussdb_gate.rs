//! GaussDB A-mode capability gate.
//!
//! GaussDB A mode (`DBCOMPATIBILITY='A'`) rejects some PostgreSQL syntax with
//! error 42601 (syntax error) at execution time. This gate intercepts such
//! statements right before they leave the process, so callers get a
//! [`DbErr::BackendNotSupported`] with a rewrite hint instead of a server-side
//! syntax error.
//!
//! The check runs on the rendered SQL text, which covers every statement
//! source (query builders, raw SQL, migrations) without walking the
//! `sea-query` AST. The matched fragments are the exact tokens emitted by the
//! upstream renderers, so false positives are limited to string literals
//! containing those tokens.

use crate::{DbBackend, DbErr, Statement};

/// SQL fragments rendered by the patched `sea-query` for features that
/// GaussDB A-mode does not support, paired with a rewrite hint.
///
/// - `LATERAL `: emitted by `prepare_join_table_ref` for lateral joins.
/// - ` FILTER (WHERE `: emitted by `prepare_function_arguments` for the
///   aggregate `FILTER (WHERE ...)` clause.
const UNSUPPORTED: &[(&str, &str)] = &[
    (
        "LATERAL ",
        "LATERAL join is not supported by GaussDB A-mode (server reports 42601); rewrite the query as a correlated subquery",
    ),
    (
        " FILTER (WHERE ",
        "aggregate FILTER (WHERE ...) clause is not supported by GaussDB A-mode (server reports 42601); rewrite it as CASE WHEN inside the aggregate or as a filtered subquery",
    ),
];

/// Gate a statement that is about to be sent to `self`'s backend: rejects
/// GaussDB A-mode-unsupported SQL, passes everything else through.
pub(crate) fn assert_stmt(stmt: &Statement) -> Result<(), DbErr> {
    if stmt.db_backend == DbBackend::GaussDB {
        assert_a_mode_supported(&stmt.sql)
    } else {
        Ok(())
    }
}

/// Reject SQL that GaussDB A-mode answers with 42601 instead of executing it.
pub(crate) fn assert_a_mode_supported(sql: &str) -> Result<(), DbErr> {
    for (fragment, reason) in UNSUPPORTED {
        if sql.contains(fragment) {
            return Err(DbErr::BackendNotSupported {
                db: "GaussDB",
                ctx: reason,
            });
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "gaussdb"))]
mod render_tests {
    use super::assert_a_mode_supported;
    use sea_query::{
        Alias, Cond, Expr, ExprTrait, Func, JoinType, MysqlQueryBuilder, PostgresQueryBuilder,
        Query, SelectStatement,
    };

    fn lateral_statement() -> SelectStatement {
        Query::select()
            .column(("font", "name"))
            .from("font")
            .join_lateral(
                JoinType::LeftJoin,
                Query::select().column("id").from("glyph").take(),
                "sub_glyph",
                Expr::col(("font", "id")).equals(("sub_glyph", "id")),
            )
            .to_owned()
    }

    fn filtered_aggregate() -> SelectStatement {
        Query::select()
            .expr_as(
                Func::count(Expr::val(1)).filter(Cond::all().add(Expr::col("character").eq("foo"))),
                Alias::new("filtered_total"),
            )
            .from("character")
            .to_owned()
    }

    #[test]
    fn postgres_lateral_renders_normally() {
        let sql = lateral_statement().to_string(PostgresQueryBuilder);
        assert!(sql.contains("LATERAL ("), "unexpected SQL: {sql}");
    }

    #[test]
    fn gaussdb_lateral_join_is_rejected() {
        let sql = lateral_statement().to_string(sea_query::GaussDBQueryBuilder);
        let err = assert_a_mode_supported(&sql).unwrap_err();
        match err {
            crate::DbErr::BackendNotSupported { db, ctx } => {
                assert_eq!(db, "GaussDB");
                assert!(ctx.contains("LATERAL"), "unexpected ctx: {ctx}");
                assert!(ctx.contains("A-mode"), "unexpected ctx: {ctx}");
            }
            other => panic!("expected BackendNotSupported, got {other:?}"),
        }
    }

    #[test]
    fn gaussdb_aggregate_filter_is_rejected() {
        let sql = filtered_aggregate().to_string(sea_query::GaussDBQueryBuilder);
        assert!(sql.contains(" FILTER (WHERE "), "unexpected SQL: {sql}");
        let err = assert_a_mode_supported(&sql).unwrap_err();
        match err {
            crate::DbErr::BackendNotSupported { ctx, .. } => {
                assert!(ctx.contains("FILTER"), "unexpected ctx: {ctx}");
                assert!(ctx.contains("A-mode"), "unexpected ctx: {ctx}");
            }
            other => panic!("expected BackendNotSupported, got {other:?}"),
        }
    }

    #[test]
    fn postgres_aggregate_filter_renders_normally() {
        let sql = filtered_aggregate().to_string(PostgresQueryBuilder);
        assert!(sql.contains(" FILTER (WHERE "), "unexpected SQL: {sql}");
    }

    #[test]
    fn plain_gaussdb_statement_passes() {
        let sql = Query::select()
            .column(("cake", "name"))
            .from("cake")
            .to_string(sea_query::GaussDBQueryBuilder);
        assert!(assert_a_mode_supported(&sql).is_ok());
    }

    #[test]
    fn other_backends_render_lateral_normally() {
        // The gate only ever sees SQL destined for GaussDB; rendering for
        // MySQL still contains LATERAL.
        let sql = lateral_statement().to_string(MysqlQueryBuilder);
        assert!(sql.contains("LATERAL ("), "unexpected SQL: {sql}");
    }
}

#[cfg(all(test, feature = "gaussdb", feature = "mock"))]
mod mock_tests {
    use crate::{ConnectionTrait, DbBackend, DbErr, MockDatabase, MockRow, Statement};

    #[smol_potat::test]
    async fn gaussdb_mock_rejects_lateral_before_execution() {
        let db = MockDatabase::new(DbBackend::GaussDB).into_connection();
        let stmt = Statement::from_string(
            DbBackend::GaussDB,
            r#"SELECT "name" FROM "font" LEFT JOIN LATERAL (SELECT "id" FROM "glyph") AS "sub_glyph" ON "font"."id" = "sub_glyph"."id""#,
        );
        // The mock has no canned result, so a query that slipped through the
        // gate would fail in the mock executor with `DbErr::Query`
        // ("`query_results` buffer is empty"); a BackendNotSupported error
        // proves the statement was rejected before execution.
        let err = db.query_all_raw(stmt).await.unwrap_err();
        match err {
            DbErr::BackendNotSupported { db: name, ctx } => {
                assert_eq!(name, "GaussDB");
                assert!(ctx.contains("LATERAL"), "unexpected ctx: {ctx}");
            }
            other => panic!("expected BackendNotSupported, got {other:?}"),
        }
    }

    #[smol_potat::test]
    async fn gaussdb_mock_rejects_aggregate_filter_before_execution() {
        let db = MockDatabase::new(DbBackend::GaussDB).into_connection();
        let stmt = Statement::from_string(
            DbBackend::GaussDB,
            r#"SELECT COUNT("character") FILTER (WHERE "character" = 'foo') FROM "character""#,
        );
        let err = db.query_all_raw(stmt).await.unwrap_err();
        match err {
            DbErr::BackendNotSupported { ctx, .. } => {
                assert!(ctx.contains("FILTER"), "unexpected ctx: {ctx}");
            }
            other => panic!("expected BackendNotSupported, got {other:?}"),
        }
    }

    #[smol_potat::test]
    async fn postgres_mock_passes_the_same_lateral_sql_through() {
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([Vec::<MockRow>::new()])
            .into_connection();
        let stmt = Statement::from_string(
            DbBackend::Postgres,
            r#"SELECT "name" FROM "font" LEFT JOIN LATERAL (SELECT "id" FROM "glyph") AS "sub_glyph" ON "font"."id" = "sub_glyph"."id""#,
        );
        let rows = db.query_all_raw(stmt).await.unwrap();
        assert!(rows.is_empty());
    }
}
