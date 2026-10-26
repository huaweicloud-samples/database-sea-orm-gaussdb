pub use rusqlite::{Connection as RusqliteConnection, Error as RusqliteError};
use rusqlite::{
    Error, Row as RawRow,
    types::{FromSql, Value},
};
use sea_query_rusqlite::rusqlite;

pub type Row = RusqliteRow;

#[derive(Debug)]
pub struct RusqliteRow {
    pub values: Vec<Value>,
}

impl RusqliteRow {
    pub fn from_row(row: &RawRow) -> Self {
        let mut values = Vec::new();

        let mut i = 0;
        loop {
            let v: Value = match row.get(i) {
                Ok(v) => v,
                Err(Error::InvalidColumnIndex(_)) => break,
                Err(err) => panic!("{err:?}"),
            };
            values.push(v);
            i += 1;
        }

        Self { values }
    }

    pub fn get<T: FromSql>(&self, idx: usize) -> T {
        let value = &self.values[idx];
        FromSql::column_result(value.into()).unwrap_or_else(|e| panic!("{e:?}: actual: {value:?}"))
    }

    pub fn sqlite(self) -> Self {
        self
    }
}

#[cfg(feature = "rusqlite")]
pub fn connect_sqlite(url: &str) -> Result<RusqliteConnection, RusqliteError> {
    RusqliteConnection::open(
        url.trim_start_matches("sqlite://")
            .trim_start_matches("sqlite:"),
    )
}

#[cfg(feature = "rusqlite")]
pub fn execute_sqlite(conn: &RusqliteConnection, sql: &str) -> Result<(), RusqliteError> {
    conn.execute_batch(&sql)?;
    Ok(())
}
