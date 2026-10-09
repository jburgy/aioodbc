//! Process-wide ODBC environment. One per process is all ODBC needs
//! (and all the driver manager typically tolerates); every connection
//! borrows it for its `'static` lifetime.
use odbc_api::Environment;
use std::sync::OnceLock;

pub fn environment() -> &'static Environment {
    static ENV: OnceLock<Environment> = OnceLock::new();
    ENV.get_or_init(|| Environment::new().expect("failed to allocate ODBC environment"))
}
