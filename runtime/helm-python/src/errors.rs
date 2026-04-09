#![allow(missing_docs)]

use pyo3::PyErr;

pub(crate) fn platform_error(err: helm_platform::PlatformError) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(err.to_string())
}
