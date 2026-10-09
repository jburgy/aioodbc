//! Wraps a Rust-produced coroutine so it can be used both as
//! `x = await thing()` and `async with thing() as x:`. The actual
//! dual-protocol glue (`_ContextManager`) is plain Python control flow
//! reused from `python/aioodbc/__init__.py` -- no reason to reimplement
//! `__await__`/`__aenter__`/`__aexit__` in Rust when a dozen lines of
//! already-written Python do the job.
use pyo3::prelude::*;

pub fn wrap_context_manager<'p>(
    py: Python<'p>,
    coro: Bound<'p, PyAny>,
    release: Bound<'p, PyAny>,
    release_on_exception: Bound<'p, PyAny>,
) -> PyResult<Bound<'p, PyAny>> {
    py.import("aioodbc")?
        .getattr("_ContextManager")?
        .call1((coro, release, release_on_exception))
}
