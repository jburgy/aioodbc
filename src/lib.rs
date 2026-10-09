mod actor;
mod cell;
mod connection;
mod ctxmgr;
mod cursor;
mod env;
mod error;
mod pool;
mod pyconv;

use pyo3::prelude::*;

#[pymodule]
fn _aioodbc(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("Error", m.py().get_type::<error::Error>())?;
    m.add_class::<connection::Connection>()?;
    m.add_class::<cursor::Cursor>()?;
    m.add_class::<pool::Pool>()?;
    m.add_function(wrap_pyfunction!(connection::connect, m)?)?;
    m.add_function(wrap_pyfunction!(pool::create_pool, m)?)?;
    Ok(())
}
