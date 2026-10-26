use sea_query::{Expr, ExprTrait, SelectStatement};

use super::def::{IndexInfo, Schema, TableDef};
pub use super::error::DiscoveryResult;
use super::executor::{Executor, IntoExecutor};
use super::query::SqliteMaster;
use crate::{Connection, rusqlite_types::RusqliteConnection};

/// Performs all the methods for schema discovery of a SQLite database
pub struct SchemaDiscovery {
    exec: Executor,
}

impl SchemaDiscovery {
    /// Discover schema from a Rusqlite pool
    pub fn new(pool: RusqliteConnection) -> Self {
        Self {
            exec: pool.into_executor(),
        }
    }

    pub fn discover(&self) -> DiscoveryResult<Schema> {
        Self::discover_with(&self.exec)
    }
}

impl SchemaDiscovery {
    /// Discover all the tables in a SQLite database
    pub fn discover_with<C: Connection>(conn: &C) -> DiscoveryResult<Schema> {
        let get_tables = SelectStatement::new()
            .column("name")
            .from(SqliteMaster)
            .and_where(Expr::col("type").eq("table"))
            .and_where(Expr::col("name").ne("sqlite_sequence"))
            .to_owned();

        let mut tables = Vec::new();
        for row in conn.query_all(get_tables)? {
            let mut table: TableDef = row.into();
            table.pk_is_autoincrement(conn)?;
            table.get_foreign_keys(conn)?;
            table.get_column_info(conn)?;
            table.get_constraints(conn)?;
            tables.push(table);
        }

        let indexes = Self::discover_indexes(conn)?;

        Ok(Schema { tables, indexes })
    }

    /// Discover table indexes
    fn discover_indexes<C: Connection>(conn: &C) -> DiscoveryResult<Vec<IndexInfo>> {
        let get_tables = SelectStatement::new()
            .column("name")
            .from(SqliteMaster)
            .and_where(Expr::col("type").eq("table"))
            .and_where(Expr::col("name").ne("sqlite_sequence"))
            .to_owned();

        let mut discovered_indexes = Vec::new();
        let rows = conn.query_all(get_tables)?;
        for row in rows {
            let mut table: TableDef = row.into();
            table.get_indexes(conn)?;
            discovered_indexes.append(&mut table.indexes);
        }

        Ok(discovered_indexes)
    }
}
