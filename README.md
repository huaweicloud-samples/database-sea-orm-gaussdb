# SeaORM GaussDB 适配（SeaORM 2.0.4 × Huawei GaussDB 507）

基于 [SeaORM](https://github.com/SeaQL/sea-orm) 2.0.4，适配华为 **GaussDB 507**（集中式部署，Oracle 兼容模式：建库参数 `DBCOMPATIBILITY = 'A'`，即 `sql_compatibility = A`）。
全部方言行为经真库实测定档（GaussDB Kernel 507.0.0），测试矩阵 **701 用例全绿**（真库 16 + 存量回归 420 + 单测 265）。

## 仓库结构

```
├── sea-orm/        # SeaORM 主 crate 适配（DatabaseBackend::GaussDB、连接器、能力门、
│                   #   主键回填、Oracle 类型通道、schema-sync、sea-orm-cli/migration 透传）
├── sea-query/      # sea-query 1.0.2 fork（GaussDBQueryBuilder：upsert 改写、
│                   #   A 模式方言渲染、backend-gaussdb feature）
├── sea-schema/     # sea-schema 0.18.1 fork（attidentity 发现、唯一索引查询去 LATERAL、
│                   #   identity+Numeric→Serial）
├── probe/          # 真库探针与定档记录（验证证据链）
├── GAUSSDB.md      # 适配说明：症状速查 / 部署要求 / 差异定档 / 改动清单
├── gaussdb-skipped-tests.md  # A 模式跳过与不支持用例（中英）
└── sea-orm/docs/gaussdb-guide.md    # 业务接入指南
    sea-orm/docs/gaussdb-testing.md  # 测试文档
```

## 上游与版本基线

| 仓库 | 上游 | 官方链接 | 适配基线 |
|---|---|---|---|
| sea-orm | SeaQL | https://github.com/SeaQL/sea-orm | tag `2.0.4` |
| sea-query | SeaQL | https://github.com/SeaQL/sea-query | tag `1.0.2`（sea-orm 2.0.4 依赖 `~1.0.0`） |
| sea-schema | SeaQL | https://github.com/SeaQL/sea-schema | tag `0.18.1` |
| sqlx | LaunchBadge / 社区 | https://github.com/launchbadge/sqlx | `0.9.0`（未改动，PG wire 协议复用） |
| GaussDB | 华为 | https://support.huaweicloud.com/ | Kernel 507.0.0 集中式实测 |

## 主要适配内容

**sea-orm/**（`DatabaseBackend::GaussDB`，feature `gaussdb` / `gaussdb-vector`，全部 cfg-gated，上游行为零影响）
- 连接层：`gaussdb://` 分派、MD5 认证、`enable_opfusion=off`（规避内核 29003 断言，见 GAUSSDB.md）
- 方言透传：upsert 渲染（A 模式 `ON DUPLICATE KEY UPDATE`）、能力门（LATERAL / 聚合 FILTER 显式拒绝）
- 主键回填：A 模式 `RETURNING` identity 列报告为 NUMERIC 的解码兜底
- Oracle 类型通道：`number` / `varchar2` / `clob` / `blob` / `raw`（hex）/ `datea` 引导
- 向量能力：`FloatVector` / `BoolVector` 类型 + `$1::floatvector` cast 绑定 + ANN 查询 + 距离查询 helper（`order_by_l2_distance` 等）+ `DeriveValueType` 一等实体字段（`save_as = "floatvector"`）+ GSIVFFLAT/GSDISKANN 索引 API
- schema-sync / codegen / CLI：`generate entity` 端到端（identity 主键正确推断 i32）
- 迁移：sea-orm-migration `gaussdb` feature + journey

**sea-query/**（`backend-gaussdb` feature）
- `GaussDBQueryBuilder`：`ON CONFLICT` → `ON DUPLICATE KEY UPDATE`（DO NOTHING 非键列自赋值模拟）

**sea-schema/**
- discovery 携带 `pg_attribute.attidentity`（A 模式 `is_identity` 恒 NO）
- 唯一索引发现去 LATERAL（A 模式 42601，标准 PostgreSQL 语义严格等价）

## 使用方法

业务项目 `Cargo.toml`：

```toml
[dependencies]
sea-orm = { git = "https://github.com/huaweicloud-samples/database-sea-orm-gaussdb", branch = "2.0.4",
            directory = "sea-orm", features = ["gaussdb", "runtime-tokio-native-tls"] }

[patch.crates-io]
sea-query = { git = "https://github.com/huaweicloud-samples/database-sea-orm-gaussdb", branch = "2.0.4", directory = "sea-query" }
sea-query-sqlx = { git = "https://github.com/huaweicloud-samples/database-sea-orm-gaussdb", branch = "2.0.4", directory = "sea-query/sea-query-sqlx" }
sea-schema = { git = "https://github.com/huaweicloud-samples/database-sea-orm-gaussdb", branch = "2.0.4", directory = "sea-schema" }
```

连接与基本使用：

```rust
use sea_orm::Database;

let db = Database::connect("gaussdb://user:password@host:5432/dbname").await?;
```

完整用法（实体定义、CRUD、upsert、实体生成、迁移、向量检索）见 **`sea-orm/docs/gaussdb-guide.md`**。

## 部署要求

- 服务端认证：`password_encryption_type = 1`（MD5）+ `gs_hba.conf` 放行 md5（A 模式默认 sha256 为华为私有算法，标准 PG 协议栈不支持——云上实例需降级或走代理）
- 向量能力：`enable_vectordb = on`（库级 GUC，507 默认已开）
- opfusion 执行器：gaussdb 连接器已自动 `SET enable_opfusion = off`（用户无需操作，原因见 GAUSSDB.md §3）
- 集中式部署建议连接串追加 `targetServerType=master` 语义（主库路由）

## 测试

```powershell
cd sea-orm
$env:GAUSSDB_PASSWORD='<password>'
cargo test -p sea-orm --features gaussdb,schema-sync,runtime-tokio-native-tls `
  --test gaussdb_smoke --test gaussdb_types --test gaussdb_schema_sync --test gaussdb_vector

cd sea-orm-migration
cargo test -p sea-orm-migration --features gaussdb,runtime-tokio-native-tls --test gaussdb_migration
```

运行命令、用例清单与验证矩阵详见 **`sea-orm/docs/gaussdb-testing.md`**；存量回归 420/420、lib 单测 266/266（上游 Postgres/MySQL/SQLite 零回归）。

## 已知限制

A 模式不支持的能力（LATERAL、聚合 FILTER、hash 索引、boolvector 列 NULL、`gen_random_uuid()` 等）、
语义差异（空串→NULL、datea 私有类型、identity 序列不级联、`server_version` 9.2.4 假象）
详见 **`GAUSSDB.md`** 症状速查表与 **`gaussdb-skipped-tests.md`**（中英双语，21 项定档）。
