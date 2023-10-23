//! Python coroutine implementation, used notably when wrapping `async fn`
//! with `#[pyfunction]`/`#[pymethods]`.
use crate::coroutine::waker::Waker;
use crate::exceptions::{PyAttributeError, PyRuntimeError, PyRuntimeWarning, PyStopIteration};
use crate::pyclass::IterNextOutput;
use crate::{IntoPy, Py, PyAny, PyErr, PyObject, PyResult, Python};
use pyo3_macros::{pyclass, pymethods};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

pub mod agnostic;
pub mod asyncio;
mod cancel;
pub mod sniffio;
pub mod trio;
pub(crate) mod waker;

use crate::types::PyString;
pub use cancel::CancelHandle;
pub use sniffio as anyio;
pub use waker::{CoroutineWaker, CoroutineWakerExt};

const COROUTINE_REUSED_ERROR: &str = "cannot reuse already awaited coroutine";

/// Default behavior for coroutine `throw` method: simply reraise the exception.
pub fn raise_on_throw(error: &PyAny) -> PyResult<()> {
    Err(PyErr::from_value(error))
}

trait CoroutineImpl {
    fn poll(
        &mut self,
        py: Python<'_>,
        prev_result: Option<Result<PyObject, PyObject>>,
    ) -> PyResult<IterNextOutput<PyObject, PyObject>>;

    fn drop(&mut self, qualname: Option<Py<PyString>>);
}

struct CoroutineInner<F, W, TC, const ALLOW_THREADS: bool> {
    future: Option<F>,
    waker: Option<Arc<Waker<W>>>,
    throw_callback: TC,
}

impl<F, W, TC, const ALLOW_THREADS: bool> CoroutineInner<F, W, TC, ALLOW_THREADS> {
    fn complete(&mut self) {
        // the Rust future is dropped, and the field set to `None`
        // to indicate the coroutine has been run to completion
        drop(self.future.take());
    }
}

impl<F, T, E, W, TC, const ALLOW_THREADS: bool> CoroutineImpl
    for CoroutineInner<F, W, TC, ALLOW_THREADS>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: IntoPy<PyObject> + Send,
    E: Send,
    PyErr: From<E>,
    W: CoroutineWaker + Send + Sync + 'static,
    TC: Fn(&PyAny) -> PyResult<()> + Send + 'static,
{
    fn poll(
        &mut self,
        py: Python<'_>,
        mut prev_result: Option<Result<PyObject, PyObject>>,
    ) -> PyResult<IterNextOutput<PyObject, PyObject>> {
        // raise if the coroutine has already been run to completion
        let future_rs = match self.future {
            // SAFETY: future is never moved until dropped
            Some(ref mut fut) => unsafe { Pin::new_unchecked(fut) },
            None => return Err(PyRuntimeError::new_err(COROUTINE_REUSED_ERROR)),
        };
        // if the future is not pending on a Python awaitable,
        // execute throw callback or complete on close
        if !matches!(self.waker, Some(ref w) if w.yielded_from_awaitable(py)) {
            match prev_result {
                Some(Err(err)) => {
                    if let Err(err) = (self.throw_callback)(err.as_ref(py)) {
                        self.complete();
                        return Err(err);
                    }
                    prev_result = Some(Ok(py.None()));
                }
                None => {
                    self.complete();
                    return Ok(IterNextOutput::Return(py.None()));
                }
                res => prev_result = res,
            }
        }
        // create a new waker, or try to reset it in place
        if let Some(waker) = self.waker.as_mut().and_then(Arc::get_mut) {
            waker.reset(prev_result);
        } else {
            self.waker = Some(Arc::new(Waker::new(prev_result)));
        }
        let waker = std::task::Waker::from(self.waker.clone().unwrap());
        let poll = if ALLOW_THREADS {
            py.allow_threads(|| future_rs.poll(&mut Context::from_waker(&waker)))
        } else {
            future_rs.poll(&mut Context::from_waker(&waker))
        };
        // poll the Rust future and forward its results if ready
        if let Poll::Ready(res) = poll {
            self.complete();
            return Ok(IterNextOutput::Return(res?.into_py(py)));
        }
        match self.waker.as_ref().unwrap().yield_(py) {
            Ok(to_yield) => Ok(IterNextOutput::Yield(to_yield)),
            Err(err) => {
                self.complete();
                Err(err)
            }
        }
    }

    fn drop(&mut self, qualname: Option<Py<PyString>>) {
        if self.future.is_some() {
            Python::with_gil(|gil| {
                let qualname = qualname
                    .as_ref()
                    .map_or(Ok("<coroutine>"), |n| n.as_ref(gil).to_str())
                    .unwrap();
                let message = format!("coroutine {qualname} was never awaited");
                PyErr::warn(gil, gil.get_type::<PyRuntimeWarning>(), &message, 2)
                    .expect("warning error");
                self.poll(gil, None).expect("coroutine close error");
            })
        }
    }
}

/// Python coroutine wrapping a [`Future`].
#[pyclass(crate = "crate")]
pub struct Coroutine {
    name: Option<Py<PyString>>,
    qualname: Option<Py<PyString>>,
    implem: Box<dyn CoroutineImpl + Send>,
}

impl Coroutine {
    ///  Wrap a future into a Python coroutine.
    ///
    /// Coroutine `send` polls the wrapped future, ignoring the value passed
    /// (should always be `None` anyway).
    ///
    /// Coroutine `throw` passes the exception to `throw_callback`, and polls
    /// the wrapped future if no exception is raised.
    ///
    /// When [`PyFuture`](crate::types::PyFuture) is awaited in the Rust future,
    /// objects passed in coroutine `send`/`throw` are forwarded to the Python
    /// awaitable.
    pub fn new<T, E, W, const ALLOW_THREADS: bool>(
        name: Option<Py<PyString>>,
        mut qualname: Option<Py<PyString>>,
        future: impl Future<Output = Result<T, E>> + Send + 'static,
        throw_callback: impl Fn(&PyAny) -> PyResult<()> + Send + 'static,
    ) -> Self
    where
        T: IntoPy<PyObject> + Send,
        E: Send,
        PyErr: From<E>,
        W: CoroutineWaker + Send + Sync + 'static,
    {
        qualname = qualname.or_else(|| name.clone());
        Self {
            name,
            qualname,
            implem: Box::new(CoroutineInner::<_, W, _, ALLOW_THREADS> {
                future: Some(future),
                waker: None,
                throw_callback,
            }),
        }
    }
}

pub(crate) fn iter_result(result: IterNextOutput<PyObject, PyObject>) -> PyResult<PyObject> {
    match result {
        IterNextOutput::Yield(ob) => Ok(ob),
        IterNextOutput::Return(ob) => Err(PyStopIteration::new_err(ob)),
    }
}

#[pymethods(crate = "crate")]
impl Coroutine {
    #[getter]
    fn __name__(&self) -> PyResult<Py<PyString>> {
        self.name
            .clone()
            .ok_or_else(|| PyAttributeError::new_err("__name__"))
    }

    #[getter]
    fn __qualname__(&self) -> PyResult<Py<PyString>> {
        self.qualname
            .clone()
            .ok_or_else(|| PyAttributeError::new_err("__qualname__"))
    }

    fn send(&mut self, py: Python<'_>, value: PyObject) -> PyResult<PyObject> {
        iter_result(self.implem.poll(py, Some(Ok(value)))?)
    }

    fn throw(&mut self, py: Python<'_>, exc: PyObject) -> PyResult<PyObject> {
        iter_result(self.implem.poll(py, Some(Err(exc)))?)
    }

    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        self.implem.poll(py, None)?;
        Ok(())
    }

    fn __await__(self_: Py<Self>) -> Py<Self> {
        self_
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<IterNextOutput<PyObject, PyObject>> {
        self.implem.poll(py, Some(Ok(py.None())))
    }
}

impl Drop for Coroutine {
    fn drop(&mut self) {
        let qualname = self.qualname.take();
        CoroutineImpl::drop(self.implem.as_mut(), qualname);
    }
}
