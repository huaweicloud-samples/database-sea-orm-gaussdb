//! GaussDB 真库 Oracle 类型（A 模式）读侧 + 写侧探针测试（T6）。
//!
//! 与 `gaussdb_smoke.rs` 同构：`--features gaussdb` 编译，仅当
//! `GAUSSDB_PASSWORD` 设置时触碰真库；缺失则逐用例打印 skip 并成功退出。
//! 由 controller 对本地探针库执行：
//! `gaussdb://appuser:<GAUSSDB_PASSWORD>@localhost:5432/seaorm_survey`。
//!
//! 定档出处缩写：
//! - [probe §二] = probe/verification-results.md §二（类型定档）
//! - [probe §六[4]] = 同文件 §六[4]（floatvector wire 行为：私有 udt 文本传输）
//! - [survey/02] = survey/02-GaussDB方言差异与向量能力.md §类型定档表

#![cfg(feature = "gaussdb")]

use sea_orm::entity::prelude::{DateTime, Decimal};
use sea_orm::{ConnectionTrait, Database, DbBackend, DbErr, Statement};

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

/// 唯一表名，避免并行运行 / 上次失败残留造成的冲突。
fn unique_table() -> String {
    format!(
        "seaorm_types_t_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos()
    )
}

/// T6 任务 DDL。附加 `c_dt date` 列：datea 引导错误断言（datea_read_guided_error）
/// 所需，不在任务给定 DDL 中，注释注明。
async fn create_types_table(db: &sea_orm::DatabaseConnection, table: &str) -> Result<(), DbErr> {
    db.execute_unprepared(&format!(
        "CREATE TABLE {table} (id int primary key, c_num number(10,2), \
         c_vc varchar2(20), c_clob clob, c_blob blob, c_raw raw(16), \
         c_ts timestamp, c_dt date)"
    ))
    .await
    .map(|_| ())
}

#[tokio::test]
async fn oracle_scalar_roundtrip() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    let table = unique_table();
    create_types_table(&db, &table)
        .await
        .expect("create types table");

    let body = async {
        // 写侧用 SQL 字面量（参数绑定写侧行为另见 oracle_lob_write_probe 探针）
        db.execute_unprepared(&format!(
            "INSERT INTO {table} VALUES (1, 123.45, 'hello-varchar', 'hello-clob', \
             NULL, NULL, TIMESTAMP '2026-01-02 03:04:05', NULL)"
        ))
        .await
        .expect("insert scalar row");

        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                format!("SELECT c_num, c_vc, c_clob, c_ts FROM {table} WHERE id = 1"),
            ))
            .await
            .expect("select scalar row");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];

        // [probe §二] number(10,2) → catalog `numeric`：NUMERIC → Decimal 是
        // sqlx 原生兼容直读（无需 T4 兜底，兜底仅整数类型）
        let num: Decimal = row.try_get_by("c_num").expect("c_num as Decimal");
        assert_eq!(num, Decimal::new(12345, 2));

        // [probe §二] varchar2 → catalog `varchar`，读取已通（sqlx 直读）
        let vc: String = row.try_get_by("c_vc").expect("c_vc as String");
        assert_eq!(vc, "hello-varchar");

        // [survey/02 + probe §二] clob → 私有 udt，sqlx 直读被严格 OID 检查拒绝
        // → T6 读侧 text repr 兜底（[probe §六[4]] 佐证私有 udt wire 层文本传输）
        let clob: String = row.try_get_by("c_clob").expect("c_clob as String");
        assert_eq!(clob, "hello-clob");

        // [未在前序探针定档 → 本断言定档] timestamp → PG 兼容 TIMESTAMP，
        // NaiveDateTime 直读
        let ts: DateTime = row.try_get_by("c_ts").expect("c_ts as NaiveDateTime");
        assert_eq!(
            ts,
            chrono::NaiveDateTime::parse_from_str("2026-01-02 03:04:05", "%Y-%m-%d %H:%M:%S")
                .expect("parse expected timestamp")
        );

        Ok::<(), DbErr>(())
    }
    .await;

    // 无论 body 成败都清理临时表
    let dropped = db
        .execute_unprepared(&format!("DROP TABLE {table}"))
        .await
        .map(|_| ());

    body.expect("oracle_scalar_roundtrip body");
    dropped.expect("drop types table");
}

#[tokio::test]
async fn datea_read_guided_error_and_empty_string_null() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    let table = unique_table();
    create_types_table(&db, &table)
        .await
        .expect("create types table");

    let body = async {
        // c_dt 用 sysdate 表达式写入（date 列拒收 ISO 文本，[probe §二]），
        // 非 NULL 的 datea 值才能走到类型引导分支（NULL 属数据语义，走标准
        // TryGetError::Null，不劫持类型引导）
        db.execute_unprepared(&format!(
            "INSERT INTO {table} (id, c_vc, c_dt) VALUES (1, '', sysdate)"
        ))
        .await
        .expect("insert row with empty varchar and sysdate date");

        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                format!("SELECT c_dt, c_vc FROM {table}"),
            ))
            .await
            .expect("select row");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];

        // [T6 决策 + survey/02 + probe §二] date 列暴露为私有 udt `datea`
        // （文本输出 Oracle 形态 DD-MON-RR 且拒收 ISO），sea-orm 不做原生映射，
        // 读取返回引导性错误（引导用 timestamp），不从 datea 解码日期
        let err = row
            .try_get_by::<String, _>("c_dt")
            .expect_err("datea read must fail with guided error");
        let msg = format!("{err}");
        assert!(
            msg.contains("datea") && msg.contains("timestamp"),
            "expected guided error mentioning 'datea' and 'timestamp', got: {msg}"
        );

        // [probe §二] 空串 → NULL（服务端行为，实锤条目的真库回归断言）
        let null_rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                format!("SELECT c_vc IS NULL AS vc_is_null FROM {table} WHERE id = 1"),
            ))
            .await
            .expect("select is-null");
        let vc_is_null: bool = null_rows[0]
            .try_get_by("vc_is_null")
            .expect("vc_is_null as bool");
        assert!(vc_is_null);

        Ok::<(), DbErr>(())
    }
    .await;

    let dropped = db
        .execute_unprepared(&format!("DROP TABLE {table}"))
        .await
        .map(|_| ());

    body.expect("datea_read_guided_error body");
    dropped.expect("drop types table");
}

#[tokio::test]
async fn oracle_lob_write_probe() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    let table = unique_table();
    create_types_table(&db, &table)
        .await
        .expect("create types table");

    let body = async {
        // ---- blob：bind Vec<u8>（Value::Bytes → bytea 参数）直插 ----
        // [survey/02] Dapper 定档「blob 可直传 byte[]」；sqlx 侧由本探针确认
        let blob: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF];
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::GaussDB,
            format!("INSERT INTO {table} (id, c_blob) VALUES ($1, $2)"),
            [1i32.into(), blob.clone().into()],
        ))
        .await
        .expect("blob direct byte[] insert (probe)");

        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                format!("SELECT c_blob FROM {table} WHERE id = 1"),
            ))
            .await
            .expect("select c_blob");
        // 读侧：blob 私有 udt → T6 gaussdb_lob_bytes（text repr 优先 hex 解析）
        let read_back: Vec<u8> = rows[0].try_get_by("c_blob").expect("c_blob as Vec<u8>");
        assert_eq!(read_back, blob);

        // ---- raw：写侧双形态探针 ----
        // 形态 1：bind String hex 文本直插（探明服务端是否接受 text → raw 隐式转换）
        // 形态 2：`hextoraw($2)`（[survey/02] 定档形态：raw 拒收 bytea，
        //         写入需 hextoraw(hex 文本)）
        // 测试代码按先 1 后 2 探测，运行时择通过形态；两种形态的定档结果由
        // 控制器运行日志（probe[raw] 前缀）落档。
        let hex: String = "DEADBEEF0123456789ABCDEF01234567".to_owned();
        let form1 = db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::GaussDB,
                format!("INSERT INTO {table} (id, c_raw) VALUES ($1, $2)"),
                [2i32.into(), hex.clone().into()],
            ))
            .await;
        match form1 {
            Ok(_) => eprintln!("probe[raw]: direct text->raw implicit conversion = OK"),
            Err(e) => {
                eprintln!("probe[raw]: direct text->raw rejected ({e}); hextoraw($2) form adopted");
                db.execute_raw(Statement::from_sql_and_values(
                    DbBackend::GaussDB,
                    format!("INSERT INTO {table} (id, c_raw) VALUES ($1, hextoraw($2))"),
                    [2i32.into(), hex.clone().into()],
                ))
                .await
                .expect("raw insert via hextoraw (probe form 2)");
            }
        }

        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                format!("SELECT c_raw FROM {table} WHERE id = 2"),
            ))
            .await
            .expect("select c_raw");
        // 读侧断言按 hex 形态假设（写侧 hex 语义对称，[survey/02]）；
        // 若真库 text repr 非 hex 形态，此断言失败即定档差异信号
        let raw_bytes: Vec<u8> = rows[0].try_get_by("c_raw").expect("c_raw as Vec<u8>");
        assert_eq!(raw_bytes, hex_to_bytes(&hex));

        Ok::<(), DbErr>(())
    }
    .await;

    let dropped = db
        .execute_unprepared(&format!("DROP TABLE {table}"))
        .await
        .map(|_| ());

    body.expect("oracle_lob_write_probe body");
    dropped.expect("drop types table");
}

/// hex 文本 → 字节（测试侧期望值构造）。
fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}
