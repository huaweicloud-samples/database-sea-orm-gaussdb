use crate::postgres::def::{ColumnDefault, ColumnInfo, Type};
use sea_query::{
    Alias, ColumnDef, ColumnType, DynIden, Expr, IntoIden, Keyword, PgInterval, RcOrArc,
    SimpleExpr, StringLen,
};
use std::convert::TryFrom;

impl ColumnInfo {
    pub fn write(&self) -> ColumnDef {
        let mut col_info = self.clone();
        if let Some(ColumnDefault::AutoIncrement(_)) = &self.default {
            col_info = Self::convert_to_serial(col_info);
        }
        if self.is_identity_column() {
            // Must happen before `write_col_type` so that a GaussDB A-mode
            // identity column, reported as `numeric` by the catalog, is written
            // as an integer auto-increment column (serial semantics).
            col_info = Self::convert_to_serial(col_info);
        }
        let col_type = col_info.write_col_type();
        let mut col_def = ColumnDef::new_with_type(Alias::new(self.name.as_str()), col_type);
        if matches!(
            col_info.col_type,
            Type::SmallSerial | Type::Serial | Type::BigSerial
        ) {
            col_def.auto_increment();
        }
        if self.not_null.is_some() {
            col_def.not_null();
        }
        if let Some(default) = &self.default {
            if let Some(default_expr) = default.write() {
                col_def.default(default_expr);
            }
        }
        col_def
    }

    /// Whether the column is an identity column, either per
    /// `information_schema.columns.is_identity`, or per
    /// `pg_attribute.attidentity` (`'a'` / `'d'`) — the authoritative signal on
    /// GaussDB A-compatibility mode, where identity columns are reported as
    /// `numeric` and `is_identity` is not reliable.
    pub fn is_identity_column(&self) -> bool {
        self.is_identity || matches!(self.att_identity.as_deref(), Some("a" | "d"))
    }

    fn convert_to_serial(mut col_info: ColumnInfo) -> ColumnInfo {
        match col_info.col_type {
            Type::SmallInt => {
                col_info.col_type = Type::SmallSerial;
            }
            Type::Integer => {
                col_info.col_type = Type::Serial;
            }
            Type::BigInt => {
                col_info.col_type = Type::BigSerial;
            }
            // GaussDB A: identity columns are reported as `numeric` by the
            // catalog even though they are integer columns semantically
            // (discovered `Type::Numeric`, see T4/T7 findings).
            Type::Numeric(_) => {
                col_info.col_type = Type::Serial;
            }
            _ => {}
        };
        col_info
    }

    pub fn write_col_type(&self) -> ColumnType {
        fn write_type(col_type: &Type) -> ColumnType {
            match col_type {
                Type::SmallInt => ColumnType::SmallInteger,
                Type::Integer => ColumnType::Integer,
                Type::BigInt => ColumnType::BigInteger,
                Type::Decimal(num_attr) | Type::Numeric(num_attr) => {
                    match (num_attr.precision, num_attr.scale) {
                        (None, None) => ColumnType::Decimal(None),
                        (precision, scale) => ColumnType::Decimal(Some((
                            precision.unwrap_or(0).into(),
                            scale.unwrap_or(0).into(),
                        ))),
                    }
                }
                Type::Real => ColumnType::Float,
                Type::DoublePrecision => ColumnType::Double,
                Type::SmallSerial => ColumnType::SmallInteger,
                Type::Serial => ColumnType::Integer,
                Type::BigSerial => ColumnType::BigInteger,
                Type::Money => ColumnType::Money(None),
                Type::Varchar(string_attr) => match string_attr.length {
                    Some(length) => ColumnType::String(StringLen::N(length.into())),
                    None => ColumnType::String(StringLen::None),
                },
                Type::Char(string_attr) => ColumnType::Char(string_attr.length.map(Into::into)),
                Type::Text => ColumnType::Text,
                Type::Bytea => ColumnType::VarBinary(StringLen::None),
                // The SQL standard requires that writing just timestamp be equivalent to timestamp without time zone,
                // and PostgreSQL honors that behavior. (https://www.postgresql.org/docs/current/datatype-datetime.html)
                Type::Timestamp(_) => ColumnType::DateTime,
                Type::TimestampWithTimeZone(_) => ColumnType::TimestampWithTimeZone,
                Type::Date => ColumnType::Date,
                Type::Time(_) => ColumnType::Time,
                Type::TimeWithTimeZone(_) => ColumnType::Time,
                Type::Interval(interval_attr) => {
                    let field = match &interval_attr.field {
                        Some(field) => PgInterval::try_from(field).ok(),
                        None => None,
                    };
                    let precision = interval_attr.precision.map(Into::into);
                    ColumnType::Interval(field, precision)
                }
                Type::Boolean => ColumnType::Boolean,
                Type::Point => ColumnType::Custom("point".into_iden()),
                Type::Line => ColumnType::Custom("line".into_iden()),
                Type::Lseg => ColumnType::Custom("lseg".into_iden()),
                Type::Box => ColumnType::Custom("box".into_iden()),
                Type::Path => ColumnType::Custom("path".into_iden()),
                Type::Polygon => ColumnType::Custom("polygon".into_iden()),
                Type::Circle => ColumnType::Custom("circle".into_iden()),
                Type::Cidr => ColumnType::Cidr,
                Type::Inet => ColumnType::Inet,
                Type::MacAddr => ColumnType::Custom("macaddr".into_iden()),
                Type::MacAddr8 => ColumnType::Custom("macaddr8".into_iden()),
                Type::Bit(bit_attr) => ColumnType::Bit(bit_attr.length.map(Into::into)),
                Type::VarBit(bit_attr) => ColumnType::VarBit(bit_attr.length.unwrap_or(1).into()),
                Type::TsVector => ColumnType::Custom("tsvector".into_iden()),
                Type::TsQuery => ColumnType::Custom("tsquery".into_iden()),
                Type::Uuid => ColumnType::Uuid,
                Type::Xml => ColumnType::Custom("xml".into_iden()),
                Type::Json => ColumnType::Json,
                Type::JsonBinary => ColumnType::JsonBinary,
                Type::Int4Range => ColumnType::Custom("int4range".into_iden()),
                Type::Int8Range => ColumnType::Custom("int8range".into_iden()),
                Type::NumRange => ColumnType::Custom("numrange".into_iden()),
                Type::TsRange => ColumnType::Custom("tsrange".into_iden()),
                Type::TsTzRange => ColumnType::Custom("tstzrange".into_iden()),
                Type::DateRange => ColumnType::Custom("daterange".into_iden()),
                Type::PgLsn => ColumnType::Custom("pg_lsn".into_iden()),
                #[cfg(feature = "postgres-vector")]
                Type::Vector(vector_attr) => match vector_attr.length {
                    Some(length) => ColumnType::Vector(Some(length)),
                    None => ColumnType::Vector(None),
                },
                Type::Unknown(s) => ColumnType::Custom(Alias::new(s).into_iden()),
                Type::Enum(enum_def) => {
                    let name = Alias::new(&enum_def.typename).into_iden();
                    let variants: Vec<DynIden> = enum_def
                        .values
                        .iter()
                        .map(|variant| Alias::new(variant).into_iden())
                        .collect();
                    ColumnType::Enum { name, variants }
                }
                Type::Array(array_def) => ColumnType::Array(RcOrArc::new(write_type(
                    array_def.col_type.as_ref().expect("Array type not defined"),
                ))),
            }
        }
        write_type(&self.col_type)
    }
}

impl ColumnDefault {
    /// Convert to a [SimpleExpr] for use with `col_def.default()`.
    /// Returns `None` for [ColumnDefault::AutoIncrement] since those are handled
    /// via SERIAL type conversion instead.
    pub fn write(&self) -> Option<SimpleExpr> {
        match self {
            ColumnDefault::Int(int) => Some((*int).into()),
            ColumnDefault::Real(real) => Some((*real).into()),
            ColumnDefault::String(string) => Some(string.into()),
            ColumnDefault::Bool(val) => Some(Expr::val(*val)),
            ColumnDefault::CurrentTimestamp => Some(Keyword::CurrentTimestamp.into()),
            ColumnDefault::AutoIncrement(_) => None,
            ColumnDefault::Expression(expr) => Some(Expr::cust(expr.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postgres::def::{ArbitraryPrecisionNumericAttr, NotNull};

    fn numeric_column(is_identity: bool, att_identity: Option<&str>) -> ColumnInfo {
        ColumnInfo {
            name: "id".to_owned(),
            col_type: Type::Numeric(ArbitraryPrecisionNumericAttr {
                precision: Some(10),
                scale: Some(0),
            }),
            default: None,
            generated: None,
            not_null: Some(NotNull),
            is_identity,
            att_identity: att_identity.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn att_identity_numeric_writes_integer_auto_increment() {
        // GaussDB A: the catalog reports an identity primary key column as
        // `numeric`; the identity signal only shows up in
        // `pg_attribute.attidentity` ('d' = BY DEFAULT).
        let col_def = numeric_column(false, Some("d")).write();
        assert_eq!(col_def.get_column_type(), Some(&ColumnType::Integer));
        assert!(col_def.get_column_spec().auto_increment);
    }

    #[test]
    fn att_identity_always_numeric_writes_integer_auto_increment() {
        let col_def = numeric_column(false, Some("a")).write();
        assert_eq!(col_def.get_column_type(), Some(&ColumnType::Integer));
        assert!(col_def.get_column_spec().auto_increment);
    }

    #[test]
    fn plain_numeric_stays_decimal() {
        // A real decimal/numeric column (no identity marker) must keep the
        // decimal mapping, including its precision and scale.
        let col_def = numeric_column(false, None).write();
        assert_eq!(
            col_def.get_column_type(),
            Some(&ColumnType::Decimal(Some((10, 0))))
        );
        assert!(!col_def.get_column_spec().auto_increment);
    }

    #[test]
    fn information_schema_identity_integer_still_writes_auto_increment() {
        // Standard PostgreSQL behavior is preserved: `is_identity` from
        // `information_schema.columns` alone still yields auto-increment.
        let mut col = numeric_column(true, None);
        col.col_type = Type::Integer;
        let col_def = col.write();
        assert_eq!(col_def.get_column_type(), Some(&ColumnType::Integer));
        assert!(col_def.get_column_spec().auto_increment);
    }

    #[test]
    fn plain_integer_stays_plain() {
        // A non-identity integer column must not gain auto-increment.
        let mut col = numeric_column(false, None);
        col.col_type = Type::Integer;
        let col_def = col.write();
        assert_eq!(col_def.get_column_type(), Some(&ColumnType::Integer));
        assert!(!col_def.get_column_spec().auto_increment);
    }
}
