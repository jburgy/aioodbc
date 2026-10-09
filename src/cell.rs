//! `Cell`: a small, GIL-free stand-in for a fetched/bound value. Keeping
//! ODBC <-> Rust conversion (this file) separate from Rust <-> Python
//! conversion (`pyconv.rs`) lets the connection actor stay free of PyO3
//! and the GIL entirely.
use odbc_api::{
    buffers::{AnyColumnBufferSlice, BufferDesc, ColumnarDynBuffer},
    parameter::{InputParameter, VarBinaryBox, VarCharBox, WithDataType},
    sys::{Date, Time, Timestamp},
    Bit, DataType, Nullability, Nullable,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Date {
        year: i16,
        month: u16,
        day: u16,
    },
    Time {
        hour: u16,
        minute: u16,
        second: u16,
    },
    Timestamp {
        year: i16,
        month: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        fraction: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Text,
    Bytes,
    Date,
    Time,
    Timestamp,
}

#[derive(Clone, Debug)]
pub struct ColumnMeta {
    pub name: String,
    pub kind: Kind,
    pub nullable: bool,
    pub buffer_desc: BufferDesc,
}

/// Picks a buffer layout for a column. Always binds nullable, fixed-width
/// buffers (`i64`/`f64` rather than the narrowest int that fits) -- a
/// few wasted bytes per cell is a fair trade for one unified match arm
/// per `Kind` instead of one per ODBC integer width.
pub fn column_meta(name: String, data_type: DataType, nullability: Nullability) -> ColumnMeta {
    use DataType::*;
    let (buffer_desc, kind) = match data_type {
        TinyInt | SmallInt | Integer | BigInt => (BufferDesc::I64 { nullable: true }, Kind::Int),
        Numeric { scale, .. } | Decimal { scale, .. } if scale == 0 => {
            (BufferDesc::I64 { nullable: true }, Kind::Int)
        }
        Bit => (BufferDesc::Bit { nullable: true }, Kind::Bool),
        Real | Float { .. } | Double => (BufferDesc::F64 { nullable: true }, Kind::Float),
        Date => (BufferDesc::Date { nullable: true }, Kind::Date),
        Time { .. } => (BufferDesc::Time { nullable: true }, Kind::Time),
        Timestamp { .. } => (BufferDesc::Timestamp { nullable: true }, Kind::Timestamp),
        Binary { length } | Varbinary { length } | LongVarbinary { length } => (
            BufferDesc::Binary {
                max_bytes: length.map(|l| l.get()).unwrap_or(1 << 20),
            },
            Kind::Bytes,
        ),
        other => {
            let max_str_len = other.display_size().map(|l| l.get()).unwrap_or(4000).max(1);
            (BufferDesc::Text { max_str_len }, Kind::Text)
        }
    };
    ColumnMeta {
        name,
        kind,
        nullable: !matches!(nullability, Nullability::NoNulls),
        buffer_desc,
    }
}

/// Reads row `row` of column `col` (bound per `kind`) out of a fetched
/// batch and turns it into an owned `Cell`.
pub fn read_cell(batch: &ColumnarDynBuffer, col: usize, kind: Kind, row: usize) -> Cell {
    let slice = batch.column(col);
    match kind {
        Kind::Bool => as_nullable::<Bit>(slice, row)
            .map(|b| Cell::Bool(b.as_bool()))
            .unwrap_or(Cell::Null),
        Kind::Int => as_nullable::<i64>(slice, row)
            .map(|v| Cell::Int(*v))
            .unwrap_or(Cell::Null),
        Kind::Float => as_nullable::<f64>(slice, row)
            .map(|v| Cell::Float(*v))
            .unwrap_or(Cell::Null),
        Kind::Text => slice
            .as_text()
            .and_then(|s| s.get(row))
            .map(|bytes| Cell::Text(String::from_utf8_lossy(bytes).into_owned()))
            .unwrap_or(Cell::Null),
        Kind::Bytes => slice
            .as_binary()
            .and_then(|s| s.get(row))
            .map(|bytes| Cell::Bytes(bytes.to_vec()))
            .unwrap_or(Cell::Null),
        Kind::Date => as_nullable::<Date>(slice, row)
            .map(|d| Cell::Date {
                year: d.year,
                month: d.month,
                day: d.day,
            })
            .unwrap_or(Cell::Null),
        Kind::Time => as_nullable::<Time>(slice, row)
            .map(|t| Cell::Time {
                hour: t.hour,
                minute: t.minute,
                second: t.second,
            })
            .unwrap_or(Cell::Null),
        Kind::Timestamp => as_nullable::<Timestamp>(slice, row)
            .map(|t| Cell::Timestamp {
                year: t.year,
                month: t.month,
                day: t.day,
                hour: t.hour,
                minute: t.minute,
                second: t.second,
                fraction: t.fraction,
            })
            .unwrap_or(Cell::Null),
    }
}

fn as_nullable<T: odbc_api::Pod>(slice: AnyColumnBufferSlice<'_>, row: usize) -> Option<&T> {
    slice.as_nullable_slice::<T>().and_then(|s| s.get(row))
}

/// Converts a bound parameter value into something `odbc-api` can bind.
/// `Null` always binds as a NULL `VARCHAR`; this works for every backend
/// we tested even when the target column is some other type, so we did
/// not bother threading target-column-type information through to pick
/// a more specific NULL.
pub fn cell_to_param(cell: &Cell) -> Box<dyn InputParameter> {
    match cell {
        Cell::Null => Box::new(VarCharBox::null()),
        Cell::Bool(b) => Box::new(Nullable::new(Bit::from_bool(*b))),
        Cell::Int(v) => Box::new(Nullable::new(*v)),
        Cell::Float(v) => Box::new(Nullable::new(*v)),
        Cell::Text(s) => Box::new(VarCharBox::from_string(s.clone())),
        Cell::Bytes(b) => Box::new(VarBinaryBox::from_vec(b.clone())),
        Cell::Date { year, month, day } => Box::new(Date {
            year: *year,
            month: *month,
            day: *day,
        }),
        Cell::Time {
            hour,
            minute,
            second,
        } => Box::new(WithDataType {
            value: Time {
                hour: *hour,
                minute: *minute,
                second: *second,
            },
            data_type: DataType::Time { precision: 0 },
        }),
        Cell::Timestamp {
            year,
            month,
            day,
            hour,
            minute,
            second,
            fraction,
        } => Box::new(WithDataType {
            value: Timestamp {
                year: *year,
                month: *month,
                day: *day,
                hour: *hour,
                minute: *minute,
                second: *second,
                fraction: *fraction,
            },
            data_type: DataType::Timestamp { precision: 9 },
        }),
    }
}
