//! Maps `odbc_api::Error` to a Python exception, mirroring pyodbc's
//! `Error` shape of `args = (sqlstate, message)` closely enough for
//! consumers who sniff the SQLSTATE out of `exc.args[0]`.
use crate::actor::ActorError;
use pyo3::{create_exception, exceptions::PyException, PyErr};

create_exception!(aioodbc, Error, PyException);

/// SQLSTATE of the diagnostic record, if any ("" otherwise).
fn sqlstate(err: &odbc_api::Error) -> String {
    match err {
        odbc_api::Error::Diagnostics { record, .. } => record.state.as_str().to_owned(),
        _ => String::new(),
    }
}

pub fn to_py_err(err: odbc_api::Error) -> PyErr {
    let state = sqlstate(&err);
    let message = err.to_string();
    Error::new_err((state, message))
}

pub fn actor_err_to_py(err: ActorError) -> PyErr {
    match err {
        ActorError::Odbc(e) => to_py_err(e),
        ActorError::Stale => Error::new_err((
            "",
            "cursor invalidated: another statement ran on this connection",
        )),
        ActorError::Closed => Error::new_err(("", "connection is closed")),
    }
}
