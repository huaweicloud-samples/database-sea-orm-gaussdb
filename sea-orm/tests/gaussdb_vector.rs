//! GaussDB 真库向量 MVP（floatvector）测试（T11）。
//!
//! 与 `gaussdb_types.rs` 同构：`--features gaussdb` 编译，仅当
//! `GAUSSDB_PASSWORD` 设置时触碰真库；缺失则逐用例打印 skip 并成功退出。
//! 由 controller 对本地探针库执行：
//! `gaussdb://appuser:<GAUSSDB_PASSWORD>@localhost:5432/seaorm_survey`。
//!
//! 环境前提（真库定档，probe/verification-results.md §六[4]、survey/02 §四）：
//! - 探针库已有表 `vt (id int PRIMARY KEY, repr floatvector(3), b boolvector(3))`，
//!   预置数据 id=1: [1,2,3]、id=3: [7.5,8.5,9.5]
//! - GUC `enable_vectordb` 已 on
//!
//! 绑定方案：`FloatVector` → `Value::String`（文本形态）+ SQL 侧
//! `$1::floatvector` 显式 cast（方案 b，见 `sqlx_gaussdb_vector.rs` 模块文档）。
//! `boolvector`（OID 4410）本期不做。

#![cfg(feature = "gaussdb")]

use sea_orm::{ConnectionTrait, Database, DbBackend, DbErr, FloatVector, Statement};

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

/// 清理本测试写入探针表 vt 的行（幂等，先删后删保并发/重跑安全）。
async fn cleanup_vt_row(db: &sea_orm::DatabaseConnection, id: i32) {
    db.execute_unprepared(&format!("DELETE FROM vt WHERE id = {id}"))
        .await
        .expect("cleanup vt probe row");
}

/// 向量绑定插入（方案 b）+ round-trip 读回。
///
/// 写入探针表 vt（id=42，用后清理）：`$2::floatvector` cast 使文本形态
/// 参数经 vector_in 解析（真库定档 §六[4]：TEXT 声明的参数被服务端拒）。
#[tokio::test]
async fn vector_insert_bind_and_roundtrip() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    cleanup_vt_row(&db, 42).await;

    let body = async {
        let vec = FloatVector(vec![1.5, 2.5, -3.25]);
        let res = db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::GaussDB,
                // b 列（boolvector）内核级拒绝 NULL（0A000 真库定档），必须带值；
                // 私有 udt 参数须显式 cast（text 参数直插被拒，探针定档）
                "INSERT INTO vt (id, repr, b) VALUES ($1, $2::floatvector, $3::boolvector)",
                [
                    42i32.into(),
                    vec.clone().into(),
                    "[1,0,1]".to_owned().into(),
                ],
            ))
            .await
            .expect("insert via FloatVector bind + cast");
        assert_eq!(res.rows_affected(), 1, "insert rows_affected");

        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                "SELECT repr FROM vt WHERE id = 42".to_owned(),
            ))
            .await
            .expect("select inserted repr");
        assert_eq!(rows.len(), 1);
        let read_back: FloatVector = rows[0].try_get_by("repr").expect("repr as FloatVector");
        assert_eq!(read_back, vec, "floatvector round-trip");

        Ok::<(), DbErr>(())
    }
    .await;

    cleanup_vt_row(&db, 42).await;
    body.expect("vector_insert_bind_and_roundtrip body");
}

/// 预置数据读侧：`FloatVector` TryGetable 从 floatvector 列直读
/// （id=3 → [7.5,8.5,9.5]，真库定档 §六[4]）。
#[tokio::test]
async fn vector_read_probe_row() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");

    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::GaussDB,
            "SELECT repr FROM vt WHERE id = 3".to_owned(),
        ))
        .await
        .expect("select probe row repr");
    assert_eq!(rows.len(), 1, "probe row id=3 must exist (定档前提)");
    let v: FloatVector = rows[0].try_get_by("repr").expect("repr as FloatVector");
    assert_eq!(v.0, vec![7.5, 8.5, 9.5]);
}

/// ANN：`ORDER BY repr <-> $1::floatvector LIMIT 1` 参数绑定可用。
///
/// 写入自身探针行（id=43，向量 [9,9,9]，用后清理），以同向量查询 →
/// L2 距离 0，严格最近（不受表内其他行影响，确定性断言 id=43）。
#[tokio::test]
async fn vector_ann_order_by() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    cleanup_vt_row(&db, 43).await;

    let body = async {
        let probe = FloatVector(vec![9.0, 9.0, 9.0]);
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::GaussDB,
            "INSERT INTO vt (id, repr, b) VALUES ($1, $2::floatvector, $3::boolvector)",
            [
                43i32.into(),
                probe.clone().into(),
                "[0,1,0]".to_owned().into(),
            ],
        ))
        .await
        .expect("insert ann probe row");

        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::GaussDB,
                "SELECT id FROM vt ORDER BY repr <-> $1::floatvector LIMIT 1",
                [probe.into()],
            ))
            .await
            .expect("ann order by <->")
            .expect("nearest row must exist");
        let nearest: i32 = row.try_get_by("id").expect("id as i32");
        assert_eq!(nearest, 43, "nearest must be the probe row (L2 = 0)");

        Ok::<(), DbErr>(())
    }
    .await;

    cleanup_vt_row(&db, 43).await;
    body.expect("vector_ann_order_by body");
}

/// l2_distance 数值断言：绑定参数形态（对预置行 id=1 [1,2,3] → 0）与
/// 字面量形态（sqrt(3)）。距离列 cast `::float8` 消除返回类型歧义
/// （float4/float8 均安全读为 f64）。
#[tokio::test]
async fn vector_l2_distance() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");

    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::GaussDB,
            "SELECT l2_distance(repr, $1::floatvector)::float8 AS d \
             FROM vt WHERE id = 1",
            [FloatVector(vec![1.0, 2.0, 3.0]).into()],
        ))
        .await
        .expect("l2_distance bound form")
        .expect("probe row id=1 must exist (定档前提)");
    let d: f64 = row.try_get_by("d").expect("d as f64");
    assert!(d.abs() < 1e-9, "l2_distance([1,2,3], id=1) = {d}, want 0");

    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::GaussDB,
            "SELECT l2_distance('[1,2,3]'::floatvector, '[2,3,4]'::floatvector)::float8 AS d"
                .to_owned(),
        ))
        .await
        .expect("l2_distance literal form")
        .expect("literal row");
    let d: f64 = row.try_get_by("d").expect("d as f64");
    let want = 3.0f64.sqrt();
    assert!(
        (d - want).abs() < 1e-6,
        "l2_distance([1,2,3],[2,3,4]) = {d}, want {want}"
    );
}

/// GSIVFFLAT 索引：建表 → 100 行随机向量 →
/// `CREATE INDEX ... USING GSIVFFLAT (repr L2) WITH (IVF_NLIST = 16)` →
/// DROP TABLE（含索引级联）。
#[tokio::test]
async fn vector_gsivfflat_index() {
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");

    // 清理上次失败残留（表存在则连同索引一并删除）
    db.execute_unprepared("DROP TABLE IF EXISTS seaorm_vec_idx_t")
        .await
        .expect("pre drop table");

    let body = async {
        db.execute_unprepared(
            "CREATE TABLE seaorm_vec_idx_t (id int primary key, repr floatvector(3))",
        )
        .await
        .expect("create index table");

        // 确定性 LCG 伪随机（-5, 5) 区间，无 NaN/Inf（真库定档：元素不支持）
        let mut state: u32 = 0x1234_5678;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            -5.0 + (((state >> 8) & 0xFFFF) as f32) / 6553.5
        };
        for i in 0..100i32 {
            let vec = FloatVector(vec![next(), next(), next()]);
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::GaussDB,
                "INSERT INTO seaorm_vec_idx_t (id, repr) VALUES ($1, $2::floatvector)",
                [i.into(), vec.into()],
            ))
            .await
            .expect("insert vector row");
        }

        db.execute_unprepared(
            "CREATE INDEX seaorm_vec_idx_t_r_idx ON seaorm_vec_idx_t \
             USING GSIVFFLAT (repr L2) WITH (IVF_NLIST = 16)",
        )
        .await
        .expect("create GSIVFFLAT index");

        Ok::<(), DbErr>(())
    }
    .await;

    // 表连同 GSI 索引级联删除；即使 body 失败也执行（IF EXISTS 兜底）
    let dropped = db
        .execute_unprepared("DROP TABLE IF EXISTS seaorm_vec_idx_t")
        .await
        .map(|_| ());

    body.expect("vector_gsivfflat_index body");
    dropped.expect("drop index table");
}
