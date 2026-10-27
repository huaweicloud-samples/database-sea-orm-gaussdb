// GaussDB connector: reuses the PostgreSQL connection stack (sqlx-postgres
// wire protocol; MD5 auth, IDENTITY columns and PL/SQL compatibility were
// probe-verified against GaussDB 507 centralized, Oracle-compat mode A).
// Dialect differences are rendered by sea-query GaussDBQueryBuilder.
// This file mirrors sqlx_postgres.rs; GaussDB dialect-specific overrides land here.
// Both the plain query stream and the transaction stream are marked GaussDB.
use futures_util::lock::Mutex;
use log::LevelFilter;
use sea_query::Values;
use std::{fmt::Write, future::Future, pin::Pin, sync::Arc};

use sqlx::{
    Connection, Executor, PgPool, Postgres, pool::PoolConnection, postgres::PgConnectOptions,
};

use sea_query_sqlx::SqlxValues;
use tracing::instrument;

use crate::{
    AccessMode, ConnectOptions, DatabaseConnection, DatabaseConnectionType, DatabaseTransaction,
    IsolationLevel, Statement, TransactionError, debug_print, error::*, executor::*,
};

use super::sqlx_common::*;

#[cfg(feature = "stream")]
use crate::QueryStream;

/// Defines the GaussDB connector (backed by [sqlx::postgres])
#[derive(Debug)]
pub struct SqlxGaussDBConnector;

/// Defines a sqlx pool for GaussDB (backed by [sqlx::PgPool])
#[derive(Clone)]
pub struct SqlxGaussDBPoolConnection {
    pub(crate) pool: PgPool,
    metric_callback: Option<crate::metric::Callback>,
    pub(crate) record_stmt_in_spans: bool,
}

impl std::fmt::Debug for SqlxGaussDBPoolConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqlxGaussDBPoolConnection {{ pool: {:?} }}", self.pool)
    }
}

impl From<PgPool> for SqlxGaussDBPoolConnection {
    fn from(pool: PgPool) -> Self {
        SqlxGaussDBPoolConnection {
            pool,
            metric_callback: None,
            record_stmt_in_spans: true,
        }
    }
}

impl SqlxGaussDBConnector {
    /// Check if the URI provided corresponds to `gaussdb://` for a GaussDB database
    pub fn accepts(string: &str) -> bool {
        string.starts_with("gaussdb://") && string.parse::<PgConnectOptions>().is_ok()
    }

    /// Add configuration options for the GaussDB database
    #[instrument(level = "trace")]
    pub async fn connect(options: ConnectOptions) -> Result<DatabaseConnection, DbErr> {
        let record_stmt_in_spans = options.get_record_stmt_in_spans();
        let mut sqlx_opts = options
            .url
            .parse::<PgConnectOptions>()
            .map_err(sqlx_error_to_conn_err)?;
        use sqlx::ConnectOptions;
        if !options.sqlx_logging {
            sqlx_opts = sqlx_opts.disable_statement_logging();
        } else {
            sqlx_opts = sqlx_opts.log_statements(options.sqlx_logging_level);
            if options.sqlx_slow_statements_logging_level != LevelFilter::Off {
                sqlx_opts = sqlx_opts.log_slow_statements(
                    options.sqlx_slow_statements_logging_level,
                    options.sqlx_slow_statements_logging_threshold,
                );
            }
        }

        if let Some(application_name) = &options.application_name {
            sqlx_opts = sqlx_opts.application_name(application_name);
        }

        if let Some(timeout) = options.statement_timeout {
            sqlx_opts = sqlx_opts.options([("statement_timeout", timeout.as_millis().to_string())]);
        }

        // GaussDB A 模式：关闭 opfusion（SQL bypass 执行器）。真库定档
        // （2026-10-26）：opfusion 对"参数化 PBE + 非 0 result formats"组合
        // 触发内核断言（29003 executor_opfusion.cpp "parameterized pbe
        // num_pformats != 0"），SeaORM 的 prepared statements 正常路径即可
        // 踩中（如迁移器 seaql_migrations 的参数化查询）。标准执行器无此限制。
        sqlx_opts = sqlx_opts.options([("enable_opfusion", "off")]);

        if let Some(f) = &options.pg_opts_fn {
            sqlx_opts = f(sqlx_opts);
        }

        let set_search_path_sql = options.schema_search_path.as_ref().map(|schema| {
            let mut string = "SET search_path = ".to_owned();
            if schema.starts_with('"') {
                write!(&mut string, "{schema}").expect("Infallible");
            } else {
                for (i, schema) in schema.split(',').enumerate() {
                    if i > 0 {
                        write!(&mut string, ",").expect("Infallible");
                    }
                    if schema.starts_with('"') {
                        write!(&mut string, "{schema}").expect("Infallible");
                    } else {
                        write!(&mut string, "\"{schema}\"").expect("Infallible");
                    }
                }
            }
            string
        });

        let lazy = options.connect_lazy;
        let after_connect = options.after_connect.clone();
        let pg_pool_opts_fn = options.pg_pool_opts_fn.clone();
        let pg_before_acquire = options.pg_before_acquire_fn.clone();
        let ping_after_idle = options.test_before_acquire_if_idle_for;
        let mut pool_options = options.sqlx_pool_options();
        pool_options = crate::ConnectOptions::apply_before_acquire::<sqlx::Postgres>(
            pool_options,
            ping_after_idle,
            pg_before_acquire,
        );

        if let Some(sql) = set_search_path_sql {
            pool_options = pool_options.after_connect(move |conn, _| {
                let sql = sql.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(conn, sqlx::AssertSqlSafe(sql))
                        .await
                        .map(|_| ())
                })
            });
        }
        if let Some(f) = &pg_pool_opts_fn {
            pool_options = f(pool_options);
        }
        let pool = if lazy {
            pool_options.connect_lazy_with(sqlx_opts)
        } else {
            pool_options
                .connect_with(sqlx_opts)
                .await
                .map_err(sqlx_error_to_conn_err)?
        };

        let conn: DatabaseConnection =
            DatabaseConnectionType::SqlxGaussDBPoolConnection(SqlxGaussDBPoolConnection {
                pool,
                metric_callback: None,
                record_stmt_in_spans,
            })
            .into();

        if let Some(cb) = after_connect {
            cb(conn.clone()).await?;
        }

        Ok(conn)
    }
}

impl SqlxGaussDBConnector {
    /// Instantiate a sqlx pool connection to a [DatabaseConnection]
    pub fn from_sqlx_postgres_pool(pool: PgPool) -> DatabaseConnection {
        DatabaseConnectionType::SqlxGaussDBPoolConnection(SqlxGaussDBPoolConnection {
            pool,
            metric_callback: None,
            record_stmt_in_spans: true,
        })
        .into()
    }
}

impl SqlxGaussDBPoolConnection {
    /// Execute a [Statement] on a GaussDB backend
    #[instrument(level = "trace", skip(stmt))]
    pub async fn execute(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        debug_print!("{}", stmt);

        let query = sqlx_query(&stmt);
        let mut conn = self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        crate::metric::metric!(self.metric_callback, &stmt, {
            match query.execute(&mut *conn).await {
                Ok(res) => Ok(res.into()),
                Err(err) => Err(sqlx_error_to_exec_err(err)),
            }
        })
    }

    /// Execute an unprepared SQL statement on a GaussDB backend
    #[instrument(level = "trace", skip(sql))]
    pub async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        debug_print!("{}", sql);

        let conn = &mut self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        match conn.execute(sqlx::AssertSqlSafe(sql.to_owned())).await {
            Ok(res) => Ok(res.into()),
            Err(err) => Err(sqlx_error_to_exec_err(err)),
        }
    }

    /// Get one result from a SQL query. Returns [Option::None] if no match was found
    #[instrument(level = "trace", skip(stmt))]
    pub async fn query_one(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        debug_print!("{}", stmt);

        let query = sqlx_query(&stmt);
        let mut conn = self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        crate::metric::metric!(self.metric_callback, &stmt, {
            match query.fetch_one(&mut *conn).await {
                Ok(row) => Ok(Some(gaussdb_query_result(row))),
                Err(err) => match err {
                    sqlx::Error::RowNotFound => Ok(None),
                    _ => Err(sqlx_error_to_query_err(err)),
                },
            }
        })
    }

    /// Get the results of a query returning them as a Vec<[QueryResult]>
    #[instrument(level = "trace", skip(stmt))]
    pub async fn query_all(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        debug_print!("{}", stmt);

        let query = sqlx_query(&stmt);
        let mut conn = self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        crate::metric::metric!(self.metric_callback, &stmt, {
            match query.fetch_all(&mut *conn).await {
                Ok(rows) => Ok(rows.into_iter().map(gaussdb_query_result).collect()),
                Err(err) => Err(sqlx_error_to_query_err(err)),
            }
        })
    }

    /// Stream the results of executing a SQL query
    #[instrument(level = "trace", skip(stmt))]
    #[cfg(feature = "stream")]
    pub async fn stream(&self, stmt: Statement) -> Result<QueryStream, DbErr> {
        debug_print!("{}", stmt);

        let conn = self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        Ok(QueryStream::build(
            stmt,
            crate::InnerConnection::GaussDB(conn),
            self.metric_callback.clone(),
        ))
    }

    /// Bundle a set of SQL statements that execute together.
    #[instrument(level = "trace")]
    pub async fn begin(
        &self,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<DatabaseTransaction, DbErr> {
        let conn = self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        DatabaseTransaction::new_gaussdb(
            conn,
            self.metric_callback.clone(),
            self.record_stmt_in_spans,
            isolation_level,
            access_mode,
        )
        .await
    }

    /// Create a GaussDB transaction
    #[instrument(level = "trace", skip(callback))]
    pub async fn transaction<F, T, E>(
        &self,
        callback: F,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<T, TransactionError<E>>
    where
        F: for<'b> FnOnce(
                &'b DatabaseTransaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'b>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        let conn = self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        let transaction = DatabaseTransaction::new_gaussdb(
            conn,
            self.metric_callback.clone(),
            self.record_stmt_in_spans,
            isolation_level,
            access_mode,
        )
        .await
        .map_err(|e| TransactionError::Connection(e))?;
        transaction.run(callback).await
    }

    pub(crate) fn set_metric_callback<F>(&mut self, callback: F)
    where
        F: Fn(&crate::metric::Info<'_>) + Send + Sync + 'static,
    {
        self.metric_callback = Some(Arc::new(callback));
    }

    /// Checks if a connection to the database is still valid.
    pub async fn ping(&self) -> Result<(), DbErr> {
        let conn = &mut self.pool.acquire().await.map_err(sqlx_conn_acquire_err)?;
        match conn.ping().await {
            Ok(_) => Ok(()),
            Err(err) => Err(sqlx_error_to_conn_err(err)),
        }
    }

    /// Explicitly close the GaussDB connection.
    /// See [`Self::close_by_ref`] for usage with references.
    pub async fn close(self) -> Result<(), DbErr> {
        self.close_by_ref().await
    }

    /// Explicitly close the GaussDB connection
    pub async fn close_by_ref(&self) -> Result<(), DbErr> {
        self.pool.close().await;
        Ok(())
    }
}

pub(crate) fn sqlx_query(stmt: &Statement) -> sqlx::query::Query<'_, Postgres, SqlxValues> {
    let values = stmt
        .values
        .as_ref()
        .map_or(Values(Vec::new()), |values| values.clone());
    sqlx::query_with(sqlx::AssertSqlSafe(stmt.sql.as_str()), SqlxValues(values))
}

/// 将 sqlx `PgRow`（GaussDB 复用 sqlx-postgres 协议栈）包装为标记来自 GaussDB 的
/// [`QueryResult`]。解码层据此对 A 模式报告为 NUMERIC 的列启用整数兜底解码，
/// 见 [`gaussdb_numeric_i128`]。
pub(crate) fn gaussdb_query_result(row: sqlx::postgres::PgRow) -> crate::QueryResult {
    crate::QueryResult {
        row: crate::QueryResultRow::SqlxPostgres(row),
        from_gaussdb: true,
    }
}

/// GaussDB A 模式 NUMERIC 兜底解码（供 `executor/query.rs` 的
/// `try_getable_gaussdb_num!` 在整数解码失败时调用）。
///
/// 仅当行来自 GaussDB 连接（`QueryResult::from_gaussdb`）且列被服务端报告为
/// NUMERIC 时生效：A 模式会把 identity 自增主键的 `INSERT .. RETURNING` 列报告为
/// NUMERIC（探针定档：probe/verification-results.md §六[3]），sqlx 按 INT4/INT8
/// 解码会被类型兼容性检查拒绝。此处把 NUMERIC 值按整数语义解出（二进制
/// base-10000 编码或文本十进制）。返回 `None` 表示兜底不适用，调用方沿用
/// sqlx 原始错误。
pub(crate) fn gaussdb_numeric_i128<I>(
    res: &crate::QueryResult,
    index: I,
) -> Option<Result<i128, crate::TryGetError>>
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
    // 仅对确实被报告为 NUMERIC 的列兜底；INT4/INT8 等列的解码错误原样上抛
    if raw.type_info().name() != "NUMERIC" {
        return None;
    }
    Some(decode_numeric_i128(raw))
}

fn decode_numeric_i128(raw: sqlx::postgres::PgValueRef<'_>) -> Result<i128, crate::TryGetError> {
    match raw.format() {
        sqlx::postgres::PgValueFormat::Text => {
            decode_numeric_text(raw.as_str().map_err(numeric_decode_err)?)
        }
        sqlx::postgres::PgValueFormat::Binary => {
            decode_numeric_binary(raw.as_bytes().map_err(numeric_decode_err)?)
        }
    }
}

fn numeric_decode_err(e: sqlx::error::BoxDynError) -> crate::TryGetError {
    crate::TryGetError::DbErr(crate::DbErr::TryIntoErr {
        from: "NUMERIC",
        into: "i128",
        source: std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        )),
    })
}

/// NUMERIC 文本形式 → 整数语义（小数部分截断）。PG NUMERIC 文本无指数记法。
fn decode_numeric_text(s: &str) -> Result<i128, crate::TryGetError> {
    let err = || {
        crate::TryGetError::DbErr(crate::DbErr::TryIntoErr {
            from: "NUMERIC (text)",
            into: "i128",
            source: std::sync::Arc::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("cannot decode {s:?} as integer"),
            )),
        })
    };
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty() || digits.eq_ignore_ascii_case("NaN") {
        return Err(err());
    }
    let mut value: i128 = digits
        .split('.')
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| err())?;
    if negative {
        value = -value;
    }
    Ok(value)
}

/// NUMERIC 二进制 wire 形式 → 整数语义（小数部分截断）。
/// 布局：int32 ndigits、int32 weight、int16 sign（0x0000 正 / 0x4000 负）、
/// int16 dscale、随后 ndigits 个 int16（base-10000 数字组），全部大端。
fn decode_numeric_binary(bytes: &[u8]) -> Result<i128, crate::TryGetError> {
    let err = || {
        crate::TryGetError::DbErr(crate::DbErr::TryIntoErr {
            from: "NUMERIC (binary)",
            into: "i128",
            source: std::sync::Arc::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "malformed numeric wire data",
            )),
        })
    };
    // 标准 PG NUMERIC binary（GaussDB A 模式实测同构）：
    // i16 ndigits | i16 weight | u16 sign | u16 dscale | i16 digits[ndigits]（base 10_000）
    // （此前的 i32/i32/i16 12 字节头为错误实现，导致 "malformed numeric wire data"。）
    if bytes.len() < 8 {
        return Err(err());
    }
    let ndigits = be_i16(bytes, 0).ok_or_else(err)? as usize;
    let weight = be_i16(bytes, 2).ok_or_else(err)? as i32;
    let sign = be_i16(bytes, 4).ok_or_else(err)? as u16;
    // bytes[6..8] = dscale，整数语义下忽略
    if sign != 0x0000 && sign != 0x4000 {
        return Err(err()); // NaN / ±Inf / 未知符号
    }
    if bytes.len() < 8 + ndigits * 2 {
        return Err(err());
    }
    // weight 上限保证 10_000i128.pow(exp) 不溢出；剩余极端值由 checked 运算兜住
    if weight > 9 {
        return Err(err());
    }
    let mut value: i128 = 0;
    if weight >= 0 {
        for i in 0..ndigits {
            let exp = weight - i as i32;
            if exp < 0 {
                break; // 之后都是小数位，按整数语义截断
            }
            let digit = be_i16(bytes, 8 + i * 2).ok_or_else(err)? as i128;
            value = value
                .checked_add(
                    digit
                        .checked_mul(10_000i128.pow(exp as u32))
                        .ok_or_else(err)?,
                )
                .ok_or_else(err)?;
        }
    }
    Ok(if sign == 0x4000 { -value } else { value })
}

fn be_i16(bytes: &[u8], offset: usize) -> Option<i16> {
    bytes
        .get(offset..offset + 2)
        .map(|b| i16::from_be_bytes([b[0], b[1]]))
}

// ---------------------------------------------------------------------------
// T6：A 模式 Oracle 类型读侧兜底
//
// `clob`/`blob`/`raw`/`date` 在 GaussDB A 模式下是同名私有 udt（探针定档：
// probe/verification-results.md §二、survey/02 §类型定档表），sqlx 按目标类型
// 直读会被严格 OID 兼容性检查拒绝（类比探针 §六[4] floatvector：直读 String
// 被拒，但 RowDescription 能按名识别私有 udt，且 wire 层以文本形式返回）。
// 以下入口与 [`gaussdb_numeric_i128`] 同一模式：由 executor 侧在解码报错后调用，
// 仅当行来自 GaussDB 连接且列类型名为对应私有 udt 时接管解码，否则返回 `None`
// 让调用方沿用 sqlx 原始错误。
// ---------------------------------------------------------------------------

/// A 模式私有 udt 的 `type_info().name()`（探针定档 §二：catalog 同名私有 udt；
/// §六[4] 佐证 sqlx 能按名识别私有 udt，如 floatvector）。
mod oracle_udt {
    pub(super) const CLOB: &str = "clob";
    pub(super) const BLOB: &str = "blob";
    pub(super) const RAW: &str = "raw";
    pub(super) const DATEA: &str = "datea";

    /// 该列类型名是否为 T6 管辖的 Oracle 私有 udt
    pub(super) fn is_private_udt(name: &str) -> bool {
        name == CLOB || name == BLOB || name == RAW || name == DATEA
    }
}

/// 若行来自 GaussDB 且列类型名为 T6 管辖的 Oracle 私有 udt，返回
/// `Some((类型名, 原始列值))`；否则 `None`（不适用）。不检查 NULL（由各入口
/// 自行决定语义）。
fn gaussdb_oracle_udt<'r, I>(
    res: &'r crate::QueryResult,
    index: I,
) -> Option<(String, sqlx::postgres::PgValueRef<'r>)>
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
    let name = raw.type_info().name().to_owned();
    oracle_udt::is_private_udt(&name).then_some((name, raw))
}

/// `date` 列（私有 udt `datea`）的引导性错误（T6 决策：不做原生映射）。
///
/// A 模式 `date` 暴露为私有 udt `datea`，文本输出为 Oracle 形态 `DD-MON-RR`
/// 且拒收 ISO 文本（探针定档 §二、survey/02 §类型定档表）。sea-orm 不从
/// `datea` 解码日期，引导业务改用 `timestamp` 列。
pub(crate) fn gaussdb_datea_error<I>(
    res: &crate::QueryResult,
    index: I,
) -> Option<crate::TryGetError>
where
    I: sqlx::ColumnIndex<sqlx::postgres::PgRow>,
{
    let (name, _) = gaussdb_oracle_udt(res, index)?;
    if name != oracle_udt::DATEA {
        return None;
    }
    Some(crate::TryGetError::DbErr(type_err(
        "date columns in A-mode are reported as the private 'datea' type; use timestamp instead",
    )))
}

/// `clob` 列（私有 udt `clob`）的 text repr 解码。
///
/// 私有 udt 的 wire 文本形式即字符内容本身（类比探针 §六[4] floatvector 返回
/// `'[1,2,3]'` 的行为）。仅 `String` 走此兜底；`blob`/`raw` 语义是字节，
/// 不做 String 兜底。
pub(crate) fn gaussdb_clob_text<I>(
    res: &crate::QueryResult,
    index: I,
) -> Option<Result<String, crate::TryGetError>>
where
    I: sqlx::ColumnIndex<sqlx::postgres::PgRow>,
{
    let (name, raw) = gaussdb_oracle_udt(res, index)?;
    if name != oracle_udt::CLOB {
        return None;
    }
    Some(decode_private_string(raw, &name))
}

/// `blob`/`raw` 列（私有 udt）→ `Vec<u8>`。
///
/// 私有 udt 的 text repr 形态未定档（hex 文本 vs 原始字节）：优先按 hex 解析
/// （GaussDB raw 写入即 hex 语义——survey/02「raw 拒收 bytea，写入需
/// `hextoraw(hex 文本)`」，读侧 hex 对称），非 hex 形态（空、奇数长度、含
/// 非 hex 字符）则原样按字节返回。binary 格式直接取字节。
pub(crate) fn gaussdb_lob_bytes<I>(
    res: &crate::QueryResult,
    index: I,
) -> Option<Result<Vec<u8>, crate::TryGetError>>
where
    I: sqlx::ColumnIndex<sqlx::postgres::PgRow>,
{
    let (name, raw) = gaussdb_oracle_udt(res, index)?;
    if name != oracle_udt::BLOB && name != oracle_udt::RAW {
        return None;
    }
    let idx_dbg = name.clone();
    let from: &'static str = if name == oracle_udt::BLOB {
        "blob"
    } else {
        "raw"
    };
    let format = raw.format();
    let bytes = match decode_private_bytes_value(raw, from, &idx_dbg) {
        Ok(b) => b,
        Err(e) => return Some(Err(e)),
    };
    Some(match format {
        sqlx::postgres::PgValueFormat::Text => Ok(std::str::from_utf8(&bytes)
            .ok()
            .and_then(decode_hex_str)
            .unwrap_or(bytes)),
        sqlx::postgres::PgValueFormat::Binary => Ok(bytes),
    })
}

/// hex 文本（非空、偶数长度、全 hex 字符）→ 字节；否则 `None`。
fn decode_hex_str(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// 私有 udt wire 值 → 字节（text 格式取字符内容，binary 格式取原始字节）。
/// NULL 返回 [`crate::TryGetError::Null`]。`from` 用于错误信息标注来源类型。
fn decode_private_bytes_value(
    raw: sqlx::postgres::PgValueRef<'_>,
    from: &'static str,
    idx_dbg: &str,
) -> Result<Vec<u8>, crate::TryGetError> {
    use sqlx::ValueRef as _;
    if raw.is_null() {
        return Err(crate::TryGetError::Null(idx_dbg.to_owned()));
    }
    match raw.format() {
        sqlx::postgres::PgValueFormat::Text => {
            let s = raw.as_str().map_err(|e| private_decode_err(from, e))?;
            Ok(s.as_bytes().to_vec())
        }
        sqlx::postgres::PgValueFormat::Binary => raw
            .as_bytes()
            .map(<[u8]>::to_vec)
            .map_err(|e| private_decode_err(from, e)),
    }
}

/// 私有 udt wire 值 → `String`（UTF-8）。NULL 返回 [`crate::TryGetError::Null`]。
fn decode_private_string(
    raw: sqlx::postgres::PgValueRef<'_>,
    idx_dbg: &str,
) -> Result<String, crate::TryGetError> {
    String::from_utf8(decode_private_bytes_value(raw, "clob", idx_dbg)?).map_err(|e| {
        crate::TryGetError::DbErr(crate::DbErr::TryIntoErr {
            from: "clob",
            into: "String",
            source: std::sync::Arc::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        })
    })
}

fn private_decode_err(from: &'static str, e: sqlx::error::BoxDynError) -> crate::TryGetError {
    crate::TryGetError::DbErr(crate::DbErr::TryIntoErr {
        from,
        into: "Vec<u8>",
        source: std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        )),
    })
}

impl crate::DatabaseTransaction {
    pub(crate) async fn new_gaussdb(
        inner: PoolConnection<sqlx::Postgres>,
        metric_callback: Option<crate::metric::Callback>,
        record_stmt_in_spans: bool,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<crate::DatabaseTransaction, DbErr> {
        Self::begin(
            Arc::new(Mutex::new(crate::InnerConnection::GaussDB(inner))),
            crate::DbBackend::GaussDB,
            metric_callback,
            record_stmt_in_spans,
            isolation_level,
            access_mode,
            None,
        )
        .await
    }
}
