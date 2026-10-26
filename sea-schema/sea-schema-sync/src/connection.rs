use crate::rusqlite_types::{RusqliteError, RusqliteRow};
use sea_query::SelectStatement;

pub trait Connection: Sized {
    fn query_all(&self, select: SelectStatement) -> Result<Vec<RusqliteRow>, RusqliteError>;

    fn query_all_raw(&self, sql: String) -> Result<Vec<RusqliteRow>, RusqliteError>;
}
