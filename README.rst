aioodbc
=======
.. image:: https://github.com/aio-libs/aioodbc/workflows/CI/badge.svg
   :target: https://github.com/aio-libs/aioodbc/actions?query=workflow%3ACI
   :alt: GitHub Actions status for master branch
.. image:: https://codecov.io/gh/aio-libs/aioodbc/branch/master/graph/badge.svg
    :target: https://codecov.io/gh/aio-libs/aioodbc
.. image:: https://img.shields.io/pypi/v/aioodbc.svg
    :target: https://pypi.python.org/pypi/aioodbc
.. image:: https://img.shields.io/pypi/pyversions/aioodbc.svg
    :target: https://pypi.org/project/aioodbc
.. image:: https://badges.gitter.im/Join%20Chat.svg
    :target: https://gitter.im/aio-libs/Lobby
    :alt: Chat on Gitter

**aioodbc** is a Python 3.9+ module that makes it possible to access ODBC_ databases
with asyncio_. This fork ports the implementation from pure Python (threads calling
pyodbc_ via an executor) to Rust_ and PyO3_ on top of odbc-api_, using ODBC's own
asynchronous execution support so queries genuinely run without blocking the event
loop, no thread pool required. The public API is unchanged: it still looks and
feels like pyodbc_.

**aioodbc** is fully compatible and tested with uvloop_.


Basic Example
-------------

**aioodbc** provides essentially the same API as pyodbc_, you just need
to use ``await conn.f()`` instead of ``conn.f()``.

Properties are unchanged, so ``conn.prop`` is correct as well as
``conn.prop = val``.


.. code:: python

    import asyncio

    import aioodbc


    async def test_example():
        dsn = "Driver=SQLite;Database=sqlite.db"
        conn = await aioodbc.connect(dsn=dsn)

        cur = await conn.cursor()
        await cur.execute("SELECT 42 AS age;")
        rows = await cur.fetchall()
        print(rows)
        print(rows[0])
        print(rows[0].age)
        await cur.close()
        await conn.close()


    asyncio.run(test_example())


Connection Pool
---------------
Connection pooling is backed by the deadpool_ Rust crate:

.. code:: python

    import asyncio

    import aioodbc


    async def test_pool():
        dsn = "Driver=SQLite3;Database=sqlite.db"
        pool = await aioodbc.create_pool(dsn=dsn)

        async with pool.acquire() as conn:
            cur = await conn.cursor()
            await cur.execute("SELECT 42;")
            r = await cur.fetchall()
            print(r)
            await cur.close()
            await conn.close()
        pool.close()
        await pool.wait_closed()


    asyncio.run(test_pool())


Context Managers
----------------
`Pool`, `Connection` and `Cursor` objects support the context management
protocol:

.. code:: python

    import asyncio

    import aioodbc


    async def test_example():
        dsn = "Driver=SQLite;Database=sqlite.db"

        async with aioodbc.create_pool(dsn=dsn) as pool:
            async with pool.acquire() as conn:
                async with conn.cursor() as cur:
                    await cur.execute("SELECT 42 AS age;")
                    val = await cur.fetchone()
                    print(val)
                    print(val.age)


    asyncio.run(test_example())


Architecture
------------

* Query *execution* and *fetching* use `odbc-api`'s polling-mode asynchronous
  execution (`SQL_ATTR_ASYNC_ENABLE` + repeated polling, yielding to Tokio's
  ``sleep`` between polls) instead of a thread pool -- see the `ODBC asynchronous
  execution docs`_. The notification method (the other asynchronous execution
  model ODBC documents) only works with Microsoft's own Windows driver manager
  and is barely supported by drivers either way, so it was not a realistic
  cross-platform choice; see `odbc-api`'s own guide for details. Operations with
  no async ODBC equivalent at all (connecting, commit/rollback, describing
  columns) remain quick, bounded, synchronous calls -- same as what pyodbc
  itself does for them.
* Each `Connection` is driven by a small background "actor" task that owns the
  ``odbc_api::Connection`` and talks to `Connection`/`Cursor` objects over a
  channel. ``odbc_api::Connection`` is deliberately not `Sync`, which rules out
  sharing it across Tokio's worker-thread pool; each actor instead gets its own
  OS thread running a single-threaded Tokio runtime, the standard fix for
  hosting non-``Send`` async state (the same one-thread-per-connection cost
  most synchronous pooling libraries already pay).
* Only one statement/result set is open per connection at a time (a second
  `Cursor.execute()` on the same connection closes whatever the first cursor
  had open); use one `Cursor` per `Connection`, or a `Pool`, for concurrency.
* Rows are returned as ``collections.namedtuple`` instances (stdlib, not a
  custom row type) so ``row[0]`` and ``row.col_name`` both work.
* Connection pooling is backed by deadpool_ instead of a hand-rolled pool.

Scope cuts from upstream aioodbc/pyodbc, to keep this port roughly the same
size as the Python original: the ODBC catalog/introspection convenience
methods (``getinfo``, ``tables``, ``columns``, ``statistics``,
``primaryKeys``, ``foreignKeys``, ``getTypeInfo``, ``procedures``, output
converters, ``set_attr``, ...), the ``echo``/``ansi``/``after_created``
knobs, and ``dataSources()`` were left out as speculative surface unlikely
to be needed by a port; add them if you need them. ``executemany`` issues
one statement per row rather than a true ODBC array bind. Rows affected by
an ``INSERT``/``UPDATE``/``DELETE`` (``cursor.rowcount``) is always ``-1``
(DB-API's "unknown" sentinel): the safe `odbc-api` wrappers free the
statement before ``SQLRowCount`` could be read for statements that produce
no result set.


Installation
------------

Building **aioodbc** requires a Rust_ toolchain (stable) and maturin_, in
addition to the unixODBC_ library pyodbc_-based tools also need::

      $ sudo apt-get install unixodbc unixodbc-dev
      $ pip install maturin
      $ maturin develop   # or: pip install .

A released wheel only needs the unixODBC runtime library to be installed;
the Rust toolchain is only needed to build from source.


Run tests
---------
To run tests locally, install `unixodbc` and the `sqlite` ODBC driver::

      $ sudo apt-get install unixodbc libsqliteodbc

Create a virtualenv, build the extension and install the dev requirements::

      $ pip install -r requirements-dev.txt
      $ maturin develop

Run tests, lints etc::

      $ cargo fmt --check
      $ cargo clippy
      $ pytest


Other SQL Drivers
-----------------

* aiopg_ - asyncio client for PostgreSQL
* aiomysql_ - asyncio client form MySQL


Requirements
------------

* Python_ 3.9+
* Rust_ (stable, to build from source)
* odbc-api_ / unixODBC_
* uvloop_ (optional)


.. _Python: https://www.python.org
.. _asyncio: http://docs.python.org/3.4/library/asyncio.html
.. _pyodbc: https://github.com/mkleehammer/pyodbc
.. _uvloop: https://github.com/MagicStack/uvloop
.. _ODBC: https://en.wikipedia.org/wiki/Open_Database_Connectivity
.. _aiopg: https://github.com/aio-libs/aiopg
.. _aiomysql: https://github.com/aio-libs/aiomysql
.. _PEP492: https://www.python.org/dev/peps/pep-0492/
.. _unixODBC: http://www.unixodbc.org/
.. _docker: https://docs.docker.com/engine/installation/
.. _Rust: https://www.rust-lang.org/
.. _PyO3: https://pyo3.rs/
.. _odbc-api: https://github.com/pacman82/odbc-api
.. _deadpool: https://github.com/deadpool-rs/deadpool
.. _maturin: https://www.maturin.rs/
.. _ODBC asynchronous execution docs: https://learn.microsoft.com/en-us/sql/odbc/reference/develop-app/asynchronous-execution-notification-method

