use crate::{
    Connection,
    rusqlite_types::{MySqlPool, RusqliteRow},
};
use sea_query::{MysqlQueryBuilder, SelectStatement};

use crate::{debug_print, rusqlite_types::RusqliteError};

#[allow(dead_code)]
pub struct Executor {}

pub trait IntoExecutor {
    fn into_executor(self) -> Executor;
}

impl IntoExecutor for MySqlPool {
    fn into_executor(self) -> Executor {
        Executor {}
    }
}

impl Connection for Executor {
    fn query_all(&self, select: SelectStatement) -> Result<Vec<RusqliteRow>, RusqliteError> {
        let (_sql, _values) = select.build(MysqlQueryBuilder);
        debug_print!("{}, {:?}", _sql, _values);

        panic!("This is a mock Executor");
    }

    fn query_all_raw(&self, _sql: String) -> Result<Vec<RusqliteRow>, RusqliteError> {
        debug_print!("{}", _sql);

        panic!("This is a mock Executor");
    }
}
