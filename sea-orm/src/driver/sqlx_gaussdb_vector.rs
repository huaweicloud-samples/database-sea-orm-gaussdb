// T11：GaussDB 向量 MVP（`floatvector`）。
//
// 真库定档（probe/verification-results.md §六[4]、survey/02 §四）：
// - `floatvector(d)`（OID 4409，typinput vector_in）文本形态 `'[1,2,3]'` 读写可用；
//   `boolvector`（OID 4410）本期不做
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
