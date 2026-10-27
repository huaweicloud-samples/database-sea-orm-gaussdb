use crate::{IndexCreateStatement, IndexType, IntoIden};

/// GaussDB-specific extensions for [`IndexCreateStatement`].
///
/// Only [`GaussDBQueryBuilder`](crate::GaussDBQueryBuilder) renders the state
/// set by this trait; every other backend ignores it.
///
/// # Examples
///
/// A GaussDB vector index (A-mode, real-DB 定档语法
/// `CREATE INDEX .. USING GSIVFFLAT (<col> <op>) WITH (IVF_NLIST = <n>)`):
///
/// ```
/// use sea_query::{extension::gaussdb::GaussDBIndexExt, *};
///
/// let index = Index::create()
///     .name("idx-emb")
///     .table("item")
///     .col(IndexColumn::from(Alias::new("embedding")).with_operator_class("L2"))
///     .gaussdb_index_method("GSIVFFLAT")
///     .gaussdb_index_with("IVF_NLIST", 16)
///     .to_owned();
///
/// assert_eq!(
///     index.to_string(GaussDBQueryBuilder),
///     r#"CREATE INDEX "idx-emb" ON "item" USING GSIVFFLAT ("embedding" L2) WITH (IVF_NLIST = 16)"#
/// );
/// ```
pub trait GaussDBIndexExt {
    /// Set the index access method, rendered raw after `USING`.
    ///
    /// This is the GaussDB-flavoured escape hatch over
    /// [`IndexType::Custom`], for access methods other backends don't know
    /// about, e.g. the A-mode vector index methods `GSIVFFLAT` / `GSDISKANN`.
    fn gaussdb_index_method<I>(&mut self, method: I) -> &mut Self
    where
        I: IntoIden;

    /// Append a `WITH (key = value)` index storage parameter.
    ///
    /// Both `key` and `value` are rendered raw (no quoting), e.g.
    /// `.gaussdb_index_with("IVF_NLIST", 16)` renders `WITH (IVF_NLIST = 16)`.
    /// Calling it multiple times accumulates parameters separated by `, `.
    fn gaussdb_index_with<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: Into<String>,
        V: ToString;
}

impl GaussDBIndexExt for IndexCreateStatement {
    fn gaussdb_index_method<I>(&mut self, method: I) -> &mut Self
    where
        I: IntoIden,
    {
        self.index_type(IndexType::Custom(method.into_iden()))
    }

    fn gaussdb_index_with<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: Into<String>,
        V: ToString,
    {
        self.index_with.push((key.into(), value.to_string()));
        self
    }
}
