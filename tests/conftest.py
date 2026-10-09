import os

import pytest


@pytest.fixture
def dsn(tmp_path):
    """SQLite ODBC DSN. Defaults to the "SQLite3" driver name registered
    in odbcinst.ini (what `apt-get install libsqliteodbc` /
    `brew install sqliteodbc` both register it as). Override with the
    DSN env var to test another backend entirely (`async def` tests in
    this file use only standard SQL, so most backends should work), e.g.:

        DSN="Driver=PostgreSQL Unicode;Server=..." pytest
    """
    db_path = tmp_path / "aioodbc_test.db"
    return os.environ.get("DSN", f"Driver=SQLite3;Database={db_path}")
