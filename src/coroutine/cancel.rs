use crate::{ffi, IntoPy, Py, PyAny, PyErr, PyObject, PyResult, Python};
use futures_util::future::poll_fn;
use futures_util::task::AtomicWaker;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(Debug, Default)]
struct Inner {
    exception: AtomicPtr<ffi::PyObject>,
    waker: AtomicWaker,
}

/// Helper used to wait and retrieve exception thrown in [`Coroutine`](super::Coroutine).
#[derive(Debug, Default)]
pub struct CancelHandle(Arc<Inner>);

impl CancelHandle {
    /// Create a new `CoroutineCancel`.
    pub fn new() -> Self {
        Default::default()
    }

    /// Returns whether the associated coroutine has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        !self.0.exception.load(Ordering::Relaxed).is_null()
    }

    /// Poll to retrieve the exception thrown in the associated coroutine.
    pub fn poll_cancelled(&mut self, cx: &mut Context<'_>) -> Poll<Cancellation> {
        let take = || Cancellation(self.0.exception.swap(ptr::null_mut(), Ordering::Relaxed));
        if self.is_cancelled() {
            return Poll::Ready(take());
        }
        self.0.waker.register(cx.waker());
        if self.is_cancelled() {
            return Poll::Ready(take());
        }
        Poll::Pending
    }

    /// Retrieve the exception thrown in the associated coroutine.
    pub async fn cancelled(&mut self) -> Cancellation {
        poll_fn(|cx| self.poll_cancelled(cx)).await
    }

    /// Throw callback to be passed to coroutine constructor.
    ///
    /// When called, it marks the associated [`CancelHandler`] as cancelled,
    /// waking up tasks waiting on [`cancelled`](CancelHandle::cancelled).
    pub fn throw_callback(&self) -> impl Fn(&PyAny) -> PyResult<()> {
        let inner = self.0.clone();
        move |exc| {
            let ptr = inner.exception.swap(exc.into_ptr(), Ordering::Relaxed);
            // SAFETY: non-null pointers set in `self.0.exceptions` are valid owned pointers
            drop(unsafe { PyObject::from_owned_ptr_or_opt(exc.py(), ptr) });
            inner.waker.wake();
            Ok(())
        }
    }
}

/// Wrapper around the exception thrown in coroutine, allowing to retrieve it from [`CancelHandle`]
/// without requiring the GIL to be held
pub struct Cancellation(*mut ffi::PyObject);

impl IntoPy<PyObject> for Cancellation {
    fn into_py(self, py: Python<'_>) -> PyObject {
        // Cancellation is instantiated in `CancelHandle::poll_cancelled` by swapping
        // exception pointer, after checking it was non null; because `CancelHandle` cannot
        // be cloned, it's not possible for another `poll_cancelled` to happen concurrently
        // so the pointer must be non null
        // SAFETY: Cancellation is instantiated with valid owned pointer set in
        // `CancelHandle::throw_callback`
        unsafe { Py::from_owned_ptr(py, self.0) }
    }
}

impl From<Cancellation> for PyErr {
    fn from(value: Cancellation) -> Self {
        Python::with_gil(|gil| PyErr::from_value(value.into_py(gil).as_ref(gil)))
    }
}
