//! Connection pooling via `deadpool` instead of hand-rolling aiopg's
//! condition-variable-based pool: `deadpool::managed::Manager::recycle`
//! already gives us `pool_recycle` for free, and size/min/max handling,
//! waiting, etc. come from the crate.
//!
//! `deadpool::managed::Object` is an RAII guard (drop = return to pool),
//! but aioodbc's `Pool` API is explicit (`acquire()` / `release(conn)`).
//! We bridge the two by keying live guards on the acquired `Connection`
//! Python object's pointer identity, which stays stable for as long as
//! the caller (and our side table) both hold a reference to it.
use crate::connection::Connection;
use crate::ctxmgr::wrap_context_manager;
use crate::error::Error;
use deadpool::managed::{self, Metrics, RecycleError, RecycleResult};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

struct OdbcManager {
    dsn: String,
    autocommit: bool,
    recycle: Option<Duration>,
}

impl managed::Manager for OdbcManager {
    type Type = Py<Connection>;
    type Error = PyErr;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        let handle = crate::actor::connect(self.dsn.clone(), self.autocommit)
            .await
            .map_err(crate::error::to_py_err)?;
        Python::attach(|py| Py::new(py, Connection::new(handle, self.autocommit)))
    }

    async fn recycle(
        &self,
        conn: &mut Self::Type,
        metrics: &Metrics,
    ) -> RecycleResult<Self::Error> {
        let dead = Python::attach(|py| conn.borrow(py).handle.is_closed());
        if dead {
            return Err(RecycleError::message("connection closed"));
        }
        if let Some(max_age) = self.recycle {
            if metrics.last_used() > max_age {
                return Err(RecycleError::message("pool_recycle exceeded"));
            }
        }
        Ok(())
    }
}

fn pool_err_to_py(e: managed::PoolError<PyErr>) -> PyErr {
    match e {
        managed::PoolError::Backend(err) => err,
        other => Error::new_err(("", other.to_string())),
    }
}

#[pyclass]
pub struct Pool {
    inner: managed::Pool<OdbcManager>,
    minsize: usize,
    /// Checked-out guards, keyed by the acquired `Connection`'s pointer
    /// identity; removing an entry drops the guard, returning it to
    /// `inner` (or discarding it, if `recycle` then rejects it).
    checked_out: Mutex<HashMap<usize, managed::Object<OdbcManager>>>,
}

#[pymethods]
impl Pool {
    #[getter]
    fn minsize(&self) -> usize {
        self.minsize
    }

    #[getter]
    fn maxsize(&self) -> usize {
        self.inner.status().max_size
    }

    #[getter]
    fn size(&self) -> usize {
        self.inner.status().size
    }

    #[getter]
    fn freesize(&self) -> usize {
        self.inner.status().available
    }

    #[getter]
    fn closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Returns a `Connection`, usable both as `conn = await pool.acquire()`
    /// and `async with pool.acquire() as conn:`.
    fn acquire<'p>(slf: Py<Self>, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let inner = slf.borrow(py).inner.clone();
        let slf_for_async = slf.clone_ref(py);
        let coro = pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let guard = inner.get().await.map_err(pool_err_to_py)?;
            Python::attach(|py| {
                let conn = guard.clone_ref(py);
                let key = conn.as_ptr() as usize;
                slf_for_async
                    .borrow(py)
                    .checked_out
                    .lock()
                    .unwrap()
                    .insert(key, guard);
                Ok(conn)
            })
        })?;
        let release = slf.bind(py).getattr("release")?;
        wrap_context_manager(py, coro, release.clone(), release)
    }

    /// Returns a previously-acquired `Connection` back to the pool.
    fn release<'p>(&self, py: Python<'p>, conn: Py<Connection>) -> PyResult<Bound<'p, PyAny>> {
        let key = conn.as_ptr() as usize;
        // Dropping the guard here returns it to `inner` (deadpool recycles
        // it, in the background, on next acquisition).
        self.checked_out.lock().unwrap().remove(&key);
        pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(()) })
    }

    fn close(&self) {
        self.inner.close();
    }

    fn wait_closed<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            while inner.status().size > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(())
        })
    }
}

/// Accepts an ODBC connection string and returns a new `Pool`.
#[pyfunction]
#[pyo3(signature = (*, dsn, minsize=1, maxsize=10, pool_recycle=-1, autocommit=false, **_kwargs))]
pub fn create_pool<'p>(
    py: Python<'p>,
    dsn: String,
    minsize: usize,
    maxsize: usize,
    pool_recycle: i64,
    autocommit: bool,
    _kwargs: Option<Bound<'p, PyAny>>,
) -> PyResult<Bound<'p, PyAny>> {
    let manager = OdbcManager {
        dsn,
        autocommit,
        recycle: (pool_recycle >= 0).then(|| Duration::from_secs(pool_recycle as u64)),
    };
    let inner = managed::Pool::builder(manager)
        .max_size(maxsize.max(1))
        .build()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        // Hold all `minsize` guards at once -- otherwise `get()` would
        // just keep handing back (and we'd keep dropping) the same one
        // idle connection instead of creating `minsize` distinct ones.
        let mut warm = Vec::with_capacity(minsize);
        for _ in 0..minsize {
            warm.push(inner.get().await.map_err(pool_err_to_py)?);
        }
        drop(warm);
        Ok(Pool {
            inner,
            minsize,
            checked_out: Mutex::new(HashMap::new()),
        })
    })
}
