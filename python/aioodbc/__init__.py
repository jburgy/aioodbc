"""Thin Python glue around the Rust/PyO3 extension (`aioodbc._aioodbc`).

Everything that touches ODBC lives in Rust; this module only carries the
`async with`/`await` duality that `connect()`, `create_pool()`,
`Connection.cursor()` and `Pool.acquire()` all need, plus the small
commit/rollback-on-exit helpers -- all of it generic Python control flow,
ported close to verbatim from aioodbc's original ``aioodbc/utils.py`` and
``aioodbc/connection.py`` (same project, same license).
"""
from typing import Any, Awaitable, Callable, Coroutine, Generic, Optional, Type, TypeVar

from ._aioodbc import Connection, Cursor, Error, Pool, __version__, connect as _connect, create_pool as _create_pool

__all__ = ("connect", "Connection", "create_pool", "Pool", "Cursor", "Error", "__version__")

_T = TypeVar("_T")
_Release = Callable[[_T], Awaitable[None]]


class _ContextManager(Coroutine[Any, None, _T], Generic[_T]):
    __slots__ = ("_coro", "_obj", "_release", "_release_on_exception")

    def __init__(
        self,
        coro: Coroutine[Any, None, _T],
        release: _Release[_T],
        release_on_exception: Optional[_Release[_T]] = None,
    ):
        self._coro = coro
        self._obj: Optional[_T] = None
        self._release = release
        self._release_on_exception = release_on_exception if release_on_exception is not None else release

    def send(self, value: Any) -> Any:
        return self._coro.send(value)

    def throw(self, *args: Any) -> Any:
        return self._coro.throw(*args)

    def close(self) -> None:
        self._coro.close()

    def __await__(self):
        return self._coro.__await__()

    async def __aenter__(self) -> _T:
        self._obj = await self._coro
        return self._obj

    async def __aexit__(self, exc_type: Optional[Type[BaseException]], exc: Optional[BaseException], tb: Any) -> None:
        if self._obj is None:
            return
        try:
            if exc_type is not None:
                await self._release_on_exception(self._obj)
            else:
                await self._release(self._obj)
        finally:
            self._obj = None


async def _close_cursor(cur: Cursor) -> None:
    if not cur.connection.autocommit:
        await cur.connection.commit()
    await cur.close()


async def _close_cursor_on_error(cur: Cursor) -> None:
    await cur.connection.rollback()
    await cur.close()


async def _disconnect(conn: Connection) -> None:
    if not conn.autocommit:
        await conn.commit()
    await conn.close()


async def _disconnect_on_error(conn: Connection) -> None:
    await conn.rollback()
    await conn.close()


async def _destroy_pool(pool: Pool) -> None:
    pool.close()
    await pool.wait_closed()


def connect(**kwargs: Any) -> _ContextManager[Connection]:
    """Accepts an ODBC connection string (``dsn=``) and returns a new
    `Connection`, usable both as ``conn = await connect(...)`` and
    ``async with connect(...) as conn:``.
    """
    return _ContextManager[Connection](_connect(**kwargs), _disconnect, _disconnect_on_error)


def create_pool(**kwargs: Any) -> _ContextManager[Pool]:
    """Returns a new `Pool`, usable both as ``pool = await create_pool(...)``
    and ``async with create_pool(...) as pool:``.
    """
    return _ContextManager[Pool](_create_pool(**kwargs), _destroy_pool, _destroy_pool)
