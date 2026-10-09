//! Python <-> `Cell` conversion. Isolated here so the ODBC actor
//! (`actor.rs`) never has to touch the GIL, and so `cell.rs` never has
//! to touch PyO3.
use crate::cell::{Cell, Kind};
use pyo3::{
    types::{
        PyAnyMethods, PyBool, PyBoolMethods, PyDate, PyDateAccess, PyDateTime, PyList, PyTime,
        PyTimeAccess, PyTuple,
    },
    Bound, Py, PyAny, PyResult, Python,
};

/// `execute(sql, *params)` also accepts the whole parameter sequence as
/// one positional argument (`execute(sql, (1, 2))` as well as
/// `execute(sql, 1, 2)`), matching pyodbc/DB-API. Expands the former
/// into the latter so callers only ever need to handle a flat list.
pub fn expand_params<'p>(params: Vec<Bound<'p, PyAny>>) -> PyResult<Vec<Bound<'p, PyAny>>> {
    match params.as_slice() {
        [only] if only.is_instance_of::<PyTuple>() || only.is_instance_of::<PyList>() => {
            only.try_iter()?.collect()
        }
        _ => Ok(params),
    }
}

/// Converts one bound Python argument into a `Cell` ready to bind.
pub fn py_to_cell(obj: &Bound<PyAny>) -> PyResult<Cell> {
    if obj.is_none() {
        return Ok(Cell::Null);
    }
    // `bool` is a subclass of `int` in Python, so it must be checked first.
    if let Ok(b) = obj.cast::<PyBool>() {
        return Ok(Cell::Bool(b.is_true()));
    }
    // `datetime` is a subclass of `date`, so it must be checked first too.
    if let Ok(dt) = obj.cast::<PyDateTime>() {
        return Ok(Cell::Timestamp {
            year: dt.get_year() as i16,
            month: dt.get_month() as u16,
            day: dt.get_day() as u16,
            hour: dt.get_hour() as u16,
            minute: dt.get_minute() as u16,
            second: dt.get_second() as u16,
            fraction: dt.get_microsecond() * 1000,
        });
    }
    if let Ok(d) = obj.cast::<PyDate>() {
        return Ok(Cell::Date {
            year: d.get_year() as i16,
            month: d.get_month() as u16,
            day: d.get_day() as u16,
        });
    }
    if let Ok(t) = obj.cast::<PyTime>() {
        return Ok(Cell::Time {
            hour: t.get_hour() as u16,
            minute: t.get_minute() as u16,
            second: t.get_second() as u16,
        });
    }
    if let Ok(v) = obj.extract::<i64>() {
        return Ok(Cell::Int(v));
    }
    if let Ok(v) = obj.extract::<f64>() {
        return Ok(Cell::Float(v));
    }
    if let Ok(v) = obj.extract::<Vec<u8>>() {
        return Ok(Cell::Bytes(v));
    }
    if let Ok(v) = obj.extract::<String>() {
        return Ok(Cell::Text(v));
    }
    // Fallback: stringify anything else (e.g. Decimal) and let the driver parse it.
    Ok(Cell::Text(obj.str()?.to_string()))
}

/// Converts a fetched `Cell` back into a Python object.
pub fn cell_to_py(py: Python<'_>, cell: &Cell) -> PyResult<Py<PyAny>> {
    use pyo3::IntoPyObjectExt;
    Ok(match cell {
        Cell::Null => py.None(),
        Cell::Bool(b) => b.into_py_any(py)?,
        Cell::Int(i) => i.into_py_any(py)?,
        Cell::Float(f) => f.into_py_any(py)?,
        Cell::Text(s) => s.into_py_any(py)?,
        Cell::Bytes(b) => b.into_py_any(py)?,
        Cell::Date { year, month, day } => {
            PyDate::new(py, *year as i32, *month as u8, *day as u8)?.into_py_any(py)?
        }
        Cell::Time {
            hour,
            minute,
            second,
        } => {
            PyTime::new(py, *hour as u8, *minute as u8, *second as u8, 0, None)?.into_py_any(py)?
        }
        Cell::Timestamp {
            year,
            month,
            day,
            hour,
            minute,
            second,
            fraction,
        } => PyDateTime::new(
            py,
            *year as i32,
            *month as u8,
            *day as u8,
            *hour as u8,
            *minute as u8,
            *second as u8,
            fraction / 1000,
            None,
        )?
        .into_py_any(py)?,
    })
}

/// The DB-API `type_code`: the Python type fetched values of this kind
/// will have, mirroring pyodbc's convention.
pub fn type_code(py: Python<'_>, kind: Kind) -> PyResult<Py<PyAny>> {
    let builtins = py.import("builtins")?;
    let datetime = py.import("datetime")?;
    let obj = match kind {
        Kind::Bool => builtins.getattr("bool")?,
        Kind::Int => builtins.getattr("int")?,
        Kind::Float => builtins.getattr("float")?,
        Kind::Text => builtins.getattr("str")?,
        Kind::Bytes => builtins.getattr("bytes")?,
        Kind::Date => datetime.getattr("date")?,
        Kind::Time => datetime.getattr("time")?,
        Kind::Timestamp => datetime.getattr("datetime")?,
    };
    Ok(obj.unbind())
}
