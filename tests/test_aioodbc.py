import datetime

import pytest

import aioodbc


async def test_connect_execute_fetch(dsn):
    conn = await aioodbc.connect(dsn=dsn, autocommit=True)
    assert conn.autocommit is True
    assert not conn.closed

    cur = await conn.cursor()
    await cur.execute("CREATE TABLE t (id INTEGER, name VARCHAR(50))")
    await cur.execute("INSERT INTO t VALUES (1, 'alice'), (2, 'bob')")
    await cur.execute("SELECT id, name FROM t ORDER BY id")

    rows = await cur.fetchall()
    assert [tuple(r) for r in rows] == [(1, "alice"), (2, "bob")]
    assert rows[0].id == 1
    assert rows[0].name == "alice"

    await cur.close()
    assert cur.closed
    await conn.close()
    assert conn.closed


async def test_context_managers(dsn):
    async with aioodbc.connect(dsn=dsn, autocommit=True) as conn:
        async with conn.cursor() as cur:
            await cur.execute("SELECT 42 AS age")
            row = await cur.fetchone()
            assert row.age == 42


async def test_fetchone_fetchmany_and_async_iteration(dsn):
    async with aioodbc.connect(dsn=dsn, autocommit=True) as conn:
        async with conn.cursor() as cur:
            await cur.execute("CREATE TABLE t (i INTEGER)")
            await cur.executemany("INSERT INTO t VALUES (?)", [(1,), (2,), (3,)])

            await cur.execute("SELECT i FROM t ORDER BY i")
            assert (await cur.fetchone()).i == 1

            cur.arraysize = 2
            chunk = await cur.fetchmany()
            assert [r.i for r in chunk] == [2, 3]
            assert await cur.fetchone() is None

            await cur.execute("SELECT i FROM t ORDER BY i")
            seen = [row.i async for row in cur]
            assert seen == [1, 2, 3]


async def test_parameters_and_types(dsn):
    async with aioodbc.connect(dsn=dsn, autocommit=True) as conn:
        async with conn.cursor() as cur:
            await cur.execute(
                "CREATE TABLE t (i INTEGER, f REAL, s VARCHAR(50), b BLOB, "
                "d DATE, dt TIMESTAMP)"
            )
            await cur.execute(
                "INSERT INTO t VALUES (?, ?, ?, ?, ?, ?)",
                42,
                3.5,
                "hello",
                b"\x00\x01\x02",
                datetime.date(2024, 1, 2),
                datetime.datetime(2024, 1, 2, 3, 4, 5),
            )
            await cur.execute("INSERT INTO t VALUES (?, ?, ?, ?, ?, ?)", *([None] * 6))

            await cur.execute("SELECT * FROM t ORDER BY i IS NULL, i")
            rows = await cur.fetchall()

            assert rows[0].i == 42
            assert rows[0].f == pytest.approx(3.5)
            assert rows[0].s == "hello"
            assert bytes(rows[0].b) == b"\x00\x01\x02"
            assert rows[0].d == datetime.date(2024, 1, 2)
            assert rows[0].dt == datetime.datetime(2024, 1, 2, 3, 4, 5)
            assert all(v is None for v in rows[1])


async def test_description_and_rowcount(dsn):
    async with aioodbc.connect(dsn=dsn, autocommit=True) as conn:
        async with conn.cursor() as cur:
            await cur.execute("SELECT 1 AS a, 'x' AS b")
            names = [col[0] for col in cur.description]
            types = [col[1] for col in cur.description]
            assert names == ["a", "b"]
            assert types == [int, str]


async def test_errors_raise_and_carry_sqlstate(dsn):
    async with aioodbc.connect(dsn=dsn, autocommit=True) as conn:
        async with conn.cursor() as cur:
            with pytest.raises(aioodbc.Error) as exc_info:
                await cur.execute("SELECT * FROM no_such_table")
            sqlstate, _message = exc_info.value.args
            assert sqlstate


async def test_commit_and_rollback(dsn):
    async with aioodbc.connect(dsn=dsn, autocommit=True) as conn:
        async with conn.cursor() as cur:
            await cur.execute("CREATE TABLE t (i INTEGER)")

        conn.autocommit = False
        assert conn.autocommit is False
        async with conn.cursor() as cur:
            await cur.execute("INSERT INTO t VALUES (1)")
            await conn.rollback()
            await cur.execute("SELECT COUNT(*) AS n FROM t")
            assert (await cur.fetchone()).n == 0

            await cur.execute("INSERT INTO t VALUES (1)")
            await conn.commit()
            await cur.execute("SELECT COUNT(*) AS n FROM t")
            assert (await cur.fetchone()).n == 1


async def test_pool_acquire_release_and_sizing(dsn):
    async with aioodbc.create_pool(dsn=dsn, minsize=1, maxsize=3) as pool:
        async with pool.acquire() as conn:
            cur = await conn.cursor()
            await cur.execute("SELECT 42")
            assert (await cur.fetchone())[0] == 42
            await cur.close()
        assert pool.maxsize == 3
    assert pool.closed
