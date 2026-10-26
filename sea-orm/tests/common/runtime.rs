#[cfg(feature = "runtime-async-std")]
#[macro_export]
macro_rules! block_on {
    ($($expr:tt)*) => {
        ::async_std::task::block_on( $($expr)* )
    };
}

#[cfg(feature = "runtime-tokio")]
#[macro_export]
macro_rules! block_on {
    ($($expr:tt)*) => {
        ::tokio::runtime::Runtime::new()
            .unwrap()
            .block_on( $($expr)* )
    };
}

// Fallback for feature combinations without a dedicated runtime feature
// (e.g. `--features sqlx-postgres` or `--features gaussdb`): the dev-dependency
// `smol` is always available when compiling tests.
#[cfg(all(not(feature = "runtime-async-std"), not(feature = "runtime-tokio")))]
#[macro_export]
macro_rules! block_on {
    ($($expr:tt)*) => {
        ::smol::block_on( $($expr)* )
    };
}
