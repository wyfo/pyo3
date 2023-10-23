#![cfg(feature = "macros")]
use std::{ops::Deref, sync::Arc, task::Poll, thread, time::Duration};

use futures::{channel::oneshot, future::poll_fn, FutureExt};
use pyo3::types::PyType;
use pyo3::{
    coroutine::{asyncio, raise_on_throw, CancelHandle, CoroutineWakerExt},
    prelude::*,
    py_run,
    sync::GILOnceCell,
    types::IntoPyDict,
};

#[path = "../src/tests/common.rs"]
mod common;

#[test]
fn noop_coroutine() {
    #[pyfunction]
    async fn noop() -> usize {
        42
    }
    Python::with_gil(|gil| {
        let noop = wrap_pyfunction!(noop, gil).unwrap();
        let test = "import asyncio; assert asyncio.run(noop()) == 42";
        py_run!(gil, noop, test);
    })
}

#[test]
fn test_coroutine_qualname() {
    #[pyfunction]
    async fn my_fn() {}
    #[pyclass]
    struct MyClass;
    #[pymethods]
    impl MyClass {
        #[new]
        fn new() -> Self {
            Self
        }
        async fn my_method(&self) {}
        #[classmethod]
        async fn my_classmethod(_cls: Py<PyType>) {}
        #[staticmethod]
        async fn my_staticmethod() {}
    }
    #[pyclass(module = "my_module")]
    struct MyClassWithModule;
    #[pymethods]
    impl MyClassWithModule {
        #[new]
        fn new() -> Self {
            Self
        }
        async fn my_method(&self) {}
        #[classmethod]
        async fn my_classmethod(_cls: Py<PyType>) {}
        #[staticmethod]
        async fn my_staticmethod() {}
    }
    Python::with_gil(|gil| {
        let test = r#"
        for coro, name, qualname in [
            (my_fn(), "my_fn", "my_fn"),
            (my_fn_with_module(), "my_fn", "my_module.my_fn"),
            (MyClass().my_method(), "my_method", "MyClass.my_method"),
            (MyClass().my_classmethod(), "my_classmethod", "MyClass.my_classmethod"),
            (MyClass.my_staticmethod(), "my_staticmethod", "MyClass.my_staticmethod"),
            (MyClassWithModule().my_method(), "my_method", "my_module.MyClassWithModule.my_method"),
            (MyClassWithModule().my_classmethod(), "my_classmethod", "my_module.MyClassWithModule.my_classmethod"),
            (MyClassWithModule.my_staticmethod(), "my_staticmethod", "my_module.MyClassWithModule.my_staticmethod"),
        ]:
            assert coro.__name__ == name and coro.__qualname__ == qualname
        "#;
        let my_module = PyModule::new(gil, "my_module").unwrap();
        let locals = [
            ("my_fn", wrap_pyfunction!(my_fn, gil).unwrap().deref()),
            (
                "my_fn_with_module",
                wrap_pyfunction!(my_fn, my_module).unwrap(),
            ),
            ("MyClass", gil.get_type::<MyClass>()),
            ("MyClassWithModule", gil.get_type::<MyClassWithModule>()),
        ]
        .into_py_dict(gil);
        py_run!(gil, *locals, test);
    })
}

#[test]
fn sleep_0_like_coroutine() {
    #[pyfunction]
    async fn sleep_0() -> usize {
        let mut waken = false;
        poll_fn(|cx| {
            if !waken {
                cx.waker().wake_by_ref();
                waken = true;
                return Poll::Pending;
            }
            Poll::Ready(42)
        })
        .await
    }
    Python::with_gil(|gil| {
        let sleep_0 = wrap_pyfunction!(sleep_0, gil).unwrap();
        let test = "import asyncio; assert asyncio.run(sleep_0()) == 42";
        py_run!(gil, sleep_0, test);
    })
}

#[pyfunction]
async fn sleep(seconds: f64) -> usize {
    let (tx, rx) = oneshot::channel();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs_f64(seconds));
        tx.send(42).unwrap();
    });
    rx.await.unwrap()
}

#[test]
fn sleep_coroutine() {
    Python::with_gil(|gil| {
        let sleep = wrap_pyfunction!(sleep, gil).unwrap();
        let test = "import asyncio; assert asyncio.run(sleep(0.001)) == 42";
        py_run!(gil, sleep, test);
    })
}

#[test]
fn cancelled_coroutine() {
    Python::with_gil(|gil| {
        let sleep = wrap_pyfunction!(sleep, gil).unwrap();
        let test = r#"
        import asyncio;
        async def main():
            task = asyncio.create_task(sleep(1))
            await asyncio.sleep(0)
            task.cancel()
            await task
        asyncio.run(main())
        "#;
        let globals = gil.import("__main__").unwrap().dict();
        globals.set_item("sleep", sleep).unwrap();
        let err = gil
            .run(&pyo3::unindent::unindent(test), Some(globals), None)
            .unwrap_err();
        assert_eq!(err.value(gil).get_type().name().unwrap(), "CancelledError");
    })
}

#[test]
fn coroutine_cancel() {
    #[pyfunction(cancel_handle = "cancel")]
    async fn cancellable_sleep(seconds: f64, mut cancel: CancelHandle) -> usize {
        futures::select! {
            _ = sleep(seconds).fuse() => 42,
            _ = cancel.cancelled().fuse() => 0,
        }
    }
    Python::with_gil(|gil| {
        let cancellable_sleep = wrap_pyfunction!(cancellable_sleep, gil).unwrap();
        let test = r#"
        import asyncio;
        async def main():
            task = asyncio.create_task(cancellable_sleep(1))
            await asyncio.sleep(0)
            task.cancel()
            return await task
        assert asyncio.run(main()) == 0
        "#;
        let globals = gil.import("__main__").unwrap().dict();
        globals
            .set_item("cancellable_sleep", cancellable_sleep)
            .unwrap();
        gil.run(&pyo3::unindent::unindent(test), Some(globals), None)
            .unwrap();
    })
}

#[test]
fn multi_thread_event_loop() {
    Python::with_gil(|gil| {
        let sleep = wrap_pyfunction!(sleep, gil).unwrap();
        let test = r#"
        import asyncio
        import threading
        loop = asyncio.new_event_loop()
        # spawn the sleep task and run just one iteration of the event loop
        # to schedule the sleep wakeup
        task = loop.create_task(sleep(0.1))
        loop.stop()
        loop.run_forever()
        assert not task.done()
        # spawn a thread to complete the execution of the sleep task
        def target(loop, task):
            loop.run_until_complete(task)
        thread = threading.Thread(target=target, args=(loop, task))
        thread.start()
        thread.join()
        assert task.result() == 42
        "#;
        py_run!(gil, sleep, test);
    })
}

#[test]
fn closed_event_loop() {
    let waker = Arc::new(GILOnceCell::new());
    let waker2 = waker.clone();
    let future = poll_fn(move |cx| {
        Python::with_gil(|gil| waker2.set(gil, cx.waker().clone()).unwrap());
        Poll::Pending::<PyResult<()>>
    });
    Python::with_gil(|gil| {
        let register_waker =
            asyncio::Waker::coroutine(None, None, future, false, raise_on_throw).into_py(gil);
        let test = r#"
        import asyncio
        loop = asyncio.new_event_loop()
        # register a waker by spawning a task and polling it once, then close the loop
        task = loop.create_task(register_waker)
        loop.stop()
        loop.run_forever()
        loop.close()
        "#;
        py_run!(gil, register_waker, test);
        Python::with_gil(|gil| waker.get(gil).unwrap().wake_by_ref())
    })
}

#[test]
fn coroutine_with_receiver() {
    #[pyclass]
    struct MyClass;
    #[pymethods]
    impl MyClass {
        #[new]
        fn new() -> Self {
            Self
        }
        async fn my_method(&self) {}
        async fn my_mut_method(&mut self) {}
    }
    Python::with_gil(|gil| {
        let test = r#"
        obj = MyClass()
        coro1 = obj.my_method()
        coro2 = obj.my_method()
        try:
            assert obj.my_mut_method() == 42, "borrow checking should fail"
        except RuntimeError as err:
            pass
        coro1.close()
        coro2.close()
        coro3 = obj.my_mut_method()
        try:
            assert obj.my_mut_method() == 42, "borrow checking should fail"
        except RuntimeError as err:
            pass
        try:
            assert obj.my_method() == 42, "borrow checking should fail"
        except RuntimeError as err:
            pass
        "#;
        let locals = [("MyClass", gil.get_type::<MyClass>())].into_py_dict(gil);
        py_run!(gil, *locals, test);
    })
}
