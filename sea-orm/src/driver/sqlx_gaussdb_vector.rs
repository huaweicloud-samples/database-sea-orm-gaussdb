// T11：GaussDB 向量 MVP（`floatvector`）。
//
// 真库定档（probe/verification-results.md §六[4]、survey/02 §四）：
// - `floatvector(d)`（OID 4409，typinput vector_in）文本形态 `'[1,2,3]'` 读写可用；
// - `boolvector`（OID 4410）文本形态 `'[t,f,t]'` 可读写；列内核级拒 NULL（0A000），
//   INSERT 必须带值；wire 二进制格式未定档（解码按候选布局尝试，见
//   `decode_boolvector_raw`）
// - 绑定 `bind(String)`（Parse 声明 TEXT OID）被服务端拒；`$1::floatvector`
//   显式 cast 全通（插入 / `<->` ANN 排序 / round-trip）
// - sqlx RowDescription 能给出 `type_info.name == "floatvector"`
// - 元素不支持 NaN/Inf；float32 语义约 6 位精度
//
// 绑定路径（方案 b，实测可行）：`FloatVector` → [`sea_query::Value::String`]
// （文本形态）+ 业务 SQL 中 `$1::floatvector` 显式 cast。sea-orm 的
// Statement 值层是 `sea_query::Value`（闭合枚举），任何绑定值都经
// sea-query-sqlx 固定映射（`Value::String` → `&str`/TEXT 声明），无法在此
// 管线里为单个参数声明 OID 4409；且 sqlx 预编译路径 Bind 对所有参数统一
// 二进制格式，floatvector 的二进制编码未定档——故 sqlx `Encode` 仅作为
// 方案 a 原语提供（供 sqlx 直用场景），sea-orm 内部不经由它绑定。
// 读侧沿用 T4/T6 的 from_gaussdb 兜底模式（解码失败 + `QueryResult::from_gaussdb`
// + 列类型名 `floatvector` → text repr 解析）。

#[cfg(any(feature = "mock", feature = "proxy"))]
use crate::debug_print;
use crate::sea_query::Value;
use crate::{
    ColIdx, QueryResult, TryGetError, TryGetable, driver::sqlx_error_to_query_err, error::type_err,
};

/// GaussDB `floatvector(d)` 向量值（运行时维度，未实现维度强约束；
/// `d` 由列 DDL 决定，插入时由服务端校验）。
///
/// 绑定与读取示例见 [`FloatVector`] 模块级文档与 `tests/gaussdb_vector.rs`。
///
/// # 绑定（方案 b：文本形态 + 显式 cast）
///
/// `FloatVector` → `Value::String`（`'[1,2,3]'` 文本形态）；SQL 中必须写
/// `$1::floatvector` cast（服务端拒收 text 类型声明参数，真库定档 §六[4]）：
///
/// ```rust,ignore
/// use sea_orm::{ConnectionTrait, DbBackend, Statement, FloatVector};
///
/// let vec = FloatVector(vec![1.5, 2.5, -3.25]);
/// db.execute_raw(Statement::from_sql_and_values(
///     DbBackend::GaussDB,
///     "INSERT INTO vt (id, repr) VALUES ($1, $2::floatvector)",
///     [1i32.into(), vec.into()],
/// )).await?;
/// ```
///
/// # 读取
///
/// 实现 [`TryGetable`]，`query_all_raw` / `query_one_raw` 后 `try_get_by`：
///
/// ```rust,ignore
/// let rows = db.query_all_raw(Statement::from_string(
///     DbBackend::GaussDB, "SELECT repr FROM vt WHERE id = 3",
/// )).await?;
/// let v: FloatVector = rows[0].try_get_by("repr")?;
/// ```
///
/// # 包一层自定义类型（DeriveValueType）
///
/// 业务可用 `DeriveValueType` 包装以接入 `DeriveEntityModel`（本期 sea-orm
/// 不内置向量列的一等支持，此为业务侧最小路径）：
///
/// ```rust,ignore
/// #[derive(Clone, Debug, PartialEq, sea_orm::DeriveValueType)]
/// pub struct Repr(pub FloatVector);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct FloatVector(pub Vec<f32>);

impl FloatVector {
    /// 列类型名（sqlx RowDescription 对 OID 4409 的报告名，真库定档 §六[4]）
    pub(crate) const TYPE_NAME: &'static str = "floatvector";
    /// `floatvector` 类型 OID（真库定档 §六[4]）
    pub(crate) const OID: u32 = 4409;

    /// 文本形态 `'[1,2,3]'`。
    ///
    /// f32 用最短可 round-trip 的 Display 表示。注意：GaussDB 不接受
    /// NaN/Inf 元素（真库定档 §六[4]），此类值交由服务端拒绝。
    pub fn to_text(&self) -> String {
        format!(
            "[{}]",
            self.0
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// 解析文本形态 `'[1,2,3]'` → `Vec<f32>`（元素容忍空白，空向量拒绝）。
    pub(crate) fn parse_text(s: &str) -> Result<Self, String> {
        let inner = s
            .trim()
            .strip_prefix('[')
            .ok_or_else(|| format!("missing leading '[' in {s:?}"))?
            .strip_suffix(']')
            .ok_or_else(|| format!("missing trailing ']' in {s:?}"))?;
        let inner = inner.trim();
        if inner.is_empty() {
            return Err(format!("empty vector in {s:?} (dimension must be >= 1)"));
        }
        let dims = inner
            .split(',')
            .map(|p| {
                p.trim()
                    .parse::<f32>()
                    .map_err(|e| format!("invalid element {p:?} in {s:?}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(dims))
    }
}

impl From<FloatVector> for Value {
    /// 绑定通道（方案 b）：文本形态进入 `Value::String`。
    /// SQL 侧必须配 `$1::floatvector` 显式 cast（真库定档 §六[4]）。
    fn from(v: FloatVector) -> Self {
        Value::String(Some(v.to_text()))
    }
}

impl crate::sea_query::Nullable for FloatVector {
    fn null() -> Value {
        Value::String(None)
    }
}

impl crate::sea_query::ValueType for FloatVector {
    fn try_from(v: Value) -> Result<Self, crate::sea_query::ValueTypeErr> {
        match v {
            Value::String(Some(s)) => {
                Self::parse_text(&s).map_err(|_| crate::sea_query::ValueTypeErr)
            }
            _ => Err(crate::sea_query::ValueTypeErr),
        }
    }

    fn type_name() -> String {
        "FloatVector".to_owned()
    }

    /// 向量类型无数组语义；取 f32 族占位（仅 array/derive 路径引用）。
    fn array_type() -> crate::sea_query::ArrayType {
        crate::sea_query::ArrayType::Float
    }

    /// floatvector 归入 sea-query 的 vector 列类型族（DDL 渲染为 `vector`，
    /// GaussDB 实际列类型为 `floatvector(d)`，MVP 不经此生成 DDL）。
    fn column_type() -> crate::sea_query::ColumnType {
        crate::sea_query::ColumnType::Vector(None)
    }
}

impl TryGetable for FloatVector {
    // mock/proxy 分支与 executor/query.rs 的 try_getable_*! 宏同款：
    // debug_print 在 debug-print 特性关闭时展开为 `true;`，需允许未用变量
    #[allow(unused_variables)]
    fn try_get_by<I: ColIdx>(res: &QueryResult, idx: I) -> Result<Self, TryGetError> {
        match &res.row {
            // gaussdb feature 依赖 sqlx-postgres，此分支必然参与编译
            crate::QueryResultRow::SqlxPostgres(row) => {
                use sqlx::Row as _;
                match row.try_get::<Option<FloatVector>, _>(idx.as_sqlx_postgres_index()) {
                    Ok(opt) => opt.ok_or_else(|| TryGetError::Null(format!("{idx:?}"))),
                    Err(err) => {
                        // T4/T6 兜底模式：解码失败 + 行来自 GaussDB 连接 +
                        // 列类型名 floatvector → text repr 解析
                        if let Some(v) = gaussdb_floatvector_text(res, idx.as_sqlx_postgres_index())
                        {
                            return v;
                        }
                        Err(sqlx_error_to_query_err(err).into())
                    }
                }
            }
            #[cfg(feature = "mock")]
            crate::QueryResultRow::Mock(row) => row.try_get(idx).map_err(|e| {
                debug_print!("{:#?}", e.to_string());
                TryGetError::Null(format!("{idx:?}"))
            }),
            #[cfg(feature = "proxy")]
            crate::QueryResultRow::Proxy(row) => row.try_get(idx).map_err(|e| {
                debug_print!("{:#?}", e.to_string());
                TryGetError::Null(format!("{idx:?}"))
            }),
            #[allow(unreachable_patterns)]
            _ => Err(type_err("FloatVector unsupported by this database backend").into()),
        }
    }
}

/// floatvector 列的 from_gaussdb 兜底解码（T4 `gaussdb_numeric_i128` /
/// T6 `gaussdb_clob_text` 同模式）：仅当行来自 GaussDB 连接且列类型名为
/// `floatvector` 时接管；否则 `None` 让调用方沿用 sqlx 原始错误。
/// NULL 返回 [`TryGetError::Null`]。
fn gaussdb_floatvector_text<I>(
    res: &QueryResult,
    index: I,
) -> Option<Result<FloatVector, TryGetError>>
where
    I: sqlx::ColumnIndex<sqlx::postgres::PgRow>,
{
    use sqlx::{Row as _, TypeInfo as _, ValueRef as _};

    if !res.from_gaussdb {
        return None;
    }
    let crate::QueryResultRow::SqlxPostgres(row) = &res.row else {
        return None;
    };
    let raw = row.try_get_raw(index).ok()?;
    if raw.type_info().name() != FloatVector::TYPE_NAME {
        return None;
    }
    Some(decode_floatvector_raw(raw).map(FloatVector))
}

/// floatvector wire 值 → `Vec<f32>`。文本格式直接解析；二进制格式按文本
/// 内容解读（GaussDB 私有类型在 wire 层以内容字节返回，T6 clob/blob 同款
/// 行为；floatvector 专属二进制布局未定档，若真库返回真实二进制则需按
/// 实际布局扩展此函数）。
fn decode_floatvector_raw(raw: sqlx::postgres::PgValueRef<'_>) -> Result<Vec<f32>, TryGetError> {
    use sqlx::ValueRef as _;

    let from = "floatvector";
    if raw.is_null() {
        return Err(TryGetError::Null(FloatVector::TYPE_NAME.to_owned()));
    }
    // Binary 分支定档（2026-10-26 真库）：服务端对 floatvector 返回二进制格式
    // `i16 dim | i16 unused | f32 大端 × dim`（id=1 的 [1,2,3] 实测字节
    // 00 03 | 00 00 | 3F800000 40000000 40400000），按 UTF-8 文本解析会报
    // "invalid utf-8 ... index 5"。二进制布局未见于公开文档，以 round-trip
    // 测试为准；若后续发现格式差异再扩展。
    let text = match raw.format() {
        sqlx::postgres::PgValueFormat::Text => raw
            .as_str()
            .map_err(|e| floatvector_decode_err(from, &e.to_string()))?,
        sqlx::postgres::PgValueFormat::Binary => {
            let bytes = raw
                .as_bytes()
                .map_err(|e| floatvector_decode_err(from, &e.to_string()))?;
            if bytes.len() >= 4 {
                let dim = i16::from_be_bytes([bytes[0], bytes[1]]) as usize;
                if bytes.len() == 4 + dim * 4 {
                    let mut out = Vec::with_capacity(dim);
                    for i in 0..dim {
                        let b = &bytes[4 + i * 4..8 + i * 4];
                        out.push(f32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                    }
                    return Ok(out);
                }
            }
            // 尺寸不符时回退文本解析，保留可读错误信息
            std::str::from_utf8(bytes).map_err(|e| floatvector_decode_err(from, &e.to_string()))?
        }
    };
    FloatVector::parse_text(text)
        .map(|v| v.0)
        .map_err(|e| floatvector_decode_err(from, &e))
}

fn floatvector_decode_err(from: &'static str, msg: &str) -> TryGetError {
    TryGetError::DbErr(crate::DbErr::TryIntoErr {
        from,
        into: "FloatVector",
        source: std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            msg.to_owned(),
        )),
    })
}

// ---------------------------------------------------------------------------
// 方案 a 原语：sqlx 直用场景的 Type/Encode/Decode。
//
// sea-orm Statement 绑定管线（sea_query::Value → sea-query-sqlx）到不了这里，
// 仅供绕过 sea-orm 值层、直接在 sqlx 上使用 FloatVector 的场景。
// ---------------------------------------------------------------------------

impl sqlx::Type<sqlx::Postgres> for FloatVector {
    /// Parse 消息声明参数 OID 4409（服务端不再把参数当 text）。
    /// 读取兼容性：列类型（经 sqlx catalog 解析为 Custom{oid: 4409}）与
    /// 本声明按 OID 直等，默认 `compatible` 即通过。
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(Self::OID))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for FloatVector {
    /// 编码为文本形态 `'[1,2,3]'` 字节。
    ///
    /// 注意：sqlx 预编译路径 Bind 对所有参数统一二进制格式，此时服务端会
    /// 调 floatvector 的二进制输入函数解析这些文本字节——floatvector 的
    /// 二进制编码未定档，该组合行为未经验证；本编码仅在 TEXT 格式语境
    /// （simple query）下可靠。sea-orm 绑定请走方案 b（`Value::String` +
    /// `$1::floatvector` cast）。
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        buf.extend(self.to_text().as_bytes());
        Ok(sqlx::encode::IsNull::No)
    }
}

impl sqlx::Decode<'_, sqlx::Postgres> for FloatVector {
    fn decode(value: sqlx::postgres::PgValueRef<'_>) -> Result<Self, sqlx::error::BoxDynError> {
        decode_floatvector_raw(value)
            .map(FloatVector)
            .map_err(|e| -> sqlx::error::BoxDynError { format!("{e:?}").into() })
    }
}

// ---------------------------------------------------------------------------
// BoolVector：GaussDB `boolvector(d)`（OID 4410），与 FloatVector 全对称。
//
// 真库定档：文本形态 `'[t,f,t]'` 可读写；列内核级拒 NULL（0A000），INSERT 必须
// 带值（NULL 读取走标准 TryGetError::Null 语义，无兜底）；wire 二进制格式未定档，
// 解码按候选布局尝试（见 `decode_boolvector_raw`），由 controller 真库测试定档。
// 绑定路径同 FloatVector 方案 b：文本形态进 `Value::String` + SQL 侧
// `$n::boolvector` 显式 cast。
// ---------------------------------------------------------------------------

/// GaussDB `boolvector(d)` 布尔向量值（运行时维度，`d` 由列 DDL 决定）。
///
/// 绑定与读取示例见 `tests/gaussdb_vector.rs`；绑定必须配
/// `$1::boolvector` 显式 cast（私有 udt 参数定档形态，同 [`FloatVector`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct BoolVector(pub Vec<bool>);

impl BoolVector {
    /// 列类型名（sqlx RowDescription 对 OID 4410 的报告名）
    pub(crate) const TYPE_NAME: &'static str = "boolvector";
    /// `boolvector` 类型 OID
    pub(crate) const OID: u32 = 4410;

    /// 文本形态 `'[t,f,t]'`（真库定档形态）。
    pub fn to_text(&self) -> String {
        format!(
            "[{}]",
            self.0
                .iter()
                .map(|b| if *b { "t" } else { "f" })
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// 解析文本形态 `[t,f,t]` → `Vec<bool>`（元素容忍空白与大小写，
    /// 接受 t/f/true/false 及 0/1 输入变体；空向量拒绝）。
    pub(crate) fn parse_text(s: &str) -> Result<Self, String> {
        let inner = s
            .trim()
            .strip_prefix('[')
            .ok_or_else(|| format!("missing leading '[' in {s:?}"))?
            .strip_suffix(']')
            .ok_or_else(|| format!("missing trailing ']' in {s:?}"))?;
        let inner = inner.trim();
        if inner.is_empty() {
            return Err(format!("empty vector in {s:?} (dimension must be >= 1)"));
        }
        let dims = inner
            .split(',')
            .map(|p| match p.trim().to_ascii_lowercase().as_str() {
                "t" | "true" | "1" => Ok(true),
                "f" | "false" | "0" => Ok(false),
                other => Err(format!(
                    "invalid element {p:?} in {s:?}: expected t/f/true/false/0/1, got {other:?}"
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(dims))
    }
}

impl From<BoolVector> for Value {
    /// 绑定通道（方案 b）：文本形态进入 `Value::String`。
    /// SQL 侧必须配 `$1::boolvector` 显式 cast（真库定档）。
    fn from(v: BoolVector) -> Self {
        Value::String(Some(v.to_text()))
    }
}

impl crate::sea_query::Nullable for BoolVector {
    fn null() -> Value {
        Value::String(None)
    }
}

impl crate::sea_query::ValueType for BoolVector {
    fn try_from(v: Value) -> Result<Self, crate::sea_query::ValueTypeErr> {
        match v {
            Value::String(Some(s)) => {
                Self::parse_text(&s).map_err(|_| crate::sea_query::ValueTypeErr)
            }
            _ => Err(crate::sea_query::ValueTypeErr),
        }
    }

    fn type_name() -> String {
        "BoolVector".to_owned()
    }

    /// 向量类型无数组语义；取 bool 族占位（仅 array/derive 路径引用）。
    fn array_type() -> crate::sea_query::ArrayType {
        crate::sea_query::ArrayType::Bool
    }

    /// boolvector 归入 sea-query 的 vector 列类型族（同 FloatVector 占位说明）。
    fn column_type() -> crate::sea_query::ColumnType {
        crate::sea_query::ColumnType::Vector(None)
    }
}

impl TryGetable for BoolVector {
    // mock/proxy 分支与 executor/query.rs 的 try_getable_*! 宏同款
    #[allow(unused_variables)]
    fn try_get_by<I: ColIdx>(res: &QueryResult, idx: I) -> Result<Self, TryGetError> {
        match &res.row {
            // gaussdb feature 依赖 sqlx-postgres，此分支必然参与编译
            crate::QueryResultRow::SqlxPostgres(row) => {
                use sqlx::Row as _;
                match row.try_get::<Option<BoolVector>, _>(idx.as_sqlx_postgres_index()) {
                    Ok(opt) => opt.ok_or_else(|| TryGetError::Null(format!("{idx:?}"))),
                    Err(err) => {
                        // T4/T6 兜底模式：解码失败 + 行来自 GaussDB 连接 +
                        // 列类型名 boolvector → text repr 解析
                        if let Some(v) = gaussdb_boolvector_text(res, idx.as_sqlx_postgres_index())
                        {
                            return v;
                        }
                        Err(sqlx_error_to_query_err(err).into())
                    }
                }
            }
            #[cfg(feature = "mock")]
            crate::QueryResultRow::Mock(row) => row.try_get(idx).map_err(|e| {
                debug_print!("{:#?}", e.to_string());
                TryGetError::Null(format!("{idx:?}"))
            }),
            #[cfg(feature = "proxy")]
            crate::QueryResultRow::Proxy(row) => row.try_get(idx).map_err(|e| {
                debug_print!("{:#?}", e.to_string());
                TryGetError::Null(format!("{idx:?}"))
            }),
            #[allow(unreachable_patterns)]
            _ => Err(type_err("BoolVector unsupported by this database backend").into()),
        }
    }
}

/// boolvector 列的 from_gaussdb 兜底解码（同 `gaussdb_floatvector_text` 模式）：
/// 仅当行来自 GaussDB 连接且列类型名为 `boolvector` 时接管；否则 `None`。
/// NULL 返回 [`TryGetError::Null`]（标准语义，列本身拒 NULL 写入）。
fn gaussdb_boolvector_text<I>(
    res: &QueryResult,
    index: I,
) -> Option<Result<BoolVector, TryGetError>>
where
    I: sqlx::ColumnIndex<sqlx::postgres::PgRow>,
{
    use sqlx::{Row as _, TypeInfo as _, ValueRef as _};

    if !res.from_gaussdb {
        return None;
    }
    let crate::QueryResultRow::SqlxPostgres(row) = &res.row else {
        return None;
    };
    let raw = row.try_get_raw(index).ok()?;
    if raw.type_info().name() != BoolVector::TYPE_NAME {
        return None;
    }
    Some(decode_boolvector_raw(raw).map(BoolVector))
}

/// boolvector wire 值 → `Vec<bool>`。
///
/// **二进制格式未定档**（2026-10-26）：服务端 wire 层返回格式未知，按
/// floatvector 定档的 4 字节头（`i16 dim | i16 unused`）族格式尝试两种候选
/// 布局——A：每元素 1 字节（0/1）；B：位压缩 ceil(dim/8) 字节（PG bit 类型
/// MSB-first 布局）——再回退 UTF-8 文本解析；全部失败则返回带字节数摘要的
/// 说明性错误（不做猜测性误读）。由 controller 真库 round-trip 测试定档，
/// 按实际布局收敛此函数。
fn decode_boolvector_raw(raw: sqlx::postgres::PgValueRef<'_>) -> Result<Vec<bool>, TryGetError> {
    use sqlx::ValueRef as _;

    let from = "boolvector";
    if raw.is_null() {
        return Err(TryGetError::Null(BoolVector::TYPE_NAME.to_owned()));
    }
    match raw.format() {
        // 文本格式已定档（'[t,f,t]'）
        sqlx::postgres::PgValueFormat::Text => {
            let text = raw
                .as_str()
                .map_err(|e| boolvector_decode_err(from, &e.to_string()))?;
            BoolVector::parse_text(text)
                .map(|v| v.0)
                .map_err(|e| boolvector_decode_err(from, &e))
        }
        sqlx::postgres::PgValueFormat::Binary => {
            let bytes = raw
                .as_bytes()
                .map_err(|e| boolvector_decode_err(from, &e.to_string()))?;
            decode_boolvector_binary(bytes).ok_or_else(|| {
                boolvector_decode_err(
                    from,
                    &format!(
                        "boolvector binary format not established (未定档): got {} bytes, \
                         head {:02x?} — extend decode_boolvector_binary after real-db 定档",
                        bytes.len(),
                        &bytes[..bytes.len().min(16)]
                    ),
                )
            })
        }
    }
}

/// boolvector 二进制候选布局解析（未定档，尝试性实现）。
fn decode_boolvector_binary(bytes: &[u8]) -> Option<Vec<bool>> {
    // [T-v1 真库定档 2026-10-27] boolvector binary 布局：
    //   i16 pad(0) | i16 dim | i64 bitmask（BE，LSB-first：bit_n = 第 n 个元素）
    // 实测样本 [t,f,t] → 00 00 | 00 03 | 00 00 00 00 00 00 00 05（mask = 0b101）
    // （此前"pad 内嵌 dim"的候选 A/B 假设被该样本否定。）
    if bytes.len() == 12 {
        let dim = i16::from_be_bytes([bytes[2], bytes[3]]);
        if dim > 0 && bytes[0] == 0 && bytes[1] == 0 {
            let dim = dim as usize;
            let mask = i64::from_be_bytes([
                bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
            ]);
            return Some((0..dim).map(|i| (mask >> i) & 1 == 1).collect());
        }
    }
    // 内容字节为 UTF-8 文本形态时按文本解析（text 通道回退）
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(v) = BoolVector::parse_text(text) {
            return Some(v.0);
        }
    }
    None
}

fn boolvector_decode_err(from: &'static str, msg: &str) -> TryGetError {
    TryGetError::DbErr(crate::DbErr::TryIntoErr {
        from,
        into: "BoolVector",
        source: std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            msg.to_owned(),
        )),
    })
}

// 方案 a 原语：sqlx 直用场景的 Type/Encode/Decode（同 FloatVector 说明）。

impl sqlx::Type<sqlx::Postgres> for BoolVector {
    /// Parse 消息声明参数 OID 4410。
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(Self::OID))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for BoolVector {
    /// 编码为文本形态 `'[t,f,t]'` 字节（TEXT 格式语境下可靠；同 FloatVector 限定）。
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        buf.extend(self.to_text().as_bytes());
        Ok(sqlx::encode::IsNull::No)
    }
}

impl sqlx::Decode<'_, sqlx::Postgres> for BoolVector {
    fn decode(value: sqlx::postgres::PgValueRef<'_>) -> Result<Self, sqlx::error::BoxDynError> {
        decode_boolvector_raw(value)
            .map(BoolVector)
            .map_err(|e| -> sqlx::error::BoxDynError { format!("{e:?}").into() })
    }
}

// ---------------------------------------------------------------------------
// 便捷距离查询 helper：`GaussDBVectorQuery` 扩展 trait。
//
// 落点探查结论（sea-query 1.0.2 / sea-orm 2.0.4）：
// - `Expr::cust_with_values` / `cust_with_exprs` 的 `$n` 是**局部占位符**
//   （tokenizer 映射到本表达式的 values[n-1]），值经 `prepare_value` 写出时
//   取全局下一参数编号——与语句中既有参数自动错开重编号（sea-query
//   `cust_with_expr` 文档示例：前置两个参数后 `$1` 渲染为 `$3`），无编号冲突；
// - 列引用不进字符串：经 `cust_with_exprs` 以 `Expr::Column` 作为局部值传入，
//   由 builder 递归渲染（带 schema/表限定与引号），等价于把列嵌进自定义串；
// - `<->` 无 `BinOper` 对应项（sea-query 无自定义二元操作符），故统一走
//   `cust_with_exprs` 单表达式形态，`::floatvector` cast 内联在自定义串里
//   （绑定定档：`Value::String` 无 cast 被拒）。
//
// 作用面：对实现 [`crate::query::QuerySelect`] 的查询构造器（`Select` 及其
// 多实体变体）与裸 [`SelectStatement`] 提供 blanket impl，不加新方法到既有
// trait，最小侵入。
// ---------------------------------------------------------------------------

use crate::query::{IntoSimpleExpr, QuerySelect};
use crate::sea_query::{Expr, Order, SelectStatement};

/// GaussDB 向量距离查询的便捷方法（`cfg(feature = "gaussdb")`）。
///
/// 所有方法对 floatvector 列生成「文本形态参数 + `$n::floatvector` 显式 cast」
/// 的绑定形态（真库定档，见 [`FloatVector`] 模块文档）。
///
/// ```rust,ignore
/// use sea_orm::{entity::*, query::*, FloatVector, GaussDBVectorQuery};
///
/// // ANN：ORDER BY repr <-> $1::floatvector LIMIT 10（L2 距离升序，最近在前）
/// let nearest = cake::Entity::find()
///     .order_by_l2_distance(cake::COLUMN.repr, FloatVector(vec![1.0, 2.0, 3.0]))
///     .limit(10)
///     .all(&db)
///     .await?;
///
/// // 半径过滤：WHERE l2_distance(repr, $1::floatvector) <= $2
/// let near = cake::Entity::find()
///     .filter_l2_distance_within(cake::COLUMN.repr, FloatVector(vec![1.0, 2.0, 3.0]), 0.5)
///     .all(&db)
///     .await?;
/// ```
pub trait GaussDBVectorQuery: Sized {
    /// 底层 `SelectStatement`（供方法改写 ORDER BY / WHERE，调用方不直接使用）。
    fn gaussdb_vector_query(&mut self) -> &mut SelectStatement;

    /// `ORDER BY <col> <-> $n::floatvector ASC`（L2 距离，最近在前）。
    fn order_by_l2_distance<C>(mut self, col: C, vec: FloatVector) -> Self
    where
        C: IntoSimpleExpr,
    {
        let expr = Expr::cust_with_exprs(
            "$1 <-> $2::floatvector",
            [col.into_simple_expr(), Expr::val(vec)],
        );
        self.gaussdb_vector_query().order_by_expr(expr, Order::Asc);
        self
    }

    /// `ORDER BY cosine_distance(<col>, $n::floatvector) ASC`（函数形式；
    /// 余弦距离，越小越相似）。
    fn order_by_cosine_distance<C>(mut self, col: C, vec: FloatVector) -> Self
    where
        C: IntoSimpleExpr,
    {
        let expr = Expr::cust_with_exprs(
            "cosine_distance($1, $2::floatvector)",
            [col.into_simple_expr(), Expr::val(vec)],
        );
        self.gaussdb_vector_query().order_by_expr(expr, Order::Asc);
        self
    }

    /// `WHERE l2_distance(<col>, $n::floatvector) <= $m`（半径过滤）。
    ///
    /// `radius` 绑定为 float8（`Value::Double`），服务端按声明的参数类型
    /// 隐式对齐左操作数，无需额外 cast。
    fn filter_l2_distance_within<C>(mut self, col: C, vec: FloatVector, radius: f64) -> Self
    where
        C: IntoSimpleExpr,
    {
        let cond = Expr::cust_with_exprs(
            "l2_distance($1, $2::floatvector) <= $3",
            [col.into_simple_expr(), Expr::val(vec), Expr::val(radius)],
        );
        self.gaussdb_vector_query().and_where(cond);
        self
    }
}

impl<T> GaussDBVectorQuery for T
where
    T: QuerySelect,
{
    fn gaussdb_vector_query(&mut self) -> &mut SelectStatement {
        self.query()
    }
}

impl GaussDBVectorQuery for SelectStatement {
    fn gaussdb_vector_query(&mut self) -> &mut SelectStatement {
        self
    }
}

#[cfg(all(test, feature = "gaussdb"))]
mod vector_query_tests {
    use super::*;
    use crate::sea_query::{Alias, Asterisk, Query};
    use crate::{DbBackend, Statement};

    fn build(stmt: &SelectStatement) -> Statement {
        crate::StatementBuilder::build(stmt, &DbBackend::GaussDB)
    }
    #[test]
    fn order_by_l2_distance_on_select_statement() {
        let mut q = Query::select();
        q.column(Asterisk).from(Alias::new("vt"));
        let stmt = q.order_by_l2_distance(
            Expr::col(Alias::new("repr")),
            FloatVector(vec![1.5, 2.5, -3.25]),
        );
        let built = build(&stmt);
        assert_eq!(
            built.sql,
            r#"SELECT * FROM "vt" ORDER BY "repr" <-> $1::floatvector ASC"#
        );
        assert_eq!(
            built.values,
            Some(crate::sea_query::Values(vec![Value::String(Some(
                "[1.5,2.5,-3.25]".to_owned()
            ))]))
        );
    }

    #[test]
    fn order_by_cosine_distance_uses_function_form() {
        let mut q = Query::select();
        q.column(Asterisk).from(Alias::new("vt"));
        let stmt = q.order_by_cosine_distance(
            Expr::col(Alias::new("repr")),
            FloatVector(vec![1.0, 0.0, 1.0]),
        );
        let built = build(&stmt);
        assert_eq!(
            built.sql,
            r#"SELECT * FROM "vt" ORDER BY cosine_distance("repr", $1::floatvector) ASC"#
        );
    }

    #[test]
    fn filter_l2_distance_within_renders_condition_and_radius() {
        let mut q = Query::select();
        q.column(Asterisk).from(Alias::new("vt"));
        let stmt = q.filter_l2_distance_within(
            Expr::col(Alias::new("repr")),
            FloatVector(vec![1.0, 2.0, 3.0]),
            0.5,
        );
        let built = build(&stmt);
        assert_eq!(
            built.sql,
            r#"SELECT * FROM "vt" WHERE l2_distance("repr", $1::floatvector) <= $2"#
        );
        assert_eq!(
            built.values,
            Some(crate::sea_query::Values(vec![
                Value::String(Some("[1,2,3]".to_owned())),
                Value::Double(Some(0.5)),
            ]))
        );
    }

    #[cfg(feature = "mock")]
    mod entity_path_tests {
        use super::*;
        use crate::tests_cfg::cake;
        use crate::{ColumnTrait, EntityTrait, QueryFilter, QueryTrait};

        #[test]
        fn helper_composes_with_entity_select_and_other_params() {
            // 实体路径（QuerySelect blanket impl）+ 与既有 WHERE 参数共存：
            // 局部 $n 重编号为全局 $2
            let stmt = cake::Entity::find()
                .filter(cake::Column::Name.eq("apple"))
                .order_by_l2_distance(cake::Column::Id, FloatVector(vec![1.0, 2.0, 3.0]))
                .build(DbBackend::GaussDB);
            assert_eq!(
                stmt.sql,
                r#"SELECT "cake"."id", "cake"."name" FROM "cake" WHERE "cake"."name" = $1 ORDER BY "cake"."id" <-> $2::floatvector ASC"#
            );
            assert_eq!(
                stmt.values,
                Some(crate::sea_query::Values(vec![
                    Value::String(Some("apple".to_owned())),
                    Value::String(Some("[1,2,3]".to_owned())),
                ]))
            );
        }

        #[test]
        fn filter_l2_distance_within_on_entity_select() {
            let stmt = cake::Entity::find()
                .filter_l2_distance_within(cake::Column::Id, FloatVector(vec![1.0, 0.0, 1.0]), 2.5)
                .build(DbBackend::GaussDB);
            assert_eq!(
                stmt.sql,
                r#"SELECT "cake"."id", "cake"."name" FROM "cake" WHERE l2_distance("cake"."id", $1::floatvector) <= $2"#
            );
            assert_eq!(
                stmt.values,
                Some(crate::sea_query::Values(vec![
                    Value::String(Some("[1,0,1]".to_owned())),
                    Value::Double(Some(2.5)),
                ]))
            );
        }
    }
}

#[cfg(all(test, feature = "gaussdb", feature = "mock"))]
mod boolvector_try_getable_tests {
    use super::*;
    use crate::IntoMockRow;
    use crate::{ConnectionTrait, DbBackend, MockDatabase, MockRow, Statement};
    use std::collections::BTreeMap;

    #[test]
    fn boolvector_try_get_via_mock_row() {
        let mut map = BTreeMap::new();
        map.insert("b".to_owned(), Value::String(Some("[t,f,t]".to_owned())));
        let row: MockRow = map.into_mock_row();
        let v: BoolVector = row.try_get("b").unwrap();
        assert_eq!(v, BoolVector(vec![true, false, true]));
    }

    #[smol_potat::test]
    async fn boolvector_try_get_by_via_mock_connection() {
        let mut map = BTreeMap::new();
        map.insert("b".to_owned(), Value::String(Some("[0,1,0]".to_owned())));
        let db = MockDatabase::new(DbBackend::GaussDB)
            .append_query_results([vec![map]])
            .into_connection();
        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::GaussDB,
                "SELECT b FROM vt".to_owned(),
            ))
            .await
            .unwrap();
        let v: BoolVector = rows[0].try_get_by("b").unwrap();
        assert_eq!(v, BoolVector(vec![false, true, false]));
    }
}
