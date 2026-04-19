#![allow(missing_docs)]

use pyo3::PyErr;

pub(crate) fn platform_error(err: helm_platform::PlatformError) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(err.to_string())
}

pub(crate) fn report_error(err: helm_report::SinkError) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(err.to_string())
}

pub(crate) fn debug_error(err: helm_debug::DebugError) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(err.to_string())
}
