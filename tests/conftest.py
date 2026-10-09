import os

import pytest


@pytest.fixture
def dsn(tmp_path):
    """SQLite ODBC DSN. Override with the DSN env var to test another
    driver (`async def` tests in this file use only standard SQL, so
    most backends should work), e.g.:

        DSN="Driver=PostgreSQL Unicode;Server=..." pytest
    """
    default_driver = "/opt/homebrew/lib/libsqlite3odbc.dylib"
    db_path = tmp_path / "aioodbc_test.db"
    return os.environ.get("DSN", f"Driver={default_driver};Database={db_path}")
