//! Coroutine implementation agnostic from Python async runtime.
//!
//! It means that only [`PyFuture`](crate::types::PyFuture) can be awaited in the future wrapped
//! by the coroutine; otherwise it panics.
use crate::coroutine::CoroutineWaker;
use crate::{PyObject, PyResult, Python};

const PYTHON_ONLY: &str = "PythonOnlyWaker doesn't support Rust futures";

/// Agnostic coroutine waker.
///
/// Panics if used to poll something else than a [`PyFuture`](crate::types::PyFuture).
pub struct Waker;

impl CoroutineWaker for Waker {
    fn new(_py: Python<'_>) -> PyResult<Self> {
        Ok(Self)
    }
    fn yield_(&self, _py: Python<'_>) -> PyResult<PyObject> {
        panic!("{}", PYTHON_ONLY)
    }
    fn yield_waken(_py: Python<'_>) -> PyResult<PyObject> {
        panic!("{}", PYTHON_ONLY)
    }
    fn wake(&self, _py: Python<'_>) -> PyResult<()> {
        panic!("{}", PYTHON_ONLY)
    }
}
