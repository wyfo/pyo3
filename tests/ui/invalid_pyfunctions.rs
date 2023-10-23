use pyo3::{coroutine::CancelHandle, prelude::*};

#[pyfunction]
fn generic_function<T>(value: T) {}

#[pyfunction]
fn impl_trait_function(impl_trait: impl AsRef<PyAny>) {}

#[pyfunction]
fn wildcard_argument(_: i32) {}

#[pyfunction]
fn destructured_argument((a, b): (i32, i32)) {}

#[pyfunction]
fn function_with_required_after_option(_opt: Option<i32>, _x: i32) {}

#[pyfunction(cancel_handle = "cancel")]
fn sync_fn_with_cancel_handle(cancel: CancelHandle) {}

#[pyfunction(cancel_handle = "cancel")]
async fn missing_cancel_handle() {}

fn main() {}
