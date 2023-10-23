#![cfg(feature = "macros")]

use std::task::Poll;

use futures_util::{future::poll_fn, FutureExt};
use pyo3::{prelude::*, py_run, types::PyFuture};

#[path = "../src/tests/common.rs"]
mod common;

#[pyfunction]
async fn wrap_awaitable(awaitable: PyObject) -> PyResult<PyObject> {
    let future: Py<PyFuture> = Python::with_gil(|gil| Py::from_object(gil, awaitable))?;
    future.await
}

#[test]
fn awaitable() {
    Python::with_gil(|gil| {
        let wrap_awaitable = wrap_pyfunction!(wrap_awaitable, gil).unwrap();
        let test = r#"
        import asyncio;
        async def main():
            return await wrap_awaitable(asyncio.sleep(0.001, 42))
        assert asyncio.run(main()) == 42
        "#;
        let globals = gil.import("__main__").unwrap().dict();
        globals.set_item("wrap_awaitable", wrap_awaitable).unwrap();
        gil.run(&pyo3::unindent::unindent(test), Some(globals), None)
            .unwrap();
    })
}

#[test]
#[should_panic(expected = "object int can't be used in 'await' expression")]
fn pyfuture_not_awaitable() {
    Python::with_gil(|gil| {
        let wrap_awaitable = wrap_pyfunction!(wrap_awaitable, gil).unwrap();
        let test = r#"
        import asyncio;
        async def main():
            return await wrap_awaitable(42)
        asyncio.run(main())
        "#;
        let globals = gil.import("__main__").unwrap().dict();
        globals.set_item("wrap_awaitable", wrap_awaitable).unwrap();
        gil.run(&pyo3::unindent::unindent(test), Some(globals), None)
            .unwrap();
    })
}

#[test]
#[should_panic(expected = "PyFuture must be awaited in coroutine context")]
fn pyfuture_without_coroutine() {
    #[pyfunction]
    fn block_on(awaitable: PyObject) -> PyResult<PyObject> {
        let future: Py<PyFuture> = Python::with_gil(|gil| Py::from_object(gil, awaitable))?;
        futures::executor::block_on(future)
    }
    Python::with_gil(|gil| {
        let block_on = wrap_pyfunction!(block_on, gil).unwrap();
        let test = r#"
        async def coro():
            ...
        block_on(coro())
        "#;
        py_run!(gil, block_on, test);
    })
}

async fn checkpoint() {
    let mut ready = false;
    poll_fn(|cx| {
        if ready {
            return Poll::Ready(());
        }
        ready = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await
}

#[test]
#[should_panic(expected = "Python awaitable mixed with Rust future")]
fn pyfuture_in_select() {
    #[pyfunction]
    async fn select(awaitable: PyObject) -> PyResult<PyObject> {
        let future: Py<PyFuture> = Python::with_gil(|gil| Py::from_object(gil, awaitable))?;
        futures::select_biased! {
            _ = checkpoint().fuse() => unreachable!(),
            res = future.fuse() => res,
        }
    }
    Python::with_gil(|gil| {
        let select = wrap_pyfunction!(select, gil).unwrap();
        let test = r#"
        import asyncio;
        async def main():
            return await select(asyncio.sleep(1))
        asyncio.run(main())
        "#;
        let globals = gil.import("__main__").unwrap().dict();
        globals.set_item("select", select).unwrap();
        gil.run(&pyo3::unindent::unindent(test), Some(globals), None)
            .unwrap();
    })
}

#[test]
#[should_panic(expected = "Python awaitable mixed with Rust future")]
fn pyfuture_in_select2() {
    #[pyfunction]
    async fn select2(awaitable: PyObject) -> PyResult<PyObject> {
        let future: Py<PyFuture> = Python::with_gil(|gil| Py::from_object(gil, awaitable))?;
        futures::select_biased! {
            res = future.fuse() => res,
            _ = checkpoint().fuse() => unreachable!(),
        }
    }
    Python::with_gil(|gil| {
        let select2 = wrap_pyfunction!(select2, gil).unwrap();
        let test = r#"
        import asyncio;
        async def main():
            return await select2(asyncio.sleep(1))
        asyncio.run(main())
        "#;
        let globals = gil.import("__main__").unwrap().dict();
        globals.set_item("select2", select2).unwrap();
        gil.run(&pyo3::unindent::unindent(test), Some(globals), None)
            .unwrap();
    })
}
