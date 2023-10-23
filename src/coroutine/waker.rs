use crate::coroutine::Coroutine;
use crate::exceptions::PyStopIteration;
use crate::pyclass::IterNextOutput;
use crate::sync::GILOnceCell;
use crate::types::{PyFuture, PyString};
use crate::{intern, IntoPy, Py, PyAny, PyErr, PyObject, PyResult, Python};
use std::cell::Cell;
use std::future::Future;
use std::sync::Arc;
use std::task::{Poll, Wake};

const MIXED_AWAITABLE_AND_FUTURE_ERROR: &str = "Python awaitable mixed with Rust future";

pub(crate) enum FutureOrPoll {
    Future(Py<PyFuture>),
    Poll(Poll<PyResult<PyObject>>),
}

thread_local! {
    pub(crate) static FUTURE_OR_POLL: Cell<Option<FutureOrPoll>> = Cell::new(None);
}

/// Waker implementation used in [`Coroutine`].
///
/// It is used when the Rust future polling returns `Poll::Pending`.
pub trait CoroutineWaker: Sized {
    /// Create a waker for the current Python task.
    fn new(py: Python<'_>) -> PyResult<Self>;
    /// Yield an object at the coroutine suspension point.
    fn yield_(&self, py: Python<'_>) -> PyResult<PyObject>;
    /// Yield an object in the case the future is pending,
    /// but the waker has been called while polling.
    ///
    /// This is roughly equivalent to `sleep(0)`.
    fn yield_waken(py: Python<'_>) -> PyResult<PyObject>;
    /// Wake the Python task suspended on this waker.
    fn wake(&self, py: Python<'_>) -> PyResult<()>;
}

/// Extension trait for [`CoroutineWaker`] to build coroutine with an easier interface.
pub trait CoroutineWakerExt: CoroutineWaker {
    /// Build a coroutine using the waker implementation.
    fn coroutine<T, E>(
        name: Option<Py<PyString>>,
        qualname: Option<Py<PyString>>,
        future: impl Future<Output = Result<T, E>> + Send + 'static,
        allow_threads: bool,
        throw_callback: impl Fn(&PyAny) -> PyResult<()> + Send + 'static,
    ) -> Coroutine
    where
        T: IntoPy<PyObject> + Send,
        E: Send,
        PyErr: From<E>;
}

impl<W> CoroutineWakerExt for W
where
    W: CoroutineWaker + Send + Sync + 'static,
{
    fn coroutine<T, E>(
        name: Option<Py<PyString>>,
        qualname: Option<Py<PyString>>,
        future: impl Future<Output = Result<T, E>> + Send + 'static,
        allow_threads: bool,
        throw_callback: impl Fn(&PyAny) -> PyResult<()> + Send + 'static,
    ) -> Coroutine
    where
        T: IntoPy<PyObject> + Send,
        E: Send,
        PyErr: From<E>,
    {
        if allow_threads {
            Coroutine::new::<_, _, Self, true>(name, qualname, future, throw_callback)
        } else {
            Coroutine::new::<_, _, Self, false>(name, qualname, future, throw_callback)
        }
    }
}

enum State<W> {
    Waken,
    Yielded(PyObject),
    Pending(W),
}

pub(super) struct Waker<W> {
    state: GILOnceCell<State<W>>,
    prev_result: Option<Result<PyObject, PyObject>>,
}

impl<W> Waker<W> {
    pub(super) fn new(prev_result: Option<Result<PyObject, PyObject>>) -> Self {
        Self {
            state: GILOnceCell::new(),
            prev_result,
        }
    }

    pub(super) fn reset(&mut self, prev_result: Option<Result<PyObject, PyObject>>) {
        self.state.take();
        self.prev_result = prev_result;
    }

    pub(super) fn yielded_from_awaitable(&self, py: Python<'_>) -> bool {
        matches!(self.state.get(py), Some(State::Yielded(_)))
    }
}

impl<W> Waker<W>
where
    W: CoroutineWaker,
{
    pub(super) fn yield_(&self, py: Python<'_>) -> PyResult<PyObject> {
        let init = || PyResult::Ok(State::Pending(W::new(py)?));
        let state = self.state.get_or_try_init(py, init)?;
        match state {
            State::Waken => W::yield_waken(py),
            State::Yielded(obj) => Ok(obj.clone()),
            State::Pending(waker) => waker.yield_(py),
        }
    }
}

impl<W> Wake for Waker<W>
where
    W: CoroutineWaker,
{
    fn wake(self: Arc<Self>) {
        self.wake_by_ref()
    }

    fn wake_by_ref(self: &Arc<Self>) {
        Python::with_gil(|gil| match FUTURE_OR_POLL.take() {
            Some(FutureOrPoll::Future(fut)) => FUTURE_OR_POLL.set(Some(FutureOrPoll::Poll(
                match fut.as_ref(gil).next(&self.prev_result) {
                    Ok(IterNextOutput::Return(ret)) => Poll::Ready(Ok(ret.into())),
                    Ok(IterNextOutput::Yield(yielded)) => {
                        if self.state.set(gil, State::Yielded(yielded.into())).is_err() {
                            panic!("{}", MIXED_AWAITABLE_AND_FUTURE_ERROR)
                        }
                        Poll::Pending
                    }
                    Err(err) if err.is_instance_of::<PyStopIteration>(gil) => Poll::Ready(
                        err.value(gil)
                            .getattr(intern!(gil, "value"))
                            .map(Into::into),
                    ),
                    Err(err) => Poll::Ready(Err(err)),
                },
            ))),
            Some(FutureOrPoll::Poll(_)) => unreachable!(),
            None => match self.state.get_or_init(gil, || State::Waken) {
                State::Waken => {}
                State::Yielded(_) => {
                    println!("{}", std::backtrace::Backtrace::force_capture());
                    panic!("{}", MIXED_AWAITABLE_AND_FUTURE_ERROR)
                }
                State::Pending(waker) => waker.wake(gil).expect("wake error"),
            },
        })
    }
}
