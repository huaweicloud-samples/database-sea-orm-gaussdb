# GaussDB A-mode skipped / unsupported tests (GaussDB A 模式跳过与不支持用例)

> 本清单记录 SeaORM GaussDB 适配版本中，因 A 模式（`DBCOMPATIBILITY = 'A'`，Oracle 兼容）
> 内核限制而**跳过、拒绝或不适用**的上游测试场景与能力项。全部结论经真库实测定档
> （GaussDB Kernel 507.0.0 集中式），证据链见 `probe/verification-results.md` 与 `survey/02`。
>
> This document lists upstream test scenarios that are skipped, rejected, or not
> applicable under A-mode, with real-database evidence.

## 一、能力门显式拒绝（客户端报错，SQL 不出进程 / Rejected client-side via capability gate)

| # | 能力 / Capability | 行为 | 依据 |
|---|---|---|---|
| 1 | `LATERAL` join（含 `CROSS JOIN LATERAL`） | `DbErr::BackendNotSupported`（A 模式 42601 实测） | gsql 42601；探针 §六 |
| 2 | 聚合 `FILTER (WHERE ...)` 子句 | 同上；重写指引：`CASE WHEN` 置于聚合内部或过滤子查询 | Prisma 同构先例 |

## 二、内核限制（服务端报错 / Server-side rejections）

| # | 场景 | 服务端错误 | 说明 |
|---|---|---|---|
| 3 | `INSERT ... ON CONFLICT ...` | 42601 | A 模式无 PG upsert 语法；渲染层已改写为 `ON DUPLICATE KEY UPDATE` |
| 4 | `ON DUPLICATE KEY UPDATE` 更新**键列**（含自赋值 `id = id`） | 内核断言拒绝 | DO NOTHING 用**非键列**自赋值模拟；无可用非键列（如单列主键表）时显式报错 |
| 5 | hash 索引 `CREATE INDEX ... USING hash` | 0A000 `does not support row store` | 改用 btree；btree 兼容 |
| 6 | 临时表 `serial` / IDENTITY 列 | 0A000 `The serial column cannot be created on a temporary table` | |
| 7 | `boolvector` 列写 NULL（INSERT 漏列） | 0A000 `does not support null value` | floatvector 列可 NULL，boolvector 内核级拒绝——建表建议给 boolvector 列 DEFAULT |
| 8 | `date` 列 ISO 文本写入（`'2026-10-21'`） | 22007 要求 `DD-MON-RR` 形态 | 引导用 `timestamp`；`datea` 私有类型读取返回引导性错误 |
| 9 | opfusion（SQL bypass）执行器 | 29003 内核断言 | 连接级 `enable_opfusion = off` 已规避（connector 统一设置，用户无感） |
| 10 | `gen_random_uuid()` / `sys_guid()` / `uuid_generate_v4()` | 42883 函数不存在 | 主键 UUID 在应用侧生成 |
| 11 | `to_regclass()` | 42883 函数不存在 | 存在性探测走 information_schema（已适配） |

## 三、语义差异（行为正确但与 PG 不同，测试断言按 A 模式定档 / Semantic differences）

| # | 场景 | A 模式行为 | 影响与适配 |
|---|---|---|---|
| 12 | `RETURNING`（identity 主键） | 返回列报告为 **NUMERIC**（非 INT4/INT8） | gaussdb 解码兜底已适配；上游断言 INT4 类型的用例在本分支按 A 模式定档 |
| 13 | identity 列 catalog 类型 | `information_schema.columns` 报 `numeric`；`pg_attribute.attidentity` 可用（'a'/'d'） | sync/codegen 已加 integer↔numeric 等价映射 |
| 14 | 空字符串 | 写入 `''` 回读 NULL（Oracle 语义） | 文档化；业务判空逻辑注意 |
| 15 | `information_schema.columns.is_identity` | **恒为 'NO'**（identity 语义不可观察） | 已补 `pg_attribute.attidentity` 通道（T8） |
| 16 | `DROP TABLE` 与 IDENTITY 附属序列 | 序列 `{table}_{col}_identity` **不级联删除** | 迁移/测试清理须补 `DROP SEQUENCE IF EXISTS` |
| 17 | `floatvector` wire 格式 | 二进制 `i16 dim \| i16 unused \| f32 大端 × dim`（非标准 PG 数组） | 解码器已实现（T11） |
| 18 | `server_version` | 恒报 `9.2.4`（假象） | 任何版本探测逻辑都会被骗；connector 已验证不受影响 |
| 19 | PL/SQL 复合语句经扩展协议 | **不截断**（与 .NET Npgsql 行为相反，可正常执行） | 无需适配；.NET 侧结论勿照搬 |

## 四、部署要求导致的跳过（环境约束 / Environment constraints)

| # | 场景 | 说明 |
|---|---|---|
| 20 | 云上生产实例默认 sha256 认证 | 华为私有算法，标准 PG 协议驱动（含 sqlx）不支持；**部署要求** `password_encryption_type = 1`（MD5）+ `gs_hba.conf` 放行。云上实例需网络侧方案（见 survey/02 驱动矩阵：华为 Python 定制驱动可用于探针） |
| 21 | RLS 相关测试 | `pg_policies`/`relrowsecurity` 目录缺席（引擎生效但不可观察）——introspection/verify 对 RLS warn 化 |

## 五、验证矩阵（2026-10-26 全绿记录 / Verification matrix)

| 测试目标 | 用例数 | 结果 |
|---|---|---|
| `gaussdb_smoke`（连接/裸查询/RETURNING/IDENTITY 插读） | 4 | ✅ |
| `gaussdb_types`（Oracle 类型 round-trip/datea 引导/空串 NULL） | 3 | ✅ |
| `gaussdb_schema_sync`（sync 建表/幂等/discovery 五族全链路） | 2 | ✅ |
| `gaussdb_vector`（bind+cast round-trip/ANN/l2_distance/GSIVFFLAT 索引） | 5 | ✅ |
| `gaussdb_migration`（迁移 up/记录/down 序列清理） | 1 | ✅ |
| 存量测试（default features，Postgres/MySQL/SQLite 零回归） | 420 | ✅ |
| lib 单测（含能力门/解码兜底） | 266 | ✅ |
| **合计** | **701** | **✅ 全绿** |
