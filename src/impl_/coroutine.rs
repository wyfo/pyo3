use std::future::Future;
use std::mem;
use std::ops::{Deref, DerefMut};

use crate::pyclass::boolean_struct::False;
use crate::types::{PyModule, PyString};
use crate::{Py, PyAny, PyClass, PyRef, PyRefMut, PyResult, Python};

pub fn coroutine_name(py: Python<'_>, name: &str) -> Py<PyString> {
    PyString::new(py, name).into()
}

pub unsafe fn coroutine_qualname(
    py: Python<'_>,
    module: Option<&PyModule>,
    name: &str,
) -> Option<Py<PyString>> {
    Some(PyString::new(py, &format!("{}.{name}", module?.name().ok()?)).into())
}

pub fn method_coroutine_qualname<T: PyClass>(py: Python<'_>, name: &str) -> Py<PyString> {
    let class = T::NAME;
    let qualname = match T::MODULE {
        Some(module) => format!("{module}.{class}.{name}"),
        None => format!("{class}.{name}"),
    };
    PyString::new(py, &qualname).into()
}

struct RefGuard<T: PyClass>(Py<T>);

impl<T: PyClass> Drop for RefGuard<T> {
    fn drop(&mut self) {
        Python::with_gil(|gil| self.0.as_ref(gil).release_ref())
    }
}

pub unsafe fn ref_method_future<T: PyClass, F: Future>(
    self_: &PyAny,
    fut: impl FnOnce(&'static T) -> F,
) -> PyResult<impl Future<Output = F::Output>> {
    let ref_: PyRef<'_, T> = self_.extract()?;
    let guard = RefGuard(unsafe { Py::<T>::from_borrowed_ptr(self_.py(), ref_.as_ptr()) });
    let static_ref = unsafe { mem::transmute(ref_.deref()) };
    mem::forget(ref_);
    Ok(async move {
        let _guard = guard;
        fut(static_ref).await
    })
}

struct RefMutGuard<T: PyClass>(Py<T>);

impl<T: PyClass> Drop for RefMutGuard<T> {
    fn drop(&mut self) {
        Python::with_gil(|gil| self.0.as_ref(gil).release_mut())
    }
}

pub fn mut_method_future<T: PyClass<Frozen = False>, F: Future>(
    self_: &PyAny,
    fut: impl FnOnce(&'static mut T) -> F,
) -> PyResult<impl Future<Output = F::Output>> {
    let mut mut_: PyRefMut<'_, T> = self_.extract()?;
    let guard = RefMutGuard(unsafe { Py::<T>::from_borrowed_ptr(self_.py(), mut_.as_ptr()) });
    let static_mut = unsafe { mem::transmute(mut_.deref_mut()) };
    mem::forget(mut_);
    Ok(async move {
        let _guard = guard;
        fut(static_mut).await
    })
}
