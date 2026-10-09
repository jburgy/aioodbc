use crate::actor::ActorHandle;
use crate::cell::{Cell, ColumnMeta};
use crate::connection::Connection;
use crate::error::actor_err_to_py;
use crate::pyconv::{cell_to_py, py_to_cell, type_code};
use pyo3::exceptions::PyStopAsyncIteration;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct CursorState {
    generation: u64,
    columns: Vec<ColumnMeta>,
    rowcount: i64,
    arraysize: usize,
    pending: VecDeque<Vec<Cell>>,
    exhausted: bool,
    closed: bool,
    row_type: Option<Py<PyAny>>,
}

/// Cursors represent a database cursor, used to manage the context of a
/// fetch operation. Cursors created from the same connection are not
/// isolated -- see the `ponytail` note on `crate::actor` about the one
/// open statement per connection simplification.
#[pyclass]
pub struct Cursor {
    connection: Py<Connection>,
    handle: ActorHandle,
    state: Arc<Mutex<CursorState>>,
}

impl Cursor {
    pub fn new(connection: Py<Connection>, handle: ActorHandle) -> Self {
        Cursor {
            connection,
            handle,
            state: Arc::new(Mutex::new(CursorState {
                generation: 0,
                columns: Vec::new(),
                rowcount: -1,
                arraysize: 1,
                pending: VecDeque::new(),
                exhausted: true,
                closed: false,
                row_type: None,
            })),
        }
    }

    pub async fn run_execute(&self, sql: String, params: Vec<Cell>) -> PyResult<()> {
        do_execute(&self.handle, &self.state, sql, params).await
    }
}

async fn do_execute(
    handle: &ActorHandle,
    state: &Mutex<CursorState>,
    sql: String,
    params: Vec<Cell>,
) -> PyResult<()> {
    let result = handle.execute(sql, params).await.map_err(actor_err_to_py)?;
    let exhausted = result.columns.is_empty();
    let mut st = state.lock().unwrap();
    st.generation = result.generation;
    st.columns = result.columns;
    st.rowcount = result.rowcount;
    st.pending.clear();
    st.exhausted = exhausted;
    st.row_type = None;
    Ok(())
}

/// Pops one row off the local prefetch buffer, asking the actor for
/// another batch (per `BATCH_SIZE` in `actor.rs`) once it runs dry.
async fn next_row(
    handle: &ActorHandle,
    state: &Arc<Mutex<CursorState>>,
) -> PyResult<Option<Vec<Cell>>> {
    loop {
        enum Step {
            Row(Vec<Cell>),
            Done,
            NeedMore(u64),
        }
        let step = {
            let mut st = state.lock().unwrap();
            if let Some(row) = st.pending.pop_front() {
                Step::Row(row)
            } else if st.exhausted {
                Step::Done
            } else {
                Step::NeedMore(st.generation)
            }
        };
        match step {
            Step::Row(row) => return Ok(Some(row)),
            Step::Done => return Ok(None),
            Step::NeedMore(generation) => {
                match handle.fetch(generation).await.map_err(actor_err_to_py)? {
                    Some(rows) => state.lock().unwrap().pending.extend(rows),
                    None => state.lock().unwrap().exhausted = true,
                }
            }
        }
    }
}

fn build_row(py: Python<'_>, state: &Mutex<CursorState>, cells: Vec<Cell>) -> PyResult<Py<PyAny>> {
    let row_type = {
        let mut st = state.lock().unwrap();
        if st.row_type.is_none() {
            let names = st
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>();
            let namedtuple = py.import("collections")?.getattr("namedtuple")?;
            let kwargs = PyDict::new(py);
            kwargs.set_item("rename", true)?;
            let cls = namedtuple.call(("Row", names), Some(&kwargs))?;
            st.row_type = Some(cls.unbind());
        }
        st.row_type.as_ref().unwrap().clone_ref(py)
    };
    let values = cells
        .iter()
        .map(|c| cell_to_py(py, c))
        .collect::<PyResult<Vec<_>>>()?;
    row_type.call1(py, PyTuple::new(py, values)?)
}

fn none_or_row(
    py: Python<'_>,
    state: &Mutex<CursorState>,
    row: Option<Vec<Cell>>,
) -> PyResult<Py<PyAny>> {
    match row {
        Some(cells) => build_row(py, state, cells),
        None => Ok(py.None()),
    }
}

#[pymethods]
impl Cursor {
    #[getter]
    fn connection(&self, py: Python<'_>) -> Py<Connection> {
        self.connection.clone_ref(py)
    }

    #[getter]
    fn rowcount(&self) -> i64 {
        self.state.lock().unwrap().rowcount
    }

    #[getter]
    fn arraysize(&self) -> usize {
        self.state.lock().unwrap().arraysize
    }

    #[setter]
    fn set_arraysize(&self, value: usize) {
        self.state.lock().unwrap().arraysize = value.max(1);
    }

    #[getter]
    fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    /// `(name, type_code, display_size, internal_size, precision, scale,
    /// null_ok)` per column, mirroring pyodbc: only `name`, `type_code`
    /// and `null_ok` are filled in, the rest are always `None`.
    #[getter]
    fn description<'p>(&self, py: Python<'p>) -> PyResult<Option<Py<PyAny>>> {
        use pyo3::IntoPyObjectExt;
        let st = self.state.lock().unwrap();
        if st.columns.is_empty() {
            return Ok(None);
        }
        let rows = st
            .columns
            .iter()
            .map(|c| {
                let tc = type_code(py, c.kind)?;
                PyTuple::new(
                    py,
                    [
                        c.name.clone().into_py_any(py)?,
                        tc,
                        py.None(),
                        py.None(),
                        py.None(),
                        py.None(),
                        c.nullable.into_py_any(py)?,
                    ],
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok(Some(PyTuple::new(py, rows)?.into_any().unbind()))
    }

    #[pyo3(signature = (sql, *params))]
    fn execute<'p>(
        slf: Py<Self>,
        py: Python<'p>,
        sql: String,
        params: Vec<Bound<'p, PyAny>>,
    ) -> PyResult<Bound<'p, PyAny>> {
        let cells = crate::pyconv::expand_params(params)?
            .iter()
            .map(py_to_cell)
            .collect::<PyResult<Vec<_>>>()?;
        let (handle, state) = {
            let this = slf.borrow(py);
            (this.handle.clone(), this.state.clone())
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            do_execute(&handle, &state, sql, cells).await?;
            Ok(slf)
        })
    }

    /// Executes `sql` once per row of parameters in `seq_of_params`.
    /// ponytail: implemented as a plain sequential loop rather than a
    /// true ODBC array/batch bind (`SQLBindParameter` with a parameter
    /// array); simpler, and correct, just not the fastest possible path
    /// for very large batches. Upgrade to `odbc_api::ColumnarBulkInserter`
    /// if that ever shows up in a profile.
    fn executemany<'p>(
        slf: Py<Self>,
        py: Python<'p>,
        sql: String,
        seq_of_params: Vec<Vec<Bound<'p, PyAny>>>,
    ) -> PyResult<Bound<'p, PyAny>> {
        let rows = seq_of_params
            .iter()
            .map(|row| row.iter().map(py_to_cell).collect::<PyResult<Vec<_>>>())
            .collect::<PyResult<Vec<_>>>()?;
        let (handle, state) = {
            let this = slf.borrow(py);
            (this.handle.clone(), this.state.clone())
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            for cells in rows {
                do_execute(&handle, &state, sql.clone(), cells).await?;
            }
            Ok(slf)
        })
    }

    fn fetchone<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        let state = self.state.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let row = next_row(&handle, &state).await?;
            Python::attach(|py| none_or_row(py, &state, row))
        })
    }

    #[pyo3(signature = (size=None))]
    fn fetchmany<'p>(&self, py: Python<'p>, size: Option<usize>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        let state = self.state.clone();
        let n = size.unwrap_or_else(|| self.state.lock().unwrap().arraysize);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut rows = Vec::new();
            for _ in 0..n {
                match next_row(&handle, &state).await? {
                    Some(row) => rows.push(row),
                    None => break,
                }
            }
            Python::attach(|py| {
                let objs = rows
                    .into_iter()
                    .map(|r| build_row(py, &state, r))
                    .collect::<PyResult<Vec<_>>>()?;
                Ok(PyList::new(py, objs)?.into_any().unbind())
            })
        })
    }

    fn fetchall<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        let state = self.state.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut rows = Vec::new();
            while let Some(row) = next_row(&handle, &state).await? {
                rows.push(row);
            }
            Python::attach(|py| {
                let objs = rows
                    .into_iter()
                    .map(|r| build_row(py, &state, r))
                    .collect::<PyResult<Vec<_>>>()?;
                Ok(PyList::new(py, objs)?.into_any().unbind())
            })
        })
    }

    fn close<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        let state = self.state.clone();
        let generation = self.state.lock().unwrap().generation;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            handle.close_cursor(generation).await;
            state.lock().unwrap().closed = true;
            Ok(())
        })
    }

    /// Delegates to the owning `Connection`, like `pyodbc.Cursor.commit`.
    fn commit<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        self.connection.bind(py).call_method0("commit")
    }

    /// Delegates to the owning `Connection`, like `pyodbc.Cursor.rollback`.
    fn rollback<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        self.connection.bind(py).call_method0("rollback")
    }

    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __anext__<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {
        let handle = self.handle.clone();
        let state = self.state.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match next_row(&handle, &state).await? {
                Some(cells) => Python::attach(|py| build_row(py, &state, cells)),
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }

    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(slf) })
    }

    #[pyo3(signature = (*_args))]
    fn __aexit__<'p>(
        &self,
        py: Python<'p>,
        _args: Vec<Bound<'p, PyAny>>,
    ) -> PyResult<Bound<'p, PyAny>> {
        self.close(py)
    }
}
