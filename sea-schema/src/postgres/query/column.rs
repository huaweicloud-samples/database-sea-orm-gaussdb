use super::{InformationSchema, SchemaQueryBuilder};
use crate::sqlx_types::SqlxRow;
use sea_query::{BinOper, DynIden, Expr, ExprTrait, Iden, Query, SelectStatement};

#[derive(Debug, sea_query::Iden)]
/// Ref: https://www.postgresql.org/docs/13/infoschema-columns.html
pub enum ColumnsField {
    TableCatalog,
    TableSchema,
    TableName,
    ColumnName,
    OrdinalPosition,
    ColumnDefault,
    IsNullable,
    DataType,
    CharacterMaximumLength,
    CharacterOctetLength,
    NumericPrecision,
    NumericPrecisionRadix,
    NumericScale,
    DatetimePrecision,
    IntervalType,
    IntervalPrecision,
    CollationCatalog,
    CollationSchema,
    CollationName,
    DomainCatalog,
    DomainSchema,
    DomainName,
    UdtCatalog,
    UdtSchema,
    UdtName,
    DtdIdentifier,
    IsIdentity,
    IdentityGeneration,
    IdentityStart,
    IdentityIncrement,
    IdentityMaximum,
    IdentityMinimum,
    IdentityCycle,
    IsGenerated,
    GenerationExpression,
    IsUpdatable,
}

#[derive(Debug, Default)]
pub struct ColumnQueryResult {
    pub column_name: String,
    pub column_type: String,
    pub column_default: Option<String>,
    pub column_generated: Option<String>,
    pub is_nullable: String,
    pub is_identity: String,

    // Declared or implicit parameters of numeric types; null for other data types
    pub numeric_precision: Option<i32>,
    pub numeric_precision_radix: Option<i32>,
    pub numeric_scale: Option<i32>,

    pub character_maximum_length: Option<i32>,
    pub character_octet_length: Option<i32>,

    pub datetime_precision: Option<i32>,

    pub interval_type: Option<String>,
    pub interval_precision: Option<i32>,

    pub udt_name: Option<String>,
    pub udt_name_regtype: Option<String>,

    /// `pg_attribute.attidentity` of the underlying column: `Some("a")` (ALWAYS),
    /// `Some("d")` (BY DEFAULT) or `None`/`Some("")` for non-identity columns.
    pub att_identity: Option<String>,
}

impl SchemaQueryBuilder {
    pub fn query_columns(&self, schema: DynIden, table: DynIden) -> SelectStatement {
        Query::select()
            .columns([
                ColumnsField::ColumnName,
                ColumnsField::DataType,
                ColumnsField::ColumnDefault,
                ColumnsField::GenerationExpression,
                ColumnsField::IsNullable,
                ColumnsField::IsIdentity,
                ColumnsField::NumericPrecision,
                ColumnsField::NumericPrecisionRadix,
                ColumnsField::NumericScale,
                ColumnsField::CharacterMaximumLength,
                ColumnsField::CharacterOctetLength,
                ColumnsField::DatetimePrecision,
                ColumnsField::IntervalType,
                ColumnsField::IntervalPrecision,
                ColumnsField::UdtName,
            ])
            .expr(
                // The double quotes are required to correctly handle user types containing
                // upper case letters.
                Expr::expr(Expr::cust("CONCAT('\"', udt_name, '\"')::regtype").cast_as(Text))
                    .binary(BinOper::As, Expr::col(UdtNameRegtype)),
            )
            .expr(
                // `pg_attribute.attidentity` of the underlying column ('a' / 'd' / NULL);
                // see `ATT_IDENTITY_SUBQUERY` for why this is fetched in addition to
                // `information_schema.columns.is_identity`.
                Expr::expr(Expr::cust(ATT_IDENTITY_SUBQUERY))
                    .binary(BinOper::As, Expr::col(AttIdentity)),
            )
            .from((InformationSchema::Schema, InformationSchema::Columns))
            .and_where(Expr::col(ColumnsField::TableSchema).eq(schema.to_string()))
            .and_where(Expr::col(ColumnsField::TableName).eq(table.to_string()))
            .take()
    }
}

#[cfg(feature = "sqlx-postgres")]
impl From<SqlxRow> for ColumnQueryResult {
    fn from(row: SqlxRow) -> Self {
        use crate::sqlx_types::Row;
        let row = row.postgres();
        Self {
            column_name: row.get(0),
            column_type: row.get(1),
            column_default: row.get(2),
            column_generated: row.get(3),
            is_nullable: row.get(4),
            is_identity: row.get(5),
            numeric_precision: row.get(6),
            numeric_precision_radix: row.get(7),
            numeric_scale: row.get(8),
            character_maximum_length: row.get(9),
            character_octet_length: row.get(10),
            datetime_precision: row.get(11),
            interval_type: row.get(12),
            interval_precision: row.get(13),
            udt_name: row.get(14),
            udt_name_regtype: row.get(15),
            att_identity: row.get(16),
        }
    }
}

#[cfg(not(feature = "sqlx-postgres"))]
impl From<SqlxRow> for ColumnQueryResult {
    fn from(_: SqlxRow) -> Self {
        Self::default()
    }
}

#[derive(Iden)]
struct Text;
#[derive(Iden)]
struct UdtNameRegtype;
#[derive(Iden)]
struct AttIdentity;

/// On GaussDB A-compatibility mode, an identity column is reported as
/// `numeric` by `information_schema.columns` (and `is_identity` is not
/// reliable), so `pg_attribute.attidentity` is the authoritative source.
/// This correlated scalar sub-lookup joins the catalog tables (the same
/// ones the official `information_schema` view definitions use) via the
/// schema-qualified table name and the column name, restricted to regular
/// columns (`attnum > 0`) so a user column shadowing a system column name
/// cannot produce two rows.
const ATT_IDENTITY_SUBQUERY: &str = "(SELECT a.attidentity::text \
     FROM pg_catalog.pg_attribute a \
     JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = table_schema \
     AND c.relname = table_name \
     AND a.attname = column_name \
     AND a.attnum > 0)";
