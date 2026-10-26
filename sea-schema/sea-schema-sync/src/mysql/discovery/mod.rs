//! To query & parse MySQL's INFORMATION_SCHEMA and construct a [`Schema`]

use crate::debug_print;
use crate::mysql::def::*;
use crate::mysql::parser::{parse_foreign_key_query_results, parse_index_query_results};
use crate::mysql::query::{
    ColumnQueryResult, ForeignKeyQueryResult, IndexQueryResult, SchemaQueryBuilder,
    TableQueryResult, VersionQueryResult,
};
use crate::{
    Connection,
    rusqlite_types::{MySqlPool, RusqliteError},
};
use sea_query::{Alias, DynIden, IntoIden, SeaRc};

mod executor;
pub use executor::*;

pub struct SchemaDiscovery {
    pub query: SchemaQueryBuilder,
    pub schema: DynIden,
    exec: Option<Executor>,
}

impl SchemaDiscovery {
    /// Discover schema from a Rusqlite pool
    pub fn new(pool: MySqlPool, schema: &str) -> Self {
        SchemaDiscovery {
            query: SchemaQueryBuilder::default(),
            schema: Alias::new(schema).into_iden(),
            exec: Some(pool.into_executor()),
        }
    }

    #[doc(hidden)]
    pub fn new_no_exec(schema: &str) -> Self {
        Self {
            query: SchemaQueryBuilder::default(),
            schema: Alias::new(schema).into_iden(),
            exec: None,
        }
    }

    pub fn discover(mut self) -> Result<Schema, RusqliteError> {
        let conn = match self.exec.take() {
            Some(exec) => exec,
            None => return Err(RusqliteError::PoolClosed),
        };
        self.discover_with(&conn)
    }

    #[doc(hidden)]
    pub fn discover_with<C: Connection>(mut self, conn: &C) -> Result<Schema, RusqliteError> {
        self.query = SchemaQueryBuilder::new(self.discover_system_with(conn)?);
        let mut tables = Vec::new();

        for table in self.discover_tables_with(conn)? {
            tables.push(self.discover_table_with(conn, table)?);
        }

        Ok(Schema {
            schema: self.schema.to_string(),
            system: self.query.system,
            tables,
        })
    }

    #[doc(hidden)]
    fn discover_system_with<C: Connection>(&self, conn: &C) -> Result<SystemInfo, RusqliteError> {
        let rows = conn.query_all(self.query.query_version())?;

        #[allow(clippy::never_loop)]
        for row in rows {
            let result: VersionQueryResult = row.into();
            debug_print!("{:?}", result);
            let version = result.parse();
            debug_print!("{:?}", version);
            return Ok(version);
        }
        Err(RusqliteError::RowNotFound)
    }

    fn discover_tables_with<C: Connection>(
        &self,
        conn: &C,
    ) -> Result<Vec<TableInfo>, RusqliteError> {
        let rows = conn.query_all(self.query.query_tables(self.schema.clone()))?;

        let tables: Vec<TableInfo> = rows
            .into_iter()
            .map(|row| {
                let result: TableQueryResult = row.into();
                debug_print!("{:?}", result);
                let table = result.parse();
                debug_print!("{:?}", table);
                table
            })
            .collect();

        Ok(tables)
    }

    fn discover_table_with<C: Connection>(
        &self,
        conn: &C,
        info: TableInfo,
    ) -> Result<TableDef, RusqliteError> {
        let table = SeaRc::new(Alias::new(info.name.as_str()));
        let columns = self.discover_columns_with(
            conn,
            self.schema.clone(),
            table.clone(),
            &self.query.system,
        )?;
        let indexes = self.discover_indexes_with(conn, self.schema.clone(), table.clone())?;
        let foreign_keys =
            self.discover_foreign_keys_with(conn, self.schema.clone(), table.clone())?;

        Ok(TableDef {
            info,
            columns,
            indexes,
            foreign_keys,
        })
    }

    fn discover_columns_with<C: Connection>(
        &self,
        conn: &C,
        schema: DynIden,
        table: DynIden,
        system: &SystemInfo,
    ) -> Result<Vec<ColumnInfo>, RusqliteError> {
        let rows = conn.query_all(self.query.query_columns(schema.clone(), table.clone()))?;

        let columns = rows
            .into_iter()
            .map(|row| {
                let result: ColumnQueryResult = row.into();
                debug_print!("{:?}", result);
                let column = result.parse(system);
                debug_print!("{:?}", column);
                column
            })
            .collect::<Vec<_>>();

        Ok(columns)
    }

    fn discover_indexes_with<C: Connection>(
        &self,
        conn: &C,
        schema: DynIden,
        table: DynIden,
    ) -> Result<Vec<IndexInfo>, RusqliteError> {
        let rows = conn.query_all(self.query.query_indexes(schema.clone(), table.clone()))?;

        let results = rows.into_iter().map(|row| {
            let result: IndexQueryResult = row.into();
            debug_print!("{:?}", result);
            result
        });

        Ok(parse_index_query_results(Box::new(results))
            .inspect(|_index| {
                debug_print!("{:?}", _index);
            })
            .collect())
    }

    fn discover_foreign_keys_with<C: Connection>(
        &self,
        conn: &C,
        schema: DynIden,
        table: DynIden,
    ) -> Result<Vec<ForeignKeyInfo>, RusqliteError> {
        let rows = conn.query_all(self.query.query_foreign_key(schema.clone(), table.clone()))?;

        let results = rows.into_iter().map(|row| {
            let result: ForeignKeyQueryResult = row.into();
            debug_print!("{:?}", result);
            result
        });

        Ok(parse_foreign_key_query_results(Box::new(results))
            .inspect(|_index| {
                debug_print!("{:?}", _index);
            })
            .collect())
    }
}
