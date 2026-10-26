# SeaORM × GaussDB 适配说明（GAUSSDB.md）

> 基于 SeaORM 2.0.4 / GaussDB Kernel 507.0.0（集中式 / A 兼容，`DBCOMPATIBILITY = 'A'`）真库实测定档（2026-10-26）。
> 结构对标 Dapper 适配说明；差异证据链见 `probe/verification-results.md` 与 `survey/02-GaussDB方言差异与向量能力.md`；
> 跳过/不支持用例见 `gaussdb-skipped-tests.md`；业务接入见 `docs/gaussdb-guide.md`；测试方法见 `docs/gaussdb-testing.md`。
> 本适配仅覆盖 A 兼容模式；M-Compatibility 未适配。sqlx-postgres（纯 Rust PG wire 协议）直连，无需任何外部客户端库。

## 1. 症状速查表（按错误码/异常反查）

| 错误码/异常 | 触发场景 | 处置 | 详见 |
|---|---|---|---|
| 42601 | 手写 `INSERT ... ON CONFLICT ...` | A 模式无该子句；ORM 渲染层已自动改写为 `ON DUPLICATE KEY UPDATE`，手写 SQL 改用同语法 | §4.2.1 |
| 42601 | `LATERAL` join / 聚合 `FILTER (WHERE ...)` | 能力门在 SQL 出进程前拦截，`DbErr::BackendNotSupported` 附重写指引 | §4.2.4 |
| 42601 | `in (...)` 数组绑定单参数 | 改写为 `= any($ids)` 形态 | §4.2.6 |
| 42601 | bootstrap/部分 DDL 场景的 `IF NOT EXISTS` | `CREATE TABLE IF NOT EXISTS` / `ADD COLUMN IF NOT EXISTS` 实测可用；受限场景剥离 + 存在性预检 | §4.2.5 |
| 22007 | `date` 列写 ISO 文本（`'2026-10-21'`） | 服务端要求 `DD-MON-RR` 形态；引导用 `timestamp` 类型 | §4.1.1 |
| 0A000 | `CREATE INDEX ... USING hash` | 不支持 row store；改用 btree | §4.2.3 |
| 0A000 | 临时表建 `serial` / IDENTITY 列 | 永久表 IDENTITY 可用，临时表禁用 | §4.2.2 |
| 0A000 | `boolvector` 列写 NULL（INSERT 漏列） | 内核级拒绝；INSERT 必带值或给列 DEFAULT | §4.3.3 |
| 22000 | 向量元素含 NaN/Inf（`NaN not allowed in vector`）/ 维度不符 | 应用侧保证数据合法；维度不符错误含维度值说明 | §4.3.3 |
| 29003 | opfusion（SQL bypass 执行器）内核断言 | connector 建池已统一 `enable_opfusion=off`，用户无感 | §3.2 |
| 42P07 | `DROP TABLE` 后重建同表报关系已存在 | IDENTITY 附属序列残留；补 `DROP SEQUENCE IF EXISTS {table}_{col}_identity` | §4.4.2 |
| 42883 | `gen_random_uuid()` / `sys_guid()` / `uuid_generate_v4()` / `to_regclass()` | 函数不存在；UUID 主键应用侧生成；存在性探测走 information_schema（已适配） | §4.1.5、§4.2.7 |
| 22001 | `varchar2(n)` 写超限 | 长度按 UTF-8 字节计（无截断，硬报错），应用侧控制 | §4.1.4 |
| InvalidCast / 引导错误 | `datea` / `clob` / `blob` / `raw` 私有类型直读 | gaussdb 读侧通道已兜底（clob→String、blob/raw→Vec<u8>）；`datea` 读取返回引导错误（用 timestamp） | §4.1 |
| `server_version = 9.2.4` | 依赖版本号的特性探测 | 恒报假象，勿信；connector 已验证不受影响 | §3.5 |

## 2. 快速开始

开启 `gaussdb` feature（不含 runtime，与上游同构设计，需按需叠加 runtime feature）：

```toml
# 最小 feature 组合
sea-orm = { path = "...", features = ["gaussdb", "runtime-tokio-native-tls"] }
```

连接与首查（与 `tests/gaussdb_smoke.rs` 同形态）：

```rust
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

let db = Database::connect("gaussdb://appuser:<PASSWORD>@localhost:5432/seaorm_survey").await?;
assert_eq!(db.get_database_backend(), DbBackend::GaussDB); // gaussdb:// → GaussDB 后端

let rows = db.query_all_raw(Statement::from_string(DbBackend::GaussDB, "SELECT 1")).await?;

// RETURNING 可用；identity 主键回填已适配 NUMERIC 报告（i32/i64 直读）
let res = cake::ActiveModel { name: Set("hello".into()), ..Default::default() }
    .insert(&db).await?; // Entity::insert ... RETURNING id 打通
```

`gaussdb://` scheme 复用 PostgreSQL 连接栈（sqlx-postgres wire 协议）；MD5 认证、IDENTITY、
PL/SQL 函数 DDL 均经真库探针验证。

## 3. 部署要求

### 3.1 认证：必须 MD5

服务端需 `password_encryption_type = 1`（MD5），且 `gs_hba.conf` 对应网段放行 md5。
**云上生产实例默认 sha256（华为私有算法，非标准 SCRAM），标准 PG 协议驱动（含 sqlx）不支持**——
云上实例需网络侧方案或另行立项内核侧适配。本地 507 容器实测 `password_encryption_type = 1` 直连成功。

### 3.2 enable_opfusion：自动关闭（用户无需操作）

真库定档（2026-10-26）：opfusion（SQL bypass 执行器）对「参数化 PBE + 非 0 result formats（fetch）」
组合触发内核断言 `executor_opfusion.cpp "parameterized pbe num_pformats != 0"`（错误码 29003）。
SeaORM 的 prepared statements 正常路径即可踩中（如迁移器 `seaql_migrations` 的参数化查询）。
标准执行器无此限制。**GaussDB connector 建池时统一追加连接级 GUC `enable_opfusion=off`**，
全量真库回归 9/9 绿，既有路径零影响。

### 3.3 enable_vectordb：向量功能前置

使用向量类型/索引需 GUC `enable_vectordb = on`。涉及 stream 计划/复制时注意 `extra_float_digits = 3`
（避免 float32 精度不一致）。

### 3.4 targetServerType（集中式建议）

集中式部署建议连接串追加 `targetServerType=master`（Hibernate 样本实践；该参数为 PG 驱动参数，
sqlx-postgres 是否支持尚未经探针确认，属建议项而非定档项）。

### 3.5 server_version 假象

服务端恒报 `9.2.4`。任何依赖版本号的特性探测都会被骗；本适配 connector 已验证不受影响，
业务侧请勿做版本探测分支。

### 3.6 扩展协议（PBE）行为

PL/SQL 函数 DDL 经 sqlx 扩展协议实测可执行，无需补丁。**.NET Npgsql 侧的「Parse 阶段按分号
截断」结论不可照搬到 Rust 栈。**

## 4. 兼容性差异与使用约束

### 4.1 类型系统（A 模式 Oracle 类型通道）

| 差异 | A 模式行为 | 适配与约束 |
|---|---|---|
| 4.1.1 `date` | wire 层暴露为私有类型 `datea`；文本输出 Oracle 形态 `DD-MON-RR`，ISO 输入被拒（22007） | 不做原生映射；读取返回引导错误（引导用 `timestamp`） |
| 4.1.2 `raw`/`blob`/`clob` | 私有类型 | `blob` 可直传 `byte[]`（bind `Vec<u8>`）；`raw` 拒收 bytea，写入需 `hextoraw(hex文本)`；`clob` 读侧 text repr 兜底为 `String`；blob/raw 读侧为 `Vec<u8>` |
| 4.1.3 `number` | 复用 numeric 通道，39 位保真 | 文本 `\|x\|<1` 丢前导零（服务端行为，读取侧归一化） |
| 4.1.4 `varchar2(n)` | 长度按 UTF-8 字节计 | 无截断，超限硬报 22001 |
| 空字符串 | 写入 `''` 回读 NULL | Oracle 语义；业务判空逻辑注意 |
| 4.1.5 uuid | 无 `gen_random_uuid()` / `sys_guid()` / `uuid_generate_v4()`（42883） | 主键 UUID 应用侧生成 |
| 标识符 | 未引号标识符折叠为小写 | PG 语义，非 Oracle 大写 |
| RLS | 引擎生效，但 `pg_policies` 等目录缺席 | 结构性不可观察；introspection/verify 对 RLS warn 化 |
| 向量类型 | `floatvector`（OID 4409）/ `boolvector`（OID 4410） | 见 §4.3；上游 `postgres-vector`（pgvector crate）不能直接复用 |

### 4.2 SQL 方言

| 差异 | A 模式行为 / 替代 |
|---|---|
| 4.2.1 upsert | 无 `ON CONFLICT`（42601）。渲染层改写为 `ON DUPLICATE KEY UPDATE`（支持 `excluded.col`）；DO NOTHING 语义用**非键列自赋值**模拟（键列自赋值也被内核拒绝）；无可用非键列（如单列主键表）时显式报错 |
| 4.2.2 IDENTITY | `GENERATED BY DEFAULT AS IDENTITY` 建表可用（sea-orm 2.0 默认形态）；临时表禁 `serial`/IDENTITY（0A000）。详见 §4.4 |
| 4.2.3 索引 | hash 索引不支持（0A000 `does not support row store`）；btree 兼容。向量索引用 `USING GSIVFFLAT` / GsDiskANN（§4.3.4） |
| 4.2.4 LATERAL / 聚合 FILTER | 不支持；能力门显式拒绝（§4.2.4 细则见 §1 表行与 `docs/gaussdb-guide.md` §7） |
| 4.2.5 IF NOT EXISTS | `CREATE TABLE IF NOT EXISTS` / `ADD COLUMN IF NOT EXISTS` 探针通过；Prisma 记录的 42601 限于 bootstrap/部分 DDL 场景 |
| 4.2.6 in-list | 数组绑定单参数 + 不改写 SQL 的 `in (...)` 必炸 42601，须 `= any($ids)` 形态 |
| 4.2.7 to_regclass | 缺失（42883）；存在性探测已适配走 information_schema 路径 |
| RETURNING | 支持 |
| `WITH RECURSIVE` | 支持（递归 CTE 可用于图式查询） |
| `ALTER TYPE ADD VALUE` | 支持，且事务内可用（宽于 PG 12+） |
| jsonb 函数 | `jsonb_build_array`/`jsonb_agg`/`jsonb_object_agg`/`jsonb_path_query` 缺失，有替代写法 |

### 4.3 向量能力（507 内置）

| 项 | 结论 |
|---|---|
| 4.3.1 类型 | `floatvector(d)` 定长维度向量，float32 语义（±3.402E+38，约 6 位十进制精度）；整列可为 NULL。`boolvector(d)` bool 元素向量，仅支持相等比较；**本期未实现类型通道**（二期） |
| 4.3.2 wire 格式 | floatvector 返回为二进制 `i16 dim \| i16 unused \| f32 大端 × dim`（非标准 PG 数组）；解码已实现。绑定走文本形态 `'[1,2,3]'` + SQL 侧 `$1::floatvector` 显式 cast（TEXT 声明的参数被服务端拒） |
| 4.3.3 元素约束 | 不支持 NULL/NaN/Inf 元素（22000）；**boolvector 列内核级拒 NULL**（0A000），INSERT 必须带值，建表建议给 boolvector 列 DEFAULT |
| 4.3.4 索引 | `CREATE INDEX ... USING GSIVFFLAT (repr L2) WITH (IVF_NLIST = 256)`；适用 1 万~200 万行，更大规模用 GsDiskANN。`ivf_nlist` 1~65535（默认 100）；查询 GUC `gsivfflat_probes` 建议 = ivf_nlist 的 10%；运维函数 `gs_ivfflat_inspect('索引名')`；数据变更 >20% 建议重建；truncate 后索引失效需重建 |
| 4.3.5 距离 | 函数 `l2_distance` / `vector_l2_squared_distance` / `cosine_distance` / `vector_spherical_distance` / `inner_product`（不支持 `*` 运算符）；运算符 `<->`（L2 距离，ANN 检索排序用）；与 pgvector 函数名重合但类型名不同、wire OID 私有 |

### 4.4 IDENTITY 序列

| 差异 | A 模式行为 | 适配 |
|---|---|---|
| 4.4.1 RETURNING 类型 | identity 主键 `RETURNING id` 列报告为 **NUMERIC**（非 INT4/INT8） | gaussdb 解码兜底已适配（i8/i16/i32/i64 直读）；上游断言 INT4 的用例按 A 模式定档 |
| 4.4.2 序列生命周期 | 附属序列独立于表，命名 `{table}_{col}_identity`；**`DROP TABLE` 不级联删除** → 残留致重建表报 42P07 | 迁移 down / 测试清理须补 `DROP SEQUENCE IF EXISTS` |
| 4.4.3 catalog 表现 | `information_schema.columns` 回读 id 列为 `numeric`（number 语义渗透，与 RETURNING 同源）；`is_identity` 恒为 'NO' | sync/codegen 已加 integer↔numeric 等价映射；identity 语义经 `pg_attribute.attidentity`（'a'/'d'）通道获取（已适配） |

## 5. 改动清单（相对上游）

### 5.1 sea-orm fork（分支 `gaussdb-adapter`，基线 2.0.4 tag `486a156`）

| commit | 内容 |
|---|---|
| `1e635c5` | T1：`DatabaseBackend::GaussDB` 变体、`gaussdb` feature、渲染 dispatch（24 处） |
| `99d73e7` | T1：GaussDB connector（`sqlx_gaussdb.rs`，复用 sqlx-postgres）与 inner connection wiring |
| `16da0e7` | T1：spec 修复（proxy/tracing 组合、schema-sync 显式分支、tests 编译、死代码清理） |
| `d11908a` | T1：质量修复（sqlx_query 统一 gaussdb 落点、流标记、OTel db_system_name、fmt） |
| `5c785ae` / `027c131` | T2：真库冒烟测试（连接/裸查询/RETURNING/IDENTITY 插读） |
| `6080185` | T4：NUMERIC RETURNING 解码兜底（`try_getable_gaussdb_num!`，三重门控，上游 Postgres 行为恒不变） |
| `e4e9adb` | T5：能力门（执行链路 10 个出口拦截 LATERAL / 聚合 FILTER，`DbErr::BackendNotSupported` 带重写指引） |
| `cadc7af` | T6：Oracle 类型通道读侧（clob→text、blob/raw→bytes、datea 引导错误）+ 真库类型测试 |
| `f3a9f0a` | T7：schema-sync 绿（integer↔numeric 等价映射，cfg-gated） |
| `920f962` / `106214e` | T8：codegen 端点形态单测 + fixtures 恢复 |
| `c1dc251` | T9：sea-orm-cli `gaussdb` feature + `gaussdb://` generate entity 分派 |
| `163a7f9` | T10：sea-orm-migration `gaussdb` feature + 迁移链路 + 连接级 `enable_opfusion=off` |
| `f531e20` | T11：`FloatVector` MVP（`gaussdb-vector` feature；binary 解码；`$1::floatvector` cast 定档形态） |

### 5.2 sea-query fork（分支 `gaussdb-backend`，基线 1.0.2 `f7171e1`）

| commit | 内容 |
|---|---|
| `ea1acae` | GaussDB backend 骨架（复制 postgres 全套改名 `GaussDBQueryBuilder`，`backend-gaussdb` feature，行为暂=Postgres） |
| `7014bc8` | T3：`OnConflict` 渲染为 A 模式 upsert（`ON DUPLICATE KEY UPDATE`，DO UPDATE excluded 映射） |
| `811968f` | T3 定档修复：DO NOTHING 改非键列自赋值（键列自赋值被内核拒绝）+ 空集报错 + 单测 68 passed |

### 5.3 sea-schema fork（分支 `gaussdb-discovery`）

| commit | 内容 |
|---|---|
| `890b81c` | T7：唯一索引发现去 LATERAL（`generate_series(1,32)` 等价改写，逐条件等价性论证） |
| `1657ada` | T8：discovery 携带 `pg_attribute.attidentity`；writer identity+Numeric→Serial（codegen 零改动生效） |
| `61494cd` | T9：attidentity 子查询 `::text` cast（修复原生 `"CHAR"` 单字节类型不可 String 解码） |

### 5.4 测试

- 真库测试 5 目标 15 用例：`tests/gaussdb_smoke.rs`（4）、`tests/gaussdb_types.rs`（3）、
  `tests/gaussdb_schema_sync.rs`（2）、`tests/gaussdb_vector.rs`（5）、
  `sea-orm-migration/tests/gaussdb_migration.rs`（1）。
- 三线验收（2026-10-26）：真库 15/15 + 存量 420/420（default features 零回归）+ lib 单测 266，
  合计 **701 全绿**（矩阵见 `gaussdb-skipped-tests.md` §五）。
- 跳过/不支持用例清单：`gaussdb-skipped-tests.md`（中英双语，对标 hibernate 交付机制）。

## 6. 已知边界与后续

- `boolvector` 类型通道、向量维度上限（官方文档「相关参考」页未读完）、`enable_vectordb` 作用域
  与权限为**待探针项**，未纳入本期定档。
- 云上 sha256 认证环境不可用（§3.1）；如社区强制 sha256，需内核侧适配立项。
- 测试凭据经环境变量注入（`GAUSSDB_PASSWORD`），不写入任何文档与代码。
