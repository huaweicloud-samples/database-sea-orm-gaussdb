# SeaORM GaussDB 测试文档

> 基于 SeaORM 2.0.4 / GaussDB Kernel 507.0.0（集中式 / A 兼容，`DBCOMPATIBILITY = 'A'`）真库实测定档（2026-10-26）。
> 覆盖说明：真库测试 5 目标 15 用例 + 存量零回归（420）+ lib 单测（266），合计 **701 全绿**；
> 验证矩阵与跳过用例清单见根目录 `gaussdb-skipped-tests.md`。

## 1. 测试目标与用例清单

### 1.1 `tests/gaussdb_smoke.rs` — 连接与核心链路冒烟（4 用例）

| 用例 | 覆盖 |
|---|---|
| `connect_smoke` | `gaussdb://` 连接 + `DbBackend::GaussDB` 判定 |
| `raw_query` | 裸查询 `SELECT 1` |
| `support_returning` | `RETURNING` 能力声明 |
| `insert_returning` | IDENTITY 建表 → `INSERT ... RETURNING id` 整数直读（T4 NUMERIC 兜底解码正向断言，i32/i64 均可） |

### 1.2 `tests/gaussdb_types.rs` — Oracle 类型（A 模式）读写探针（3 用例）

| 用例 | 覆盖 |
|---|---|
| `oracle_scalar_roundtrip` | `number(10,2)`→`Decimal`（123.45）、`varchar2`→`String`、`clob`→`String`（text repr 兜底）、`timestamp`→`NaiveDateTime` 直读 |
| `datea_read_guided_error_and_empty_string_null` | `datea` 私有类型读取返回引导错误（提示用 timestamp，sysdate 写入）；空串 `''` → NULL 回归断言 |
| `oracle_lob_write_probe` | `blob` 直传 `Vec<u8>`（bytea 参数）round-trip；`raw` 双形态写探针（text 直插 / `hextoraw($2)`，运行时择通过形态） |

### 1.3 `tests/gaussdb_schema_sync.rs` — schema-sync / discovery 全链路（2 用例，需 `schema-sync` feature）

| 用例 | 覆盖 |
|---|---|
| `schema_sync_creates_table_and_column_types` | 首次 sync 建表 → information_schema 确认表存在与列类型（id 报 `numeric` 为 A 模式定档预期）→ 二次 sync 幂等（发现已有表路径） |
| `postgres_discovery_covers_all_probe_queries` | postgres discovery 五族查询全路径（枚举/表/列/约束/唯一索引），identity 列解析为 `Type::Numeric` |

### 1.4 `tests/gaussdb_vector.rs` — 向量 MVP（floatvector，5 用例）

| 用例 | 覆盖 |
|---|---|
| `vector_insert_bind_and_roundtrip` | `FloatVector` 绑定（文本形态）+ `$2::floatvector` cast 插入 + round-trip 读回 |
| `vector_read_probe_row` | 预置行直读（binary 解码：`i16 dim | i16 unused | f32 大端 × dim`） |
| `vector_ann_order_by` | `ORDER BY repr <-> $1::floatvector LIMIT 1` 参数绑定 ANN，确定性最近邻断言 |
| `vector_l2_distance` | `l2_distance` 绑定形态（=0）与字面量形态（=sqrt(3)）数值断言 |
| `vector_gsivfflat_index` | 100 行随机向量 → `CREATE INDEX ... USING GSIVFFLAT (repr L2) WITH (IVF_NLIST = 16)` → 表连同索引级联删除 |

### 1.5 `sea-orm-migration/tests/gaussdb_migration.rs` — 迁移链路 journey（1 用例）

| 用例 | 覆盖 |
|---|---|
| `gaussdb_migration_journey` | `Migrator::up`（install 建表 `seaql_migrations` → identity DDL 渲染 → seed 行 → 迁移记录）→ `down`（DROP TABLE + 补清 identity 附属序列 + 删迁移记录），全程表/序列/记录三重断言 |

## 2. 运行方式

- **环境变量**：`GAUSSDB_PASSWORD`（真库口令）。未设置时所有用例打印
  `skipped: GAUSSDB_PASSWORD not set` 并成功退出（守卫模式，CI 无凭据也绿）。
  凭据不入库（凭据纪律）。
- **目标库**：`gaussdb://appuser:<GAUSSDB_PASSWORD>@localhost:5432/seaorm_survey`
  （本地 507 探针库；服务端 `password_encryption_type = 1` MD5，`enable_vectordb = on`）。
- **向量用例前置**：探针库需有表 `vt (id int PRIMARY KEY, repr floatvector(3), b boolvector(3))`，
  预置数据 id=1 `[1,2,3]`、id=3 `[7.5,8.5,9.5]`（用例文档头部注明）。

命令（仓库根 workspace）：

```bash
# smoke / types / vector（gaussdb feature 不含 runtime，与上游同构设计）
cargo test --features gaussdb,runtime-tokio-native-tls \
    --test gaussdb_smoke --test gaussdb_types --test gaussdb_vector

# schema-sync 目标需叠加 schema-sync feature
cargo test --features gaussdb,schema-sync,runtime-tokio-native-tls \
    --test gaussdb_schema_sync

# 迁移目标在 sea-orm-migration 独立 workspace（自带 [patch.crates-io] 对齐）
cd sea-orm-migration
cargo test --features gaussdb --test gaussdb_migration
```

lib 单测（含能力门 / 解码兜底，不碰真库）：

```bash
cargo test --features gaussdb --lib
```

## 3. 验证矩阵（2026-10-26 全绿记录）

引用 `gaussdb-skipped-tests.md` §五：

| 测试目标 | 用例数 | 结果 |
|---|---|---|
| `gaussdb_smoke`（连接/裸查询/RETURNING/IDENTITY 插读） | 4 | 通过 |
| `gaussdb_types`（Oracle 类型 round-trip/datea 引导/空串 NULL） | 3 | 通过 |
| `gaussdb_schema_sync`（sync 建表/幂等/discovery 五族全链路） | 2 | 通过 |
| `gaussdb_vector`（bind+cast round-trip/ANN/l2_distance/GSIVFFLAT 索引） | 5 | 通过 |
| `gaussdb_migration`（迁移 up/记录/down 序列清理） | 1 | 通过 |
| 存量测试（default features，Postgres/MySQL/SQLite 零回归） | 420 | 通过 |
| lib 单测（含能力门/解码兜底） | 266 | 通过 |
| **合计** | **701** | **全绿** |

## 4. 存量零回归说明

- 存量集成测试 420/420（default features，Postgres/MySQL/SQLite 基线对照）与 lib 单测 266
  在 T4/T10/T11 等关键节点反复复跑，均零回归；gaussdb 侧全部改动 cfg-gated 于 `gaussdb`
  feature，不触碰上游 Postgres 行为（T4 解码兜底为三重门控：`from_gaussdb` 标记 + NUMERIC
  报告 + 解码失败才走兜底）。
- 真库增量回归口径：opfusion 规避方案落地后全量真库回归 9/9 绿（T10），向量 MVP 后 266 lib
  零回归（T11）。
- 跳过/不支持用例（能力门拒绝、内核限制、语义差异、部署约束四类共 21 条）见
  `gaussdb-skipped-tests.md` §一~§四，A/M 双语清单对标 hibernate 交付机制。
