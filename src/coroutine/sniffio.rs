//! Coroutine implementation using sniffio to select the appropriate implementation,
//! compatible with anyio.
use crate::coroutine::{asyncio, trio, CoroutineWaker};
use crate::exceptions::PyRuntimeError;
use crate::sync::GILOnceCell;
use crate::{PyAny, PyErr, PyObject, PyResult, Python};

fn current_async_library(py: Python<'_>) -> PyResult<&PyAny> {
    static GET_RUNNING_LOOP: GILOnceCell<PyObject> = GILOnceCell::new();
    let import = || -> PyResult<_> {
        let module = py.import("sniffio")?;
        Ok(module.getattr("current_async_library")?.into())
    };
    GET_RUNNING_LOOP
        .get_or_try_init(py, import)?
        .as_ref(py)
        .call0()
}

fn unsupported(runtime: &str) -> PyErr {
    PyRuntimeError::new_err(format!("unsupported runtime {runtime}"))
}

/// Sniffio/anyio-compatible coroutine waker.
///
/// Polling a Rust future calls `sniffio.current_async_library` to select the appropriate
/// implementation, either asyncio or trio.
pub enum Waker {
    /// [`asyncio::Waker`]
    Asyncio(asyncio::Waker),
    /// [`trio::Waker`]
    Trio(trio::Waker),
}

impl CoroutineWaker for Waker {
    fn new(py: Python<'_>) -> PyResult<Self> {
        let sniffed = current_async_library(py)?;
        match sniffed.extract()? {
            "asyncio" => Ok(Self::Asyncio(asyncio::Waker::new(py)?)),
            "trio" => Ok(Self::Trio(trio::Waker::new(py)?)),
            rt => Err(unsupported(rt)),
        }
    }

    fn yield_(&self, py: Python<'_>) -> PyResult<PyObject> {
        match self {
            Waker::Asyncio(w) => w.yield_(py),
            Waker::Trio(w) => w.yield_(py),
        }
    }

    fn yield_waken(py: Python<'_>) -> PyResult<PyObject> {
        let sniffed = current_async_library(py)?;
        match sniffed.extract()? {
            "asyncio" => asyncio::Waker::yield_waken(py),
            "trio" => trio::Waker::yield_waken(py),
            rt => Err(unsupported(rt)),
        }
    }

    fn wake(&self, py: Python<'_>) -> PyResult<()> {
        match self {
            Waker::Asyncio(w) => w.wake(py),
            Waker::Trio(w) => w.wake(py),
        }
    }
}
