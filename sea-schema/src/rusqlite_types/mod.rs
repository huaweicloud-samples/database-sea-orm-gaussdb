#[cfg(feature = "rusqlite")]
mod real;
#[cfg(feature = "rusqlite")]
pub use real::*;

#[cfg(all(not(feature = "rusqlite"), not(feature = "sqlx-sqlite")))]
mod mock;
#[cfg(all(not(feature = "rusqlite"), not(feature = "sqlx-sqlite")))]
pub use mock::*;
