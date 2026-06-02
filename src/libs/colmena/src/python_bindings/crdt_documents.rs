//! PyO3 bindings for the v1 CRDT documents feature.
//!
//! Exposes a `colmena.documents` Python submodule (NOTE: name "documents"
//! not "crdt_documents" — the existing legacy `documents/` module is not
//! exposed to Python, so the short name is unambiguous from the operator's
//! point of view).
//!
//! Runtime is a lazy OnceCell singleton built from the
//! `COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT` env var (default
//! `.colmena/crdt_documents`). All sync entry points block on the current
//! tokio runtime via `tokio::runtime::Handle::try_current()` — callers must
//! run inside a tokio runtime (e.g. via `maturin develop` + a pytest fixture
//! that sets up a runtime, or the `colmena` CLI which is tokio-based).

use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use once_cell::sync::OnceCell;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::sync::Arc;

static RUNTIME: OnceCell<Arc<CrdtDocumentsRuntime>> = OnceCell::new();

fn runtime() -> PyResult<Arc<CrdtDocumentsRuntime>> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt.clone());
    }
    let storage_root = std::env::var("COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT")
        .unwrap_or_else(|_| ".colmena/crdt_documents".to_string());
    let cfg = serde_json::json!({ "storage_root": storage_root });
    let handle = tokio::runtime::Handle::try_current().map_err(|_| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "no tokio runtime available — colmena.documents requires the calling \
             Python context to have an active tokio runtime",
        )
    })?;
    let built = handle
        .block_on(CrdtDocumentsRuntime::from_config(&cfg))
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    let arc = Arc::new(built);
    let _ = RUNTIME.set(arc.clone());
    Ok(arc)
}

fn parse_id(s: &str) -> PyResult<ArtifactId> {
    s.parse::<ArtifactId>().map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("invalid artifact_id: {e}"))
    })
}

#[pyfunction]
#[allow(deprecated)]
fn list_sheets(py: Python<'_>, artifact_id: &str) -> PyResult<PyObject> {
    let rt = runtime()?;
    let id = parse_id(artifact_id)?;
    let Some(entry) = rt.registry.get(&id) else {
        return Err(pyo3::exceptions::PyKeyError::new_err("artifact not found"));
    };
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let out = PyList::empty(py);
    for s in proj["sheets"].as_array().cloned().unwrap_or_default() {
        let d = PyDict::new(py);
        d.set_item("sheet_id", s["id"].as_str().unwrap_or(""))?;
        d.set_item("name", s["name"].as_str().unwrap_or(""))?;
        out.append(d)?;
    }
    Ok(out.into_py(py))
}

#[pyfunction]
#[allow(deprecated)]
fn read_sheet(py: Python<'_>, artifact_id: &str, sheet_id: &str) -> PyResult<PyObject> {
    let rt = runtime()?;
    let id = parse_id(artifact_id)?;
    let Some(entry) = rt.registry.get(&id) else {
        return Err(pyo3::exceptions::PyKeyError::new_err("artifact not found"));
    };
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let Some(sheet) = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s["id"].as_str() == Some(sheet_id))
    else {
        return Err(pyo3::exceptions::PyKeyError::new_err("sheet not found"));
    };
    let cells = sheet["cells"].as_object().cloned().unwrap_or_default();
    let d = PyDict::new(py);
    for (addr, v) in cells {
        let py_val: PyObject = match v {
            serde_json::Value::String(s) => s.into_py(py),
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0).into_py(py),
            serde_json::Value::Bool(b) => b.into_py(py),
            _ => py.None(),
        };
        d.set_item(addr, py_val)?;
    }
    Ok(d.into_py(py))
}

/// Register the `documents` submodule on the parent `colmena` module.
#[allow(deprecated)]
pub fn register(parent: &PyModule) -> PyResult<()> {
    let py = parent.py();
    let m = PyModule::new(py, "documents")?;
    m.add_function(wrap_pyfunction!(list_sheets, m)?)?;
    m.add_function(wrap_pyfunction!(read_sheet, m)?)?;
    parent.add_submodule(m)?;
    Ok(())
}
