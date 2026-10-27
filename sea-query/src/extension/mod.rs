//! Engine specific SQL features.

#[cfg(feature = "backend-gaussdb")]
#[cfg_attr(docsrs, doc(cfg(feature = "backend-gaussdb")))]
pub mod gaussdb;

#[cfg(feature = "backend-mysql")]
#[cfg_attr(docsrs, doc(cfg(feature = "backend-mysql")))]
pub mod mysql;

#[cfg(feature = "backend-postgres")]
#[cfg_attr(docsrs, doc(cfg(feature = "backend-postgres")))]
pub mod postgres;

#[cfg(feature = "backend-sqlite")]
#[cfg_attr(docsrs, doc(cfg(feature = "backend-sqlite")))]
pub mod sqlite;
