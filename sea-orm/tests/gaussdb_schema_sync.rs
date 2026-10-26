//! GaussDB A 模式 sea-schema discovery / schema-sync 真库验证（T7）。
//!
//! 与 `gaussdb_smoke.rs` 同构：`--features gaussdb,schema-sync` 编译，仅当
//! `GAUSSDB_PASSWORD` 设置时触碰真库；缺失则逐用例打印 skip 并成功退出。
//! 由 controller 对本地探针库执行：
//! `gaussdb://appuser:<GAUSSDB_PASSWORD>@localhost:5432/seaorm_survey`。
//!
//! 覆盖两条链路（sea-orm GaussDB sync 分支复用 postgres discovery，见
//! src/schema/builder.rs 的 `DbBackend::GaussDB` 分支）：
//! 1. `SchemaBuilder::sync` 往返：建表 -> information_schema 确认表与列类型；
//!    二次 sync 覆盖"发现已有表"路径（columns / constraints / unique-index 全查询）。
//! 2. `postgres::discovery::SchemaDiscovery::discover_with` 全路径冒烟：
//!    依次跑枚举（pg_type × pg_enum × pg_namespace）、表（information_schema.tables
//!    + pg_inherits × pg_class）、列（information_schema.columns +
//!    `CONCAT('"', udt_name, '"')::regtype`）、约束（information_schema 约束五视图）、
//!    唯一索引（pg_index × pg_class × pg_attribute + generate_subscripts）五类查询。

#![cfg(feature = "gaussdb")]
#![allow(unused_imports, dead_code)]

use sea_orm::sea_query::{Condition, Expr, ExprTrait, Query, SelectStatement};
use sea_orm::{ConnectionTrait, Database, DbBackend, DbErr, Statement};

/// 最小实体：3 列，覆盖 number 家族（integer）/ varchar / timestamp。
/// A 模式定档（T4/T7）：number(10,2) 列 data_type 报 `numeric`；identity 主键列
/// 同样报 `numeric`（identity 底层为 number 语义，与 RETURNING 报 NUMERIC 同源），
/// 见下方 id 断言注释。
mod gauge {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "seaorm_sync_gauge")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub created: DateTime,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

// 与 gauge 同构但表名独立——两用例默认并行执行，共用表名会互相 DROP
// 对方刚建的表（测试结果随并行调度随机），故每用例独立表名。
mod gauge2 {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "seaorm_sync_gauge2")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub created: DateTime,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// 连接守卫：取 GAUSSDB_PASSWORD；缺失则打印 skip 提示并返回 None（不 panic）。
fn gaussdb_password() -> Option<String> {
    match std::env::var("GAUSSDB_PASSWORD") {
        Ok(pwd) => Some(pwd),
        Err(_) => {
            eprintln!("skipped: GAUSSDB_PASSWORD not set");
            None
        }
    }
}

/// 探针库连接 URL（本地 5432，库 seaorm_survey，用户 appuser）。
fn gaussdb_url(pwd: &str) -> String {
    format!("gaussdb://appuser:{pwd}@localhost:5432/seaorm_survey")
}

/// 当前 schema（`CURRENT_SCHEMA()`，与 builder.rs GaussDB 分支同一探针表达式）。
async fn current_schema(db: &sea_orm::DatabaseConnection) -> Result<String, DbErr> {
    let stmt = Query::select().expr(Expr::cust("CURRENT_SCHEMA()")).take();
    db.query_one(&stmt)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("Can't get current schema".into()))?
        .try_get_by_index::<String>(0)
        .map_err(DbErr::from)
}

/// information_schema.tables 存在性（COUNT(*) -> i64，走已实锤的整数解码通道）。
async fn table_exists(
    db: &sea_orm::DatabaseConnection,
    schema: &str,
    table: &str,
) -> Result<bool, DbErr> {
    let stmt = Query::select()
        .expr(Expr::cust("COUNT(*)"))
        .from(("information_schema", "tables"))
        .cond_where(
            Condition::all()
                .add(Expr::col(("tables", "table_schema")).eq(schema.to_string()))
                .add(Expr::col(("tables", "table_name")).eq(table.to_string())),
        )
        .take();
    let count: i64 = db
        .query_one(&stmt)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("COUNT(*) must return a row".into()))?
        .try_get_by_index(0)
        .map_err(DbErr::from)?;
    Ok(count > 0)
}

/// information_schema.columns 的 data_type（format_type 标准输出）。
async fn column_data_type(
    db: &sea_orm::DatabaseConnection,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Option<String>, DbErr> {
    let stmt = Query::select()
        .expr(Expr::col(("columns", "data_type")))
        .from(("information_schema", "columns"))
        .cond_where(
            Condition::all()
                .add(Expr::col(("columns", "table_schema")).eq(schema.to_string()))
                .add(Expr::col(("columns", "table_name")).eq(table.to_string()))
                .add(Expr::col(("columns", "column_name")).eq(column.to_string())),
        )
        .take();
    let row = db.query_one(&stmt).await?;
    match row {
        Some(row) => Ok(Some(
            row.try_get_by_index::<String>(0).map_err(DbErr::from)?,
        )),
        None => Ok(None),
    }
}

/// T7 主用例：schema-sync 往返。
/// 首次 sync 建表 -> information_schema 确认表存在且列类型正确 -> 二次 sync 幂等。
#[tokio::test]
async fn schema_sync_creates_table_and_column_types() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    assert_eq!(db.get_database_backend(), DbBackend::GaussDB);

    #[cfg(feature = "schema-sync")]
    {
        // 幂等起点：清掉上次运行残留（探针定档 §三：IF EXISTS 族语法 A 模式可用；
        // T7 定档：A 模式 IDENTITY 附属序列独立于表，DROP TABLE 不级联删除，
        // 残留序列会让下次建表报 42P07，故须显式 DROP SEQUENCE）
        db.execute_unprepared("DROP TABLE IF EXISTS seaorm_sync_gauge")
            .await
            .expect("drop leftover table");
        db.execute_unprepared("DROP SEQUENCE IF EXISTS seaorm_sync_gauge_id_identity")
            .await
            .expect("drop leftover identity sequence");

        // 首次 sync：经 GaussDB 分支（postgres discovery 路径）建表
        db.get_schema_builder()
            .register(gauge::Entity)
            .sync(&db)
            .await
            .expect("first schema sync");

        let schema = current_schema(&db).await.expect("current schema");

        assert!(
            table_exists(&db, &schema, "seaorm_sync_gauge")
                .await
                .expect("query information_schema.tables"),
            "table `seaorm_sync_gauge` should exist after sync (schema {schema})"
        );

        // A 模式定档（T4/T7）：identity 列在 information_schema.columns 报 `numeric`
        // ——identity 底层为 number 语义，与 RETURNING 报 NUMERIC 同源（关联 T4）。
        // catalog 报 numeric 是预期行为，不是建表类型错误。
        assert_eq!(
            column_data_type(&db, &schema, "seaorm_sync_gauge", "id")
                .await
                .expect("query id data_type"),
            Some("numeric".to_owned()),
            "id column should be reported as `numeric` (A-mode identity number semantics, \
             same root cause as RETURNING NUMERIC, see T4)"
        );
        assert_eq!(
            column_data_type(&db, &schema, "seaorm_sync_gauge", "name")
                .await
                .expect("query name data_type"),
            Some("character varying".to_owned()),
            "name column should be `character varying`"
        );
        assert_eq!(
            column_data_type(&db, &schema, "seaorm_sync_gauge", "created")
                .await
                .expect("query created data_type"),
            Some("timestamp without time zone".to_owned()),
            "created column should be `timestamp without time zone`"
        );

        // 二次 sync：幂等（覆盖"发现已有表"路径 —— 列 / 约束 / 唯一索引三类查询全跑）
        db.get_schema_builder()
            .register(gauge::Entity)
            .sync(&db)
            .await
            .expect("second schema sync must be idempotent");
    }

    #[cfg(not(feature = "schema-sync"))]
    eprintln!("skipped: schema-sync feature not enabled");
}

/// T7 冒烟：postgres discovery 全路径（五类查询）直接对 GaussDB 执行。
#[tokio::test]
async fn postgres_discovery_covers_all_probe_queries() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");

    #[cfg(feature = "schema-sync")]
    {
        use sea_schema::postgres::def::Type;
        use sea_schema::postgres::discovery::SchemaDiscovery;

        // 幂等起点：与上一用例相同的清理（A 模式 DROP TABLE 不级联删除 IDENTITY
        // 附属序列，残留序列会让重建表报 42P07）
        db.execute_unprepared("DROP TABLE IF EXISTS seaorm_sync_gauge2")
            .await
            .expect("drop leftover table");
        db.execute_unprepared("DROP SEQUENCE IF EXISTS seaorm_sync_gauge2_id_identity")
            .await
            .expect("drop leftover identity sequence");

        // 确保目标表存在（独立运行本用例时补建）
        db.get_schema_builder()
            .register(gauge2::Entity)
            .sync(&db)
            .await
            .expect("schema sync");

        let schema_name = current_schema(&db).await.expect("current schema");
        let discovery = SchemaDiscovery::new_no_exec(&schema_name);
        let schema = discovery
            .discover_with(&db)
            .await
            .expect("discover_with must run all five probe query families against GaussDB A mode");

        let table = schema
            .tables
            .iter()
            .find(|t| t.info.name == "seaorm_sync_gauge2")
            .expect("discovered `seaorm_sync_gauge2`");

        let col_type = |name: &str| {
            table
                .columns
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.col_type.clone())
        };

        // A 模式定档（T4/T7）：identity 列 catalog 报 `numeric`（number 语义，与
        // RETURNING NUMERIC 同源，关联 T4），discovery 相应解析为 Type::Numeric。
        assert!(
            matches!(col_type("id"), Some(Type::Numeric(_))),
            "id should be discovered as Type::Numeric (A-mode identity), got {:?}",
            col_type("id")
        );
        assert!(
            matches!(col_type("name"), Some(Type::Varchar(_))),
            "name should be discovered as Type::Varchar, got {:?}",
            col_type("name")
        );
        assert!(
            matches!(col_type("created"), Some(Type::Timestamp(_))),
            "created should be discovered as Type::Timestamp, got {:?}",
            col_type("created")
        );
        assert!(
            table.columns.iter().any(|c| c.not_null.is_some()),
            "discovered columns should carry not-null info"
        );
    }

    #[cfg(not(feature = "schema-sync"))]
    eprintln!("skipped: schema-sync feature not enabled");
}
