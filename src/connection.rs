use crate::actor::ActorHandle;
use crate::ctxmgr::wrap_context_manager;
use crate::cursor::Cursor;
use crate::error::{actor_err_to_py, to_py_err};
use pyo3::prelude::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Connections should only be created by `aioodbc.connect`.
#[pyclass]
pub struct Connection {
    pub(crate) handle: ActorHandle,
    autocommit: Arc<AtomicBool>,
}

#[pymethods]
impl Connection {
    #[getter]
    fn closed(&self) -> bool {
        self.handle.is_closed()
    }

    #[getter]
    fn autocommit(&self) -> bool {
        self.autocommit.load(Ordering::Relaxed)
    }

    /// Mirrors pyodbc's own `autocommit` setter: a quick, synchronous
    /// `SQLSetConnectAttr` call with no async ODBC equivalent, so -- just
    /// like upstream aioodbc, which never routes this particular setter
    /// through an executor either -- it briefly blocks the calling thread.
    #[setter]
    fn set_autocommit(&self, value: bool) -> PyResult<()> {
        pyo3_async_runtimes::tokio::get_runtime()
            .block_on(self.handle.set_autocommit(value))
            .map_err(actor_err_to_py)?;
        self.autocommit.store(value, Ordering::Relaxed);
        Ok(())
    }

    /// Returns a new `Cursor`, usable both as `cur = await conn.cursor()`
    /// and `async with conn.cursor() as cur:`.
    fn cursor<'p>(slf: Py<Self>, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = slf.borrow(py).handle.clone();
        let coro =
            pyo3_async_runtimes::tokio::future_into_py(
                py,
                async move { Ok(Cursor::new(slf, handle)) },
            )?;
        let helpers = py.import("aioodbc")?;
        wrap_context_manager(
            py,
            coro,
            helpers.getattr("_close_cursor")?,
            helpers.getattr("_close_cursor_on_error")?,
        )
    }

    /// Convenience method: allocates a `Cursor`, executes `sql` on it and
    /// returns that cursor, mirroring `pyodbc.Connection.execute`.
    #[pyo3(signature = (sql, *args))]
    fn execute<'p>(
        slf: Py<Self>,
        py: Python<'p>,
        sql: String,
        args: Vec<Bound<'p, PyAny>>,
    ) -> PyResult<Bound<'p, PyAny>> {
        let handle = slf.borrow(py).handle.clone();
        let params = crate::pyconv::expand_params(args)?
            .iter()
            .map(crate::pyconv::py_to_cell)
            .collect::<PyResult<Vec<_>>>()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let cursor = Cursor::new(slf, handle);
            cursor.run_execute(sql, params).await?;
            Ok(cursor)
        })
    }

    fn commit<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            handle.commit().await.map_err(actor_err_to_py)
        })
    }

    fn rollback<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            handle.rollback().await.map_err(actor_err_to_py)
        })
    }

    fn close<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            handle.close().await;
            Ok(())
        })
    }
}

impl Connection {
    pub fn new(handle: ActorHandle, autocommit: bool) -> Self {
        Connection {
            handle,
            autocommit: Arc::new(AtomicBool::new(autocommit)),
        }
    }
}

/// Accepts an ODBC connection string and returns a new `Connection`.
#[pyfunction]
#[pyo3(signature = (*, dsn, autocommit=false, **_kwargs))]
pub fn connect<'p>(
    py: Python<'p>,
    dsn: String,
    autocommit: bool,
    _kwargs: Option<Bound<'p, PyAny>>,
) -> PyResult<Bound<'p, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let handle = crate::actor::connect(dsn, autocommit)
            .await
            .map_err(to_py_err)?;
        Ok(Connection::new(handle, autocommit))
    })
}
