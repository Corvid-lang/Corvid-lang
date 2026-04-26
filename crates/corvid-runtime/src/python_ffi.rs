//! Python FFI bridge for native runtime hosts.
//!
//! This module is compiled only with the `python` feature. It keeps Python
//! interop behind a small JSON-like boundary so the interpreter/codegen layers
//! can share one runtime contract before richer typed wrappers land.

use crate::errors::RuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use serde_json::{Map, Number, Value};

#[derive(Debug, Clone, Default)]
pub struct PythonRuntime;

impl PythonRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn call_function(
        &self,
        module: &str,
        function: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        Python::with_gil(|py| {
            let result = (|| -> PyResult<Value> {
                let module_obj = py.import_bound(module)?;
                let function_obj = module_obj.getattr(function)?;
                let py_args = args
                    .iter()
                    .map(|arg| json_to_py(py, arg))
                    .collect::<PyResult<Vec<_>>>()?;
                let tuple = PyTuple::new_bound(py, py_args);
                let result = function_obj.call1(tuple)?;
                py_to_json(&result)
            })();
            result.map_err(|err| python_error(py, module, function, err))
        })
    }
}

fn json_to_py(py: Python<'_>, value: &Value) -> PyResult<PyObject> {
    Ok(match value {
        Value::Null => py.None(),
        Value::Bool(value) => value.into_py(py),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                value.into_py(py)
            } else if let Some(value) = value.as_u64() {
                value.into_py(py)
            } else if let Some(value) = value.as_f64() {
                value.into_py(py)
            } else {
                py.None()
            }
        }
        Value::String(value) => value.into_py(py),
        Value::Array(values) => {
            let items = values
                .iter()
                .map(|value| json_to_py(py, value))
                .collect::<PyResult<Vec<_>>>()?;
            PyList::new_bound(py, items).into_py(py)
        }
        Value::Object(values) => {
            let dict = PyDict::new_bound(py);
            for (key, value) in values {
                dict.set_item(key, json_to_py(py, value)?)?;
            }
            dict.into_py(py)
        }
    })
}

fn py_to_json(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    if value.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(value) = value.extract::<bool>() {
        return Ok(Value::Bool(value));
    }
    if let Ok(value) = value.extract::<i64>() {
        return Ok(Value::Number(value.into()));
    }
    if let Ok(value) = value.extract::<u64>() {
        return Ok(Value::Number(value.into()));
    }
    if let Ok(value) = value.extract::<f64>() {
        if let Some(number) = Number::from_f64(value) {
            return Ok(Value::Number(number));
        }
    }
    if let Ok(value) = value.extract::<String>() {
        return Ok(Value::String(value));
    }
    if let Ok(dict) = value.downcast::<PyDict>() {
        let mut object = Map::new();
        for (key, value) in dict.iter() {
            object.insert(key.extract::<String>()?, py_to_json(&value)?);
        }
        return Ok(Value::Object(object));
    }
    if let Ok(list) = value.downcast::<PyList>() {
        let mut values = Vec::with_capacity(list.len());
        for item in list.iter() {
            values.push(py_to_json(&item)?);
        }
        return Ok(Value::Array(values));
    }
    if let Ok(tuple) = value.downcast::<PyTuple>() {
        let mut values = Vec::with_capacity(tuple.len());
        for item in tuple.iter() {
            values.push(py_to_json(&item)?);
        }
        return Ok(Value::Array(values));
    }
    Ok(Value::String(value.str()?.to_string()))
}

fn python_error(
    py: Python<'_>,
    module: &str,
    function: &str,
    err: PyErr,
) -> RuntimeError {
    let traceback = format_python_error(py, &err).unwrap_or_else(|| err.to_string());
    RuntimeError::PythonFailed {
        module: module.to_string(),
        function: function.to_string(),
        traceback,
    }
}

fn format_python_error(py: Python<'_>, err: &PyErr) -> Option<String> {
    let traceback = py.import_bound("traceback").ok()?;
    let formatted = traceback
        .getattr("format_exception")
        .ok()?
        .call1((err.get_type_bound(py), err.value_bound(py), err.traceback_bound(py)))
        .ok()?;
    let lines = formatted.extract::<Vec<String>>().ok()?;
    Some(lines.join(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn calls_python_function_and_marshals_scalars() {
        let runtime = PythonRuntime::new();
        let value = runtime
            .call_function("math", "sqrt", &[json!(81.0)])
            .expect("python call");
        assert_eq!(value, json!(9.0));
    }

    #[test]
    fn marshals_python_dicts_and_lists() {
        let runtime = PythonRuntime::new();
        let value = runtime
            .call_function("json", "loads", &[json!(r#"{"items":[1,true,"x"]}"#)])
            .expect("python call");
        assert_eq!(value, json!({"items": [1, true, "x"]}));
    }

    #[test]
    fn preserves_python_exception_traceback() {
        let runtime = PythonRuntime::new();
        let err = runtime
            .call_function("math", "sqrt", &[json!(-1.0)])
            .expect_err("python error");
        match err {
            RuntimeError::PythonFailed { traceback, .. } => {
                assert!(traceback.contains("ValueError"), "{traceback}");
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
