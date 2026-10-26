use super::SchemaQueryBuilder;
use crate::rusqlite_types::RusqliteRow;
use sea_query::{Func, Query, SelectStatement};

#[derive(sea_query::Iden)]
enum MysqlFunc {
    Version,
}

#[derive(Debug, Default)]
pub struct VersionQueryResult {
    pub version: String,
}

impl SchemaQueryBuilder {
    pub fn query_version(&self) -> SelectStatement {
        Query::select().expr(Func::cust(MysqlFunc::Version)).take()
    }
}

#[cfg(feature = "sqlx-mysql")]
impl From<RusqliteRow> for VersionQueryResult {
    fn from(row: RusqliteRow) -> Self {
        use crate::mysql::discovery::GetMySqlValue;
        let row = row.mysql();
        Self {
            version: row.get_string(0),
        }
    }
}

#[cfg(not(feature = "sqlx-mysql"))]
impl From<RusqliteRow> for VersionQueryResult {
    fn from(_: RusqliteRow) -> Self {
        Self::default()
    }
}
