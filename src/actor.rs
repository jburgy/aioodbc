//! One background task ("actor") per physical ODBC connection. It owns
//! the `odbc_api::Connection` and any currently open statement/cursor
//! as plain local variables -- since the whole lifetime of that
//! self-borrow lives inside one `async fn`, the borrow checker accepts
//! it the same way it accepts any other local state kept alive across
//! `.await` points. Callers (the PyO3 `Connection`/`Cursor` classes)
//! only ever hold a cheap, `Clone`-able `ActorHandle` and talk to the
//! actor over a channel, which also conveniently serializes all
//! statements on one connection without any extra locking.
//!
//! ponytail: only one statement/resultset is open per connection at a
//! time (`Command::Execute` always closes whatever the previous cursor
//! left open). Real ODBC drivers vary wildly in how well they support
//! multiple concurrent open cursors per connection anyway; use one
//! `Cursor` at a time per `Connection` (or a `Pool`) for concurrency.
//! A `generation` counter detects a `Cursor` trying to fetch after a
//! sibling `Cursor` on the same connection has executed something else,
//! and reports it as `ActorError::Stale` instead of silently handing
//! back the wrong rows.
use crate::cell::{cell_to_param, column_meta, read_cell, Cell, ColumnMeta};
use odbc_api::{
    buffers::ColumnarDynBuffer, handles, handles::Statement as RawStatement,
    parameter::InputParameter, Connection, ConnectionOptions, CursorPolling, Error as OdbcError,
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const BATCH_SIZE: usize = 256;

type BlockCursor<'c> =
    odbc_api::BlockCursorPolling<CursorPolling<handles::StatementImpl<'c>>, ColumnarDynBuffer>;

#[derive(Debug)]
pub enum ActorError {
    Odbc(OdbcError),
    /// A sibling `Cursor` executed a new statement on this connection
    /// before this cursor finished fetching its own result set.
    Stale,
    /// The connection actor is gone (connection already closed).
    Closed,
}

impl From<OdbcError> for ActorError {
    fn from(e: OdbcError) -> Self {
        ActorError::Odbc(e)
    }
}

pub struct ExecResult {
    pub columns: Vec<ColumnMeta>,
    /// Rows affected, or -1 when ODBC cannot report it. `odbc-api`'s
    /// safe async wrappers free the statement before we could call
    /// `SQLRowCount` on a DML statement (one that produces no result
    /// set), so DML row counts are always -1 here.
    /// ponytail: upgrade to a real count by driving `SQLExecute` directly
    /// through `odbc_api::handles` if a caller needs it.
    pub rowcount: i64,
    pub generation: u64,
}

enum Command {
    Execute {
        sql: String,
        params: Vec<Cell>,
        reply: oneshot::Sender<Result<ExecResult, ActorError>>,
    },
    Fetch {
        generation: u64,
        reply: oneshot::Sender<Result<Option<Vec<Vec<Cell>>>, ActorError>>,
    },
    CloseCursor {
        generation: u64,
        reply: oneshot::Sender<()>,
    },
    Commit {
        reply: oneshot::Sender<Result<(), OdbcError>>,
    },
    Rollback {
        reply: oneshot::Sender<Result<(), OdbcError>>,
    },
    SetAutocommit {
        value: bool,
        reply: oneshot::Sender<Result<(), OdbcError>>,
    },
    Close {
        reply: oneshot::Sender<()>,
    },
}

#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<Command>,
}

impl ActorHandle {
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    pub async fn execute(&self, sql: String, params: Vec<Cell>) -> Result<ExecResult, ActorError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Execute { sql, params, reply })
            .await
            .map_err(|_| ActorError::Closed)?;
        rx.await.map_err(|_| ActorError::Closed)?
    }

    pub async fn fetch(&self, generation: u64) -> Result<Option<Vec<Vec<Cell>>>, ActorError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Fetch { generation, reply })
            .await
            .map_err(|_| ActorError::Closed)?;
        rx.await.map_err(|_| ActorError::Closed)?
    }

    pub async fn close_cursor(&self, generation: u64) {
        let (reply, rx) = oneshot::channel();
        if self
            .tx
            .send(Command::CloseCursor { generation, reply })
            .await
            .is_ok()
        {
            let _ = rx.await;
        }
    }

    pub async fn commit(&self) -> Result<(), ActorError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Commit { reply })
            .await
            .map_err(|_| ActorError::Closed)?;
        Ok(rx.await.map_err(|_| ActorError::Closed)??)
    }

    pub async fn rollback(&self) -> Result<(), ActorError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Rollback { reply })
            .await
            .map_err(|_| ActorError::Closed)?;
        Ok(rx.await.map_err(|_| ActorError::Closed)??)
    }

    pub async fn set_autocommit(&self, value: bool) -> Result<(), ActorError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::SetAutocommit { value, reply })
            .await
            .map_err(|_| ActorError::Closed)?;
        Ok(rx.await.map_err(|_| ActorError::Closed)??)
    }

    pub async fn close(&self) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Command::Close { reply }).await.is_ok() {
            let _ = rx.await;
        }
    }
}

/// Connects (blocking -- `SQLConnect` has no async equivalent in ODBC)
/// and spawns the actor that owns the resulting connection.
///
/// ponytail: `odbc_api::Connection` is deliberately `!Sync` (ODBC
/// connection handles are not safe to touch from two threads at once),
/// which makes any future holding a borrow of it (i.e. an open
/// `CursorPolling`, kept alive across `execute`/`fetch` calls) `!Send`
/// too -- so it cannot run on `tokio::spawn`'s multi-threaded pool
/// (`Runtime::block_on` has no such bound, so a plain `current_thread`
/// runtime hosts it with no extra machinery). Instead each connection
/// gets its own OS thread running a private single-threaded Tokio
/// runtime. One thread per open `Connection` is the same cost most
/// sync DB pooling libraries (HikariCP, r2d2, ...) already pay.
pub async fn connect(dsn: String, autocommit: bool) -> Result<ActorHandle, OdbcError> {
    let (tx, rx) = mpsc::channel(8);
    let (ready_tx, ready_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let conn = crate::env::environment()
            .connect_with_connection_string(&dsn, ConnectionOptions::default())
            .and_then(|conn| conn.set_autocommit(autocommit).map(|()| conn));
        let conn = match conn {
            Ok(conn) => conn,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let _ = ready_tx.send(Ok(()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("failed to start per-connection runtime");
        rt.block_on(run(conn, rx));
    });
    ready_rx.await.unwrap_or(Err(OdbcError::NoDiagnostics {
        function: "connect",
    }))?;
    Ok(ActorHandle { tx })
}

async fn run(conn: Connection<'static>, mut rx: mpsc::Receiver<Command>) {
    let mut block_cursor: Option<BlockCursor<'_>> = None;
    let mut columns: Vec<ColumnMeta> = Vec::new();
    let mut generation: u64 = 0;

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::Execute { sql, params, reply } => {
                block_cursor = None;
                generation += 1;
                let outcome = execute(&conn, &sql, &params, &mut columns, &mut block_cursor)
                    .await
                    .map(|(columns, rowcount)| ExecResult {
                        columns,
                        rowcount,
                        generation,
                    })
                    .map_err(ActorError::from);
                let _ = reply.send(outcome);
            }
            Command::Fetch {
                generation: requested,
                reply,
            } => {
                let outcome = if requested != generation {
                    Err(ActorError::Stale)
                } else {
                    fetch_batch(&mut block_cursor, &columns)
                        .await
                        .map_err(ActorError::from)
                };
                let _ = reply.send(outcome);
            }
            Command::CloseCursor {
                generation: requested,
                reply,
            } => {
                if requested == generation {
                    block_cursor = None;
                }
                let _ = reply.send(());
            }
            Command::Commit { reply } => {
                let _ = reply.send(conn.commit());
            }
            Command::Rollback { reply } => {
                let _ = reply.send(conn.rollback());
            }
            Command::SetAutocommit { value, reply } => {
                let _ = reply.send(conn.set_autocommit(value));
            }
            Command::Close { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
    // `conn` drops here, running SQLDisconnect/SQLFreeHandle.
}

async fn execute<'c>(
    conn: &'c Connection<'static>,
    sql: &str,
    params: &[Cell],
    columns: &mut Vec<ColumnMeta>,
    block_cursor: &mut Option<BlockCursor<'c>>,
) -> Result<(Vec<ColumnMeta>, i64), OdbcError> {
    let bound: Vec<Box<dyn InputParameter>> = params.iter().map(cell_to_param).collect();
    let maybe_cursor = conn
        .execute_polling(sql, bound.as_slice(), || tokio::time::sleep(POLL_INTERVAL))
        .await?;
    match maybe_cursor {
        Some(mut cursor) => {
            let cols = describe_columns(&mut cursor)?;
            let descs = cols.iter().map(|c| c.buffer_desc).collect::<Vec<_>>();
            let buffer = ColumnarDynBuffer::from_descs(BATCH_SIZE, descs);
            *block_cursor = Some(cursor.bind_buffer(buffer)?);
            *columns = cols.clone();
            Ok((cols, -1))
        }
        None => {
            columns.clear();
            Ok((Vec::new(), -1))
        }
    }
}

fn describe_columns(
    cursor: &mut CursorPolling<handles::StatementImpl<'_>>,
) -> Result<Vec<ColumnMeta>, OdbcError> {
    use odbc_api::handles::AsStatementRef;
    let mut stmt = cursor.as_stmt_ref();
    let num_cols = RawStatement::num_result_cols(&mut stmt).into_result(&stmt)?;
    let mut desc = odbc_api::ColumnDescription::default();
    let mut out = Vec::with_capacity(num_cols.max(0) as usize);
    for i in 1..=num_cols {
        RawStatement::describe_col(&mut stmt, i as u16, &mut desc).into_result(&stmt)?;
        let name = desc.name_to_string().unwrap_or_default();
        out.push(column_meta(name, desc.data_type, desc.nullability));
    }
    Ok(out)
}

async fn fetch_batch<'c>(
    block_cursor: &mut Option<BlockCursor<'c>>,
    columns: &[ColumnMeta],
) -> Result<Option<Vec<Vec<Cell>>>, OdbcError> {
    let Some(bc) = block_cursor.as_mut() else {
        return Ok(None);
    };
    let batch = bc.fetch(|| tokio::time::sleep(POLL_INTERVAL)).await?;
    let Some(batch) = batch else {
        *block_cursor = None;
        return Ok(None);
    };
    // Copies buffer bytes into an owned `Cell` here rather than straight
    // into a `PyUnicode`/`PyBytes` (which would save the later copy in
    // `cell_to_py`): that would need the GIL, and this actor thread never
    // touches it. One extra copy for Text/Bytes beats coupling the actor
    // to PyO3 for a cost nothing has shown to matter.
    let rows = (0..batch.num_rows())
        .map(|row| {
            columns
                .iter()
                .enumerate()
                .map(|(col, meta)| read_cell(batch, col, meta.kind, row))
                .collect()
        })
        .collect();
    Ok(Some(rows))
}
