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
//! 绑定方案：`FloatVector` / `BoolVector` → `Value::String`（文本形态）+ SQL 侧
//! `$n::floatvector` / `$n::boolvector` 显式 cast（方案 b，见 `sqlx_gaussdb_vector.rs`
//! 模块文档）。`boolvector`（OID 4410）文本读写已定档，wire 二进制格式由本文件
//! round-trip 用例配合 controller 定档。

#![cfg(feature = "gaussdb")]

use std::sync::Mutex;

// vt 表为多用例共享的探针表：测试默认并行执行，互删数据导致断言随机失败。
static VT_LOCK: Mutex<()> = Mutex::new(());
use sea_orm::{
    ActiveModelTrait, BoolVector, ColumnTrait, ConnectionTrait, Database, DbBackend, DbErr,
    EntityTrait, FloatVector, GaussDBVectorQuery, QueryFilter, Set, Statement,
    sea_query::{
        Alias, Asterisk, Expr, GaussDBQueryBuilder, Index, IndexColumn, Query,
        extension::gaussdb::GaussDBIndexExt,
    },
};

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
    let _vt_lock = VT_LOCK.lock().unwrap();
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
    let _vt_lock = VT_LOCK.lock().unwrap();
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
    let _vt_lock = VT_LOCK.lock().unwrap();
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
    let _vt_lock = VT_LOCK.lock().unwrap();
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
/// sea-query 向量索引 API（`GaussDBIndexExt`）构建
/// `CREATE INDEX ... USING GSIVFFLAT (repr L2) WITH (IVF_NLIST = 16)` →
/// DROP TABLE（含索引级联）。
///
/// 定档 SQL（§vector_gsivfflat_index，原 raw SQL 形态，仅标识符未加引号）：
/// `CREATE INDEX seaorm_vec_idx_t_r_idx ON seaorm_vec_idx_t \
///  USING GSIVFFLAT (repr L2) WITH (IVF_NLIST = 16)`
#[tokio::test]
async fn vector_gsivfflat_index() {
    let _vt_lock = VT_LOCK.lock().unwrap();
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

        let index_sql = Index::create()
            .name("seaorm_vec_idx_t_r_idx")
            .table(Alias::new("seaorm_vec_idx_t"))
            .col(IndexColumn::from(Alias::new("repr")).with_operator_class("L2"))
            .gaussdb_index_method("GSIVFFLAT")
            .gaussdb_index_with("IVF_NLIST", 16)
            .to_owned()
            .to_string(GaussDBQueryBuilder);
        db.execute_raw(Statement::from_string(DbBackend::GaussDB, index_sql))
            .await
            .expect("create GSIVFFLAT index via API");

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

/// 渲染单测（不触真库）：`GaussDBIndexExt` API 构建的索引语句与真库定档的
/// raw SQL 形态一致（仅标识符加引号，语义等价）。
#[test]
fn gsivfflat_index_api_renders_canonical_sql() {
    let sql = Index::create()
        .name("seaorm_vec_idx_t_r_idx")
        .table(Alias::new("seaorm_vec_idx_t"))
        .col(IndexColumn::from(Alias::new("repr")).with_operator_class("L2"))
        .gaussdb_index_method("GSIVFFLAT")
        .gaussdb_index_with("IVF_NLIST", 16)
        .to_owned()
        .to_string(GaussDBQueryBuilder);
    assert_eq!(
        sql,
        r#"CREATE INDEX "seaorm_vec_idx_t_r_idx" ON "seaorm_vec_idx_t" USING GSIVFFLAT ("repr" L2) WITH (IVF_NLIST = 16)"#
    );
}

// ---------------------------------------------------------------------------
// 便捷距离查询 helper（GaussDBVectorQuery）
// ---------------------------------------------------------------------------

/// helper 落库验证：`order_by_l2_distance`（`<->`）与
/// `order_by_cosine_distance`（cosine_distance 函数形式）作用于
/// `SelectStatement`，经 `db.query_all(&stmt)` 走 GaussDB 构建。
///
/// 写入探针行 id=44 向量 [9,9,9]（用后清理）；半径 0.5 的
/// `filter_l2_distance_within` 只保留探针行（id=3 距离 ≈0.866 > 0.5），
/// 两种排序均应命中 id=44。
#[tokio::test]
async fn vector_helper_distance_queries() {
    let _vt_lock = VT_LOCK.lock().unwrap();
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    cleanup_vt_row(&db, 44).await;

    let body = async {
        let probe = FloatVector(vec![9.0, 9.0, 9.0]);
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::GaussDB,
            "INSERT INTO vt (id, repr, b) VALUES ($1, $2::floatvector, $3::boolvector)",
            [
                44i32.into(),
                probe.clone().into(),
                BoolVector(vec![true, false, true]).into(),
            ],
        ))
        .await
        .expect("insert helper probe row");

        // L2：`<->` 形态
        let mut q = Query::select();
        q.column(Asterisk).from(Alias::new("vt"));
        let stmt = q
            .filter_l2_distance_within(Expr::col(Alias::new("repr")), probe.clone(), 0.5)
            .order_by_l2_distance(Expr::col(Alias::new("repr")), probe.clone())
            .limit(1)
            .to_owned();
        let rows = db.query_all(&stmt).await.expect("helper l2 ann query");
        assert_eq!(rows.len(), 1, "radius 0.5 must keep only the probe row");
        let id: i32 = rows[0].try_get_by("id").expect("id as i32");
        assert_eq!(id, 44, "nearest within radius must be the probe row");

        // cosine：函数形态
        let mut q = Query::select();
        q.column(Asterisk).from(Alias::new("vt"));
        let stmt = q
            .order_by_cosine_distance(Expr::col(Alias::new("repr")), probe)
            .limit(1)
            .to_owned();
        let rows = db.query_all(&stmt).await.expect("helper cosine query");
        let id: i32 = rows[0].try_get_by("id").expect("id as i32");
        assert_eq!(
            id, 44,
            "cosine_distance(v,v)=0 must rank the probe row first"
        );

        Ok::<(), DbErr>(())
    }
    .await;

    cleanup_vt_row(&db, 44).await;
    body.expect("vector_helper_distance_queries body");
}

// ---------------------------------------------------------------------------
// BoolVector 类型通道（OID 4410）
// ---------------------------------------------------------------------------

/// BoolVector 绑定插入（`$3::boolvector` cast）+ round-trip 读回
/// （text 定档形态 `[t,f,t]`；wire 二进制格式由本用例定档）。
#[tokio::test]
async fn boolvector_insert_bind_and_roundtrip() {
    let _vt_lock = VT_LOCK.lock().unwrap();
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    cleanup_vt_row(&db, 45).await;

    let body = async {
        let b = BoolVector(vec![true, false, true]);
        let res = db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::GaussDB,
                "INSERT INTO vt (id, repr, b) VALUES ($1, $2::floatvector, $3::boolvector)",
                [
                    45i32.into(),
                    FloatVector(vec![4.5, 5.5, 6.5]).into(),
                    b.clone().into(),
                ],
            ))
            .await
            .expect("insert via BoolVector bind + cast");
        assert_eq!(res.rows_affected(), 1, "insert rows_affected");

        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                "SELECT b FROM vt WHERE id = 45".to_owned(),
            ))
            .await
            .expect("select inserted b");
        assert_eq!(rows.len(), 1);
        let back: BoolVector = rows[0].try_get_by("b").expect("b as BoolVector");
        assert_eq!(back, b, "boolvector round-trip");

        // 预置行 b='[1,0,1]'（定档前提）
        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                "SELECT b FROM vt WHERE id = 1".to_owned(),
            ))
            .await
            .expect("select probe row b");
        let v: BoolVector = rows[0].try_get_by("b").expect("b as BoolVector");
        assert_eq!(v, BoolVector(vec![true, false, true]), "probe row b");

        Ok::<(), DbErr>(())
    }
    .await;

    cleanup_vt_row(&db, 45).await;
    body.expect("boolvector_insert_bind_and_roundtrip body");
}

/// boolvector 列内核级拒 NULL（0A000 真库定档）：INSERT 漏列必须被服务端拒绝
/// （定档测试；不做客户端兜底）。
#[tokio::test]
async fn boolvector_column_rejects_null_insert() {
    let _vt_lock = VT_LOCK.lock().unwrap();
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");
    cleanup_vt_row(&db, 46).await;

    let err = db
        .execute_unprepared("INSERT INTO vt (id) VALUES (46)")
        .await
        .expect_err("boolvector column must reject NULL (0A000)");
    let msg = format!("{err:?}");
    assert!(msg.contains("0A000"), "expected 0A000 in error, got: {msg}");
}

// ---------------------------------------------------------------------------
// DeriveValueType 一等实体字段路径
// ---------------------------------------------------------------------------

/// DeriveValueType 一等路径验证实体。
///
/// 关键形态（探明结论）：
/// - 包装类型 `Embedding(FloatVector)` 经 `DeriveValueType` 自动生成
///   From/TryGetable/ValueType/Nullable/IntoActiveValue，全部委托内层，
///   与 FloatVector 手工 impl 无冲突；
/// - `column_type = custom("floatvector")` 落在包装类型上（列类型推断/DDL）；
/// - **写侧必须配 `save_as = "floatvector"`**（实体字段属性）：INSERT/UPDATE
///   渲染 `CAST($n AS floatvector)`（等价 `$n::floatvector` 定档形态）。
///   缺省时值以 TEXT 声明参数直插，被服务端拒（绑定定档）。
mod dv_vec {
    use sea_orm::{FloatVector, entity::prelude::*};

    #[derive(Clone, Debug, PartialEq, DeriveValueType)]
    #[sea_orm(column_type = r#"custom("floatvector")"#)]
    pub struct Embedding(pub FloatVector);

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "seaorm_vec_dv_t")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        #[sea_orm(save_as = "floatvector")]
        pub emb: Embedding,
    }

    // 无关系实体（展开形态，宏要求 Relation 存在）
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// 包装类型实体的插入与读取 round-trip（表 `seaorm_vec_dv_t`，建表+清理）。
#[tokio::test]
async fn vector_derive_value_type_roundtrip() {
    let _vt_lock = VT_LOCK.lock().unwrap();
    let Some(pwd) = gaussdb_password() else {
        return;
    };
    let db = Database::connect(gaussdb_url(&pwd))
        .await
        .expect("connect gaussdb://");

    // 清理上次失败残留
    db.execute_unprepared("DROP TABLE IF EXISTS seaorm_vec_dv_t")
        .await
        .expect("pre drop dv table");

    let body = async {
        db.execute_unprepared(
            "CREATE TABLE seaorm_vec_dv_t (id int PRIMARY KEY, emb floatvector(3))",
        )
        .await
        .expect("create dv table");

        let emb = dv_vec::Embedding(FloatVector(vec![1.5, -2.5, 3.25]));
        let inserted = dv_vec::ActiveModel {
            id: Set(1),
            emb: Set(emb.clone()),
        }
        .insert(&db)
        .await
        .expect("insert via DeriveValueType entity (CAST($2 AS floatvector))");
        assert_eq!(inserted.emb, emb, "insert result carries the value");

        let m = dv_vec::Entity::find()
            .filter(dv_vec::Column::Id.eq(1))
            .one(&db)
            .await
            .expect("find dv row")
            .expect("dv row must exist");
        assert_eq!(m.emb, emb, "DeriveValueType floatvector round-trip");

        Ok::<(), DbErr>(())
    }
    .await;

    let dropped = db
        .execute_unprepared("DROP TABLE IF EXISTS seaorm_vec_dv_t")
        .await
        .map(|_| ());

    body.expect("vector_derive_value_type_roundtrip body");
    dropped.expect("drop dv table");
}
