//! PyO3 native Python bindings for `httpx-rs`.

pub mod asynchronous;
pub mod sync;

use crate::auth::{AuthMethod, BasicAuth, BearerAuth, DigestAuth};
use crate::exceptions::{register_exceptions, HttpxError, HTTPStatusError};
use crate::middleware::TimeoutConfig;
use crate::proxy::ProxyMounts;
use crate::request::{
    BoxByteStream, CookieJar, Headers, MultipartEncoder, MultipartFile, Request, RequestBuilder,
};
use crate::response::Response;
use crate::transport::{ClientCertConfig, PoolLimits, TlsVerify};
use bytes::Bytes;
use futures_util::stream;
use pyo3::exceptions::{PyKeyError, PyStopAsyncIteration, PyStopIteration, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// Case-insensitive multi-value HTTP headers exposed to Python.
#[pyclass(name = "Headers", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyHeaders {
    pub inner: Headers,
}

#[pymethods]
impl PyHeaders {
    #[new]
    #[pyo3(signature = (headers=None, encoding="utf-8"))]
    fn new(headers: Option<&Bound<'_, PyAny>>, encoding: &str) -> PyResult<Self> {
        let _ = encoding;
        let inner = match headers {
            Some(obj) if !obj.is_none() => extract_headers(obj)?,
            _ => Headers::new(),
        };
        Ok(Self { inner })
    }

    #[pyo3(signature = (key, default=None))]
    fn get(&self, key: &str, default: Option<String>) -> Option<String> {
        self.inner.get(key).or(default)
    }

    #[pyo3(signature = (key, split_commas=false))]
    fn get_list(&self, key: &str, split_commas: bool) -> Vec<String> {
        self.inner.get_list(key, split_commas)
    }

    fn keys(&self) -> Vec<String> {
        self.inner.items().into_iter().map(|(k, _)| k).collect()
    }

    fn values(&self) -> Vec<String> {
        self.inner.items().into_iter().map(|(_, v)| v).collect()
    }

    fn items(&self) -> Vec<(String, String)> {
        self.inner.items()
    }

    fn multi_items(&self) -> Vec<(String, String)> {
        self.inner.multi_items()
    }

    fn update(&mut self, headers: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = extract_headers(headers)?;
        self.inner.update(&parsed);
        Ok(())
    }

    fn copy(&self) -> Self {
        self.clone()
    }

    fn __getitem__(&self, key: &str) -> PyResult<String> {
        self.inner
            .get(key)
            .ok_or_else(|| PyKeyError::new_err(key.to_string()))
    }

    fn __setitem__(&mut self, key: &str, value: &str) -> PyResult<()> {
        self.inner.insert(key, value).map_err(PyErr::from)
    }

    fn __delitem__(&mut self, key: &str) -> PyResult<()> {
        if self.inner.remove(key) {
            Ok(())
        } else {
            Err(PyKeyError::new_err(key.to_string()))
        }
    }

    fn __contains__(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }

    fn __len__(&self) -> usize {
        self.inner.items().len()
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<PyStringVecIter>> {
        let keys = slf.keys();
        Py::new(slf.py(), PyStringVecIter { items: keys, idx: 0 })
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        if let Ok(other_hdrs) = extract_headers(other) {
            self.inner.items() == other_hdrs.items()
        } else {
            false
        }
    }

    fn __repr__(&self) -> String {
        format!("Headers({:?})", self.inner.multi_items())
    }
}

#[pyclass(module = "httpx")]
struct PyStringVecIter {
    items: Vec<String>,
    idx: usize,
}

#[pymethods]
impl PyStringVecIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<String> {
        if slf.idx < slf.items.len() {
            let val = slf.items[slf.idx].clone();
            slf.idx += 1;
            Some(val)
        } else {
            None
        }
    }
}

/// Cookie container compatible with HTTPX's `Cookies`.
#[pyclass(name = "Cookies", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyCookies {
    pub inner: CookieJar,
}

#[pymethods]
impl PyCookies {
    #[new]
    #[pyo3(signature = (cookies=None))]
    fn new(cookies: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let jar = match cookies {
            Some(obj) if !obj.is_none() => extract_cookie_jar(obj)?,
            _ => CookieJar::new(),
        };
        Ok(Self { inner: jar })
    }

    #[pyo3(signature = (name, value, domain=None, path="/"))]
    fn set(&self, name: &str, value: &str, domain: Option<&str>, path: Option<&str>) {
        self.inner.set(name, value, domain, path);
    }

    #[pyo3(signature = (name, default=None, domain=None, path=None))]
    fn get(
        &self,
        name: &str,
        default: Option<String>,
        domain: Option<&str>,
        path: Option<&str>,
    ) -> Option<String> {
        self.inner.get(name, domain, path).or(default)
    }

    #[pyo3(signature = (name, domain=None, path=None))]
    fn delete(&self, name: &str, domain: Option<&str>, path: Option<&str>) {
        self.inner.delete(name, domain, path);
    }

    #[pyo3(signature = (domain=None, path=None))]
    fn clear(&self, domain: Option<&str>, path: Option<&str>) {
        let _ = (domain, path);
        self.inner.clear();
    }

    fn items(&self) -> Vec<(String, String)> {
        self.inner.items()
    }

    fn keys(&self) -> Vec<String> {
        self.inner.items().into_iter().map(|(k, _)| k).collect()
    }

    fn values(&self) -> Vec<String> {
        self.inner.items().into_iter().map(|(_, v)| v).collect()
    }

    fn __getitem__(&self, key: &str) -> PyResult<String> {
        self.inner
            .get(key, None, None)
            .ok_or_else(|| PyKeyError::new_err(key.to_string()))
    }

    fn __setitem__(&self, key: &str, value: &str) {
        self.inner.set(key, value, None, None);
    }

    fn __delitem__(&self, key: &str) {
        self.inner.delete(key, None, None);
    }

    fn __contains__(&self, key: &str) -> bool {
        self.inner.get(key, None, None).is_some()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<PyStringVecIter>> {
        let keys = slf.keys();
        Py::new(slf.py(), PyStringVecIter { items: keys, idx: 0 })
    }

    fn __repr__(&self) -> String {
        format!("Cookies({:?})", self.inner.items())
    }
}

/// Parsed URL wrapper matching HTTPX's `URL`.
#[pyclass(name = "URL", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyURL {
    pub inner: Url,
}

#[pymethods]
impl PyURL {
    #[new]
    fn new(url: &str) -> PyResult<Self> {
        let parsed = Url::parse(url)
            .map_err(|e| PyErr::from(HttpxError::InvalidUrl(format!("Invalid URL '{url}': {e}"))))?;
        Ok(Self { inner: parsed })
    }

    #[getter]
    fn scheme(&self) -> &str {
        self.inner.scheme()
    }

    #[getter]
    fn host(&self) -> &str {
        self.inner.host_str().unwrap_or("")
    }

    #[getter]
    fn port(&self) -> Option<u16> {
        self.inner.port()
    }

    #[getter]
    fn path(&self) -> &str {
        self.inner.path()
    }

    #[getter]
    fn raw_path<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let raw = match self.inner.query() {
            Some(q) => format!("{}?{q}", self.inner.path()),
            None => self.inner.path().to_string(),
        };
        PyBytes::new_bound(py, raw.as_bytes())
    }

    #[getter]
    fn query<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new_bound(py, self.inner.query().unwrap_or("").as_bytes())
    }

    #[getter]
    fn fragment(&self) -> &str {
        self.inner.fragment().unwrap_or("")
    }

    #[getter]
    fn username(&self) -> &str {
        self.inner.username()
    }

    #[getter]
    fn password(&self) -> Option<&str> {
        self.inner.password()
    }

    #[getter]
    fn netloc<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let netloc = match self.inner.port() {
            Some(p) => format!("{}:{p}", self.host()),
            None => self.host().to_string(),
        };
        PyBytes::new_bound(py, netloc.as_bytes())
    }

    fn join(&self, url: &str) -> PyResult<Self> {
        let joined = self.inner.join(url).map_err(|e| {
            PyErr::from(HttpxError::InvalidUrl(format!(
                "Failed to join '{url}' onto '{}': {e}",
                self.inner
            )))
        })?;
        Ok(Self { inner: joined })
    }

    fn __str__(&self) -> &str {
        self.inner.as_str()
    }

    fn __repr__(&self) -> String {
        format!("URL('{}')", self.inner.as_str())
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        if let Ok(other_url) = other.extract::<PyRef<'_, PyURL>>() {
            self.inner == other_url.inner
        } else if let Ok(s) = other.extract::<String>() {
            self.inner.as_str() == s
        } else {
            false
        }
    }
}

/// Timeout configuration matching HTTPX's `Timeout`.
#[pyclass(name = "Timeout", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyTimeout {
    pub inner: TimeoutConfig,
}

#[pymethods]
impl PyTimeout {
    #[new]
    #[pyo3(signature = (timeout=5.0, *, connect=None, read=None, write=None, pool=None))]
    fn new(
        timeout: Option<f64>,
        connect: Option<Option<f64>>,
        read: Option<Option<f64>>,
        write: Option<Option<f64>>,
        pool: Option<Option<f64>>,
    ) -> PyResult<Self> {
        let inner = TimeoutConfig::from_secs(timeout, connect, read, write, pool)?;
        Ok(Self { inner })
    }

    #[getter]
    fn connect(&self) -> Option<f64> {
        self.inner.connect.map(|d| d.as_secs_f64())
    }

    #[getter]
    fn read(&self) -> Option<f64> {
        self.inner.read.map(|d| d.as_secs_f64())
    }

    #[getter]
    fn write(&self) -> Option<f64> {
        self.inner.write.map(|d| d.as_secs_f64())
    }

    #[getter]
    fn pool(&self) -> Option<f64> {
        self.inner.pool.map(|d| d.as_secs_f64())
    }

    fn __repr__(&self) -> String {
        format!(
            "Timeout(connect={:?}, read={:?}, write={:?}, pool={:?})",
            self.connect(),
            self.read(),
            self.write(),
            self.pool()
        )
    }
}

/// Connection pool limits matching HTTPX's `Limits`.
#[pyclass(name = "Limits", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyLimits {
    pub inner: PoolLimits,
}

#[pymethods]
impl PyLimits {
    #[new]
    #[pyo3(signature = (*, max_connections=100, max_keepalive_connections=20, keepalive_expiry=5.0))]
    fn new(
        max_connections: Option<usize>,
        max_keepalive_connections: Option<usize>,
        keepalive_expiry: Option<f64>,
    ) -> PyResult<Self> {
        let expiry = match keepalive_expiry {
            None => None,
            Some(v) if v < 0.0 || v.is_nan() => {
                return Err(PyValueError::new_err("keepalive_expiry must be non-negative"))
            }
            Some(v) => Some(Duration::from_secs_f64(v)),
        };
        Ok(Self {
            inner: PoolLimits {
                max_connections,
                max_keepalive_connections,
                keepalive_expiry: expiry,
            },
        })
    }

    #[getter]
    fn max_connections(&self) -> Option<usize> {
        self.inner.max_connections
    }

    #[getter]
    fn max_keepalive_connections(&self) -> Option<usize> {
        self.inner.max_keepalive_connections
    }

    #[getter]
    fn keepalive_expiry(&self) -> Option<f64> {
        self.inner.keepalive_expiry.map(|d| d.as_secs_f64())
    }

    fn __repr__(&self) -> String {
        format!(
            "Limits(max_connections={:?}, max_keepalive_connections={:?}, keepalive_expiry={:?})",
            self.max_connections(),
            self.max_keepalive_connections(),
            self.keepalive_expiry()
        )
    }
}

/// HTTP Basic auth wrapper exposed to Python.
#[pyclass(name = "BasicAuth", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyBasicAuth {
    pub inner: BasicAuth,
}

#[pymethods]
impl PyBasicAuth {
    #[new]
    fn new(username: &str, password: &str) -> Self {
        Self {
            inner: BasicAuth::new(username, password),
        }
    }
}

/// HTTP Digest auth wrapper exposed to Python.
#[pyclass(name = "DigestAuth", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyDigestAuth {
    pub inner: DigestAuth,
}

#[pymethods]
impl PyDigestAuth {
    #[new]
    fn new(username: &str, password: &str) -> Self {
        Self {
            inner: DigestAuth::new(username, password),
        }
    }
}

/// HTTP Bearer token auth wrapper exposed to Python.
#[pyclass(name = "BearerAuth", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyBearerAuth {
    pub inner: BearerAuth,
}

#[pymethods]
impl PyBearerAuth {
    #[new]
    fn new(token: &str) -> Self {
        Self {
            inner: BearerAuth::new(token),
        }
    }
}

/// HTTP Request object exposed to Python.
#[pyclass(name = "Request", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyRequest {
    pub inner: Request,
}

#[pymethods]
impl PyRequest {
    #[new]
    #[pyo3(signature = (method, url, *, params=None, headers=None, cookies=None, content=None, data=None, files=None, json=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        method: &str,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let url_str = extract_url_str(url)?;
        let builder = RequestBuilder::new(method, &url_str)?;
        let req = populate_request_builder(
            py,
            builder,
            params,
            headers,
            cookies,
            None,
            content,
            data,
            files,
            json,
            extensions,
        )?;
        Ok(Self { inner: req })
    }

    #[getter]
    fn method(&self) -> &str {
        self.inner.method.as_str()
    }

    #[getter]
    fn url(&self) -> PyURL {
        PyURL {
            inner: self.inner.url.clone(),
        }
    }

    #[getter]
    fn headers(&self) -> PyHeaders {
        PyHeaders {
            inner: self.inner.headers.clone(),
        }
    }

    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let bytes = self.inner.content();
        PyBytes::new_bound(py, &bytes)
    }

    fn read<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        self.content(py)
    }

    fn __repr__(&self) -> String {
        format!("<Request('{}', '{}')>", self.method(), self.inner.url)
    }
}

/// HTTP Response object exposed to Python with lazy text decoding and zero-copy body bytes.
#[pyclass(name = "Response", module = "httpx")]
#[derive(Clone, Debug)]
pub struct PyResponse {
    pub inner: Response,
}

#[pymethods]
impl PyResponse {
    #[getter]
    fn status_code(&self) -> u16 {
        self.inner.status_code
    }

    #[getter]
    fn reason_phrase(&self) -> &str {
        &self.inner.reason_phrase
    }

    #[getter]
    fn headers(&self) -> PyHeaders {
        PyHeaders {
            inner: self.inner.headers.clone(),
        }
    }

    #[getter]
    fn cookies(&self) -> PyCookies {
        PyCookies {
            inner: self.inner.cookies.clone(),
        }
    }

    #[getter]
    fn url(&self) -> PyURL {
        PyURL {
            inner: self.inner.url.clone(),
        }
    }

    #[getter]
    fn http_version(&self) -> &str {
        &self.inner.http_version
    }

    #[getter]
    fn elapsed<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let secs = self.inner.elapsed.as_secs();
        let micros = self.inner.elapsed.subsec_micros();
        let dt = py.import_bound("datetime")?;
        dt.getattr("timedelta")?.call1((0, secs, micros))
    }

    #[getter]
    fn encoding(&self) -> String {
        self.inner.encoding()
    }

    #[setter]
    fn set_encoding(&self, value: &str) {
        self.inner.set_encoding(value);
    }

    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self.inner.content()?;
        Ok(PyBytes::new_bound(py, &bytes))
    }

    #[getter]
    fn text(&self) -> PyResult<String> {
        Ok(self.inner.text()?)
    }

    #[pyo3(signature = (**_kwargs))]
    fn json<'py>(
        &self,
        py: Python<'py>,
        _kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        let val: JsonValue = self.inner.json()?;
        json_value_to_py(py, &val)
    }

    #[getter]
    fn is_informational(&self) -> bool {
        self.inner.is_informational()
    }

    #[getter]
    fn is_success(&self) -> bool {
        self.inner.is_success()
    }

    #[getter]
    fn is_redirect(&self) -> bool {
        self.inner.is_redirect()
    }

    #[getter]
    fn is_client_error(&self) -> bool {
        self.inner.is_client_error()
    }

    #[getter]
    fn is_server_error(&self) -> bool {
        self.inner.is_server_error()
    }

    #[getter]
    fn is_error(&self) -> bool {
        self.inner.is_error()
    }

    #[getter]
    fn is_stream_consumed(&self) -> bool {
        self.inner.is_stream_consumed()
    }

    #[getter]
    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    #[getter]
    fn num_bytes_downloaded(&self) -> usize {
        self.inner.num_bytes_downloaded()
    }

    #[getter]
    fn request(&self) -> Option<PyRequest> {
        self.inner
            .request
            .clone()
            .map(|r| PyRequest { inner: r })
    }

    #[getter]
    fn history(&self) -> Vec<PyResponse> {
        self.inner
            .history
            .iter()
            .cloned()
            .map(|r| PyResponse { inner: r })
            .collect()
    }

    fn raise_for_status(slf: PyRef<'_, Self>) -> PyResult<Py<PyResponse>> {
        let py = slf.py();
        if let Err(err) = slf.inner.raise_for_status() {
            let msg = err.to_string();
            let exc = HTTPStatusError::new_err(msg);
            let exc_bound = exc.value_bound(py);
            let _ = exc_bound.setattr("response", slf.clone().into_py(py));
            if let Some(req) = slf.request() {
                let _ = exc_bound.setattr("request", req.into_py(py));
            }
            return Err(exc);
        }
        Ok(Py::from(slf))
    }

    fn read<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let resp = self.inner.clone();
        let bytes = py.allow_threads(|| {
            let rt = crate::client::shared_runtime()?;
            rt.block_on(resp.read())
        })?;
        Ok(PyBytes::new_bound(py, &bytes))
    }

    fn aread<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let resp = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let bytes = resp.read().await.map_err(PyErr::from)?;
            Python::with_gil(|py| Ok(PyBytes::new_bound(py, &bytes).into_any().unbind()))
        })
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let resp = self.inner.clone();
        py.allow_threads(|| {
            let rt = crate::client::shared_runtime()?;
            rt.block_on(resp.close());
            Ok::<_, HttpxError>(())
        })?;
        Ok(())
    }

    fn aclose<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let resp = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            resp.close().await;
            Ok(())
        })
    }

    #[pyo3(signature = (chunk_size=None))]
    fn iter_bytes(&self, chunk_size: Option<usize>) -> PySyncByteIter {
        PySyncByteIter {
            resp: self.inner.clone(),
            chunk_size,
            buffer: Vec::new(),
            eof: false,
        }
    }

    #[pyo3(signature = (chunk_size=None))]
    fn iter_raw(&self, chunk_size: Option<usize>) -> PySyncByteIter {
        self.iter_bytes(chunk_size)
    }

    #[pyo3(signature = (chunk_size=None))]
    fn iter_text(&self, chunk_size: Option<usize>) -> PySyncTextIter {
        PySyncTextIter {
            resp: self.inner.clone(),
            encoding: self.inner.encoding(),
            chunk_size,
            buffer: String::new(),
            eof: false,
        }
    }

    fn iter_lines(&self) -> PySyncLineIter {
        PySyncLineIter {
            resp: self.inner.clone(),
            pending: String::new(),
            eof: false,
        }
    }

    #[pyo3(signature = (chunk_size=None))]
    fn aiter_bytes(&self, chunk_size: Option<usize>) -> PyAsyncByteIter {
        PyAsyncByteIter {
            resp: self.inner.clone(),
            chunk_size,
        }
    }

    #[pyo3(signature = (chunk_size=None))]
    fn aiter_raw(&self, chunk_size: Option<usize>) -> PyAsyncByteIter {
        self.aiter_bytes(chunk_size)
    }

    #[pyo3(signature = (chunk_size=None))]
    fn aiter_text(&self, chunk_size: Option<usize>) -> PyAsyncTextIter {
        PyAsyncTextIter {
            resp: self.inner.clone(),
            encoding: self.inner.encoding(),
            chunk_size,
        }
    }

    fn aiter_lines(&self) -> PyAsyncLineIter {
        PyAsyncLineIter {
            resp: self.inner.clone(),
            pending: std::sync::Arc::new(tokio::sync::Mutex::new(String::new())),
            eof: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "<Response [{} {}]>",
            self.inner.status_code, self.inner.reason_phrase
        )
    }
}

#[pyclass(module = "httpx")]
pub struct PySyncByteIter {
    resp: Response,
    chunk_size: Option<usize>,
    buffer: Vec<u8>,
    eof: bool,
}

#[pymethods]
impl PySyncByteIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(mut slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        if let Some(target_size) = slf.chunk_size {
            let target = target_size.max(1);
            while slf.buffer.len() < target && !slf.eof {
                let resp = slf.resp.clone();
                let next = py.allow_threads(|| {
                    let rt = crate::client::shared_runtime()?;
                    rt.block_on(resp.next_chunk())
                })?;
                match next {
                    Some(chunk) => slf.buffer.extend_from_slice(&chunk),
                    None => slf.eof = true,
                }
            }
            if slf.buffer.is_empty() {
                return Err(PyStopIteration::new_err(()));
            }
            let take = slf.buffer.len().min(target);
            let out: Vec<u8> = slf.buffer.drain(..take).collect();
            return Ok(PyBytes::new_bound(py, &out));
        }

        if slf.eof {
            return Err(PyStopIteration::new_err(()));
        }
        let resp = slf.resp.clone();
        let next = py.allow_threads(|| {
            let rt = crate::client::shared_runtime()?;
            rt.block_on(resp.next_chunk())
        })?;
        match next {
            Some(chunk) => Ok(PyBytes::new_bound(py, &chunk)),
            None => {
                slf.eof = true;
                Err(PyStopIteration::new_err(()))
            }
        }
    }
}

#[pyclass(module = "httpx")]
pub struct PySyncLineIter {
    resp: Response,
    pending: String,
    eof: bool,
}

#[pymethods]
impl PySyncLineIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<String> {
        loop {
            if let Some(pos) = self.pending.find('\n') {
                let mut line: String = self.pending.drain(..=pos).collect();
                if line.ends_with('\n') {
                    line.pop();
                }
                if line.ends_with('\r') {
                    line.pop();
                }
                return Ok(line);
            }
            if self.eof {
                if !self.pending.is_empty() {
                    return Ok(std::mem::take(&mut self.pending));
                }
                return Err(PyStopIteration::new_err(()));
            }
            let resp = self.resp.clone();
            let next = py.allow_threads(|| {
                let rt = crate::client::shared_runtime()?;
                rt.block_on(resp.next_chunk())
            })?;
            match next {
                Some(chunk) => {
                    self.pending.push_str(&String::from_utf8_lossy(&chunk));
                }
                None => self.eof = true,
            }
        }
    }
}

#[pyclass(module = "httpx")]
pub struct PySyncTextIter {
    resp: Response,
    encoding: String,
    chunk_size: Option<usize>,
    buffer: String,
    eof: bool,
}

#[pymethods]
impl PySyncTextIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<String> {
        let enc = encoding_rs::Encoding::for_label(self.encoding.as_bytes())
            .unwrap_or(encoding_rs::UTF_8);

        if let Some(target) = self.chunk_size {
            let target = target.max(1);
            while self.buffer.chars().count() < target && !self.eof {
                let resp = self.resp.clone();
                let next = py.allow_threads(|| {
                    let rt = crate::client::shared_runtime()?;
                    rt.block_on(resp.next_chunk())
                })?;
                match next {
                    Some(chunk) => {
                        let (decoded, _, _) = enc.decode(&chunk);
                        self.buffer.push_str(&decoded);
                    }
                    None => self.eof = true,
                }
            }
            if self.buffer.is_empty() {
                return Err(PyStopIteration::new_err(()));
            }
            let split_byte = self
                .buffer
                .char_indices()
                .nth(target)
                .map(|(idx, _)| idx)
                .unwrap_or(self.buffer.len());
            let out: String = self.buffer.drain(..split_byte).collect();
            return Ok(out);
        }

        if self.eof {
            return Err(PyStopIteration::new_err(()));
        }
        let resp = self.resp.clone();
        let next = py.allow_threads(|| {
            let rt = crate::client::shared_runtime()?;
            rt.block_on(resp.next_chunk())
        })?;
        match next {
            Some(chunk) => {
                let (decoded, _, _) = enc.decode(&chunk);
                Ok(decoded.into_owned())
            }
            None => {
                self.eof = true;
                Err(PyStopIteration::new_err(()))
            }
        }
    }
}

#[pyclass(module = "httpx")]
pub struct PyAsyncByteIter {
    resp: Response,
    chunk_size: Option<usize>,
}

#[pymethods]
impl PyAsyncByteIter {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let resp = self.resp.clone();
        let _ = self.chunk_size;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match resp.next_chunk().await.map_err(PyErr::from)? {
                Some(chunk) => {
                    Python::with_gil(|py| Ok(PyBytes::new_bound(py, &chunk).into_any().unbind()))
                }
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }
}

#[pyclass(module = "httpx")]
pub struct PyAsyncTextIter {
    resp: Response,
    encoding: String,
    chunk_size: Option<usize>,
}

#[pymethods]
impl PyAsyncTextIter {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let resp = self.resp.clone();
        let encoding = self.encoding.clone();
        let _ = self.chunk_size;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match resp.next_chunk().await.map_err(PyErr::from)? {
                Some(chunk) => {
                    let enc = encoding_rs::Encoding::for_label(encoding.as_bytes())
                        .unwrap_or(encoding_rs::UTF_8);
                    let (decoded, _, _) = enc.decode(&chunk);
                    Ok(decoded.into_owned())
                }
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }
}

#[pyclass(module = "httpx")]
pub struct PyAsyncLineIter {
    resp: Response,
    pending: std::sync::Arc<tokio::sync::Mutex<String>>,
    eof: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[pymethods]
impl PyAsyncLineIter {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let resp = self.resp.clone();
        let pending = std::sync::Arc::clone(&self.pending);
        let eof = std::sync::Arc::clone(&self.eof);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            loop {
                {
                    let mut buf = pending.lock().await;
                    if let Some(pos) = buf.find('\n') {
                        let mut line: String = buf.drain(..=pos).collect();
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        if line.ends_with('\r') {
                            line.pop();
                        }
                        return Ok(line);
                    }
                    if eof.load(std::sync::atomic::Ordering::Relaxed) {
                        if !buf.is_empty() {
                            return Ok(std::mem::take(&mut *buf));
                        }
                        return Err(PyStopAsyncIteration::new_err(()));
                    }
                }

                match resp.next_chunk().await.map_err(PyErr::from)? {
                    Some(chunk) => {
                        let mut buf = pending.lock().await;
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    None => {
                        eof.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        })
    }
}

// ============================================================================
// Python <-> Rust Extraction Helpers
// ============================================================================

pub fn extract_url_str(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(u) = obj.extract::<PyRef<'_, PyURL>>() {
        Ok(u.inner.as_str().to_string())
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(s)
    } else {
        Err(PyTypeError::new_err("URL must be a str or httpx.URL"))
    }
}

pub fn extract_headers(obj: &Bound<'_, PyAny>) -> PyResult<Headers> {
    if let Ok(hdrs) = obj.extract::<PyRef<'_, PyHeaders>>() {
        return Ok(hdrs.inner.clone());
    }
    let mut out = Headers::new();
    if let Ok(dict) = obj.downcast::<PyDict>() {
        for (k, v) in dict.iter() {
            let ks: String = k.extract()?;
            let vs: String = v.extract()?;
            out.insert(&ks, &vs)?;
        }
        return Ok(out);
    }
    if let Ok(seq) = obj.iter() {
        for item_res in seq {
            let item = item_res?;
            let (k, v): (String, String) = item.extract()?;
            out.append(&k, &v)?;
        }
        return Ok(out);
    }
    Err(PyTypeError::new_err(
        "headers must be a Headers, dict, or sequence of (name, value) tuples",
    ))
}

pub fn extract_params(obj: &Bound<'_, PyAny>) -> PyResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    if let Ok(dict) = obj.downcast::<PyDict>() {
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            if v.is_none() {
                continue;
            }
            if let Ok(list) = v.downcast::<PyList>() {
                for item in list.iter() {
                    out.push((key.clone(), py_scalar_to_string(&item)?));
                }
            } else if let Ok(tuple) = v.downcast::<PyTuple>() {
                for item in tuple.iter() {
                    out.push((key.clone(), py_scalar_to_string(&item)?));
                }
            } else {
                out.push((key, py_scalar_to_string(&v)?));
            }
        }
        return Ok(out);
    }
    if let Ok(seq) = obj.iter() {
        for item_res in seq {
            let item = item_res?;
            let (k, v): (String, Bound<'_, PyAny>) = item.extract()?;
            out.push((k, py_scalar_to_string(&v)?));
        }
        return Ok(out);
    }
    Err(PyTypeError::new_err(
        "params must be a dict or sequence of (key, value) tuples",
    ))
}

fn py_scalar_to_string(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(s) = obj.extract::<String>() {
        Ok(s)
    } else if obj.is_instance_of::<PyBool>() {
        let b: bool = obj.extract()?;
        Ok(if b { "true" } else { "false" }.to_string())
    } else if let Ok(i) = obj.extract::<i64>() {
        Ok(i.to_string())
    } else if let Ok(f) = obj.extract::<f64>() {
        Ok(f.to_string())
    } else {
        Ok(obj.str()?.to_string())
    }
}

pub fn extract_cookie_jar(obj: &Bound<'_, PyAny>) -> PyResult<CookieJar> {
    if let Ok(c) = obj.extract::<PyRef<'_, PyCookies>>() {
        return Ok(c.inner.clone());
    }
    let jar = CookieJar::new();
    let map = extract_cookie_map(obj)?;
    for (k, v) in map {
        jar.set(k, v, None, None);
    }
    Ok(jar)
}

pub fn extract_cookies_or_none(
    obj: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<HashMap<String, String>>> {
    match obj {
        Some(c) if !c.is_none() => Ok(Some(extract_cookie_map(c)?)),
        _ => Ok(None),
    }
}

pub fn extract_cookie_map(obj: &Bound<'_, PyAny>) -> PyResult<HashMap<String, String>> {
    if let Ok(c) = obj.extract::<PyRef<'_, PyCookies>>() {
        return Ok(c.inner.items().into_iter().collect());
    }
    let mut map = HashMap::new();
    if let Ok(dict) = obj.downcast::<PyDict>() {
        for (k, v) in dict.iter() {
            map.insert(k.extract::<String>()?, v.extract::<String>()?);
        }
        return Ok(map);
    }
    if let Ok(seq) = obj.iter() {
        for item_res in seq {
            let item = item_res?;
            let (k, v): (String, String) = item.extract()?;
            map.insert(k, v);
        }
        return Ok(map);
    }
    Err(PyTypeError::new_err(
        "cookies must be a Cookies, dict, or sequence of (name, value) tuples",
    ))
}

pub fn extract_auth(obj: &Bound<'_, PyAny>) -> PyResult<AuthMethod> {
    if let Ok(b) = obj.extract::<PyRef<'_, PyBasicAuth>>() {
        return Ok(AuthMethod::Basic(b.inner.clone()));
    }
    if let Ok(d) = obj.extract::<PyRef<'_, PyDigestAuth>>() {
        return Ok(AuthMethod::Digest(d.inner.clone()));
    }
    if let Ok(b) = obj.extract::<PyRef<'_, PyBearerAuth>>() {
        return Ok(AuthMethod::Bearer(b.inner.clone()));
    }
    if let Ok((user, pass)) = obj.extract::<(String, String)>() {
        return Ok(AuthMethod::Basic(BasicAuth::new(user, pass)));
    }
    Err(PyTypeError::new_err(
        "auth must be a (username, password) tuple, BasicAuth, DigestAuth, or BearerAuth",
    ))
}

pub fn extract_timeout(obj: &Bound<'_, PyAny>) -> PyResult<TimeoutConfig> {
    if obj.is_none() {
        return Ok(TimeoutConfig::none());
    }
    if let Ok(t) = obj.extract::<PyRef<'_, PyTimeout>>() {
        return Ok(t.inner);
    }
    if let Ok(secs) = obj.extract::<f64>() {
        return Ok(TimeoutConfig::from_secs(Some(secs), None, None, None, None)?);
    }
    Err(PyTypeError::new_err(
        "timeout must be a float, int, None, or httpx.Timeout",
    ))
}

pub fn extract_limits(obj: &Bound<'_, PyAny>) -> PyResult<PoolLimits> {
    if let Ok(l) = obj.extract::<PyRef<'_, PyLimits>>() {
        return Ok(l.inner);
    }
    Err(PyTypeError::new_err("limits must be an httpx.Limits"))
}

pub fn extract_proxies(obj: &Bound<'_, PyAny>) -> PyResult<ProxyMounts> {
    if let Ok(s) = obj.extract::<String>() {
        return Ok(ProxyMounts::single(&s)?);
    }
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = HashMap::new();
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            let val: Option<String> = if v.is_none() {
                None
            } else {
                Some(v.extract::<String>()?)
            };
            map.insert(key, val);
        }
        return Ok(ProxyMounts::from_map(map)?);
    }
    Err(PyTypeError::new_err(
        "proxies must be a proxy URL string or dict mapping mount patterns to URLs",
    ))
}

pub fn extract_verify(obj: &Bound<'_, PyAny>) -> PyResult<TlsVerify> {
    if obj.is_instance_of::<PyBool>() {
        let b: bool = obj.extract()?;
        return Ok(if b {
            TlsVerify::Default
        } else {
            TlsVerify::Disabled
        });
    }
    if let Ok(path) = obj.extract::<String>() {
        return Ok(TlsVerify::CustomCa(PathBuf::from(path)));
    }
    Err(PyTypeError::new_err(
        "verify must be a bool or path to a CA certificate bundle",
    ))
}

pub fn extract_cert(obj: &Bound<'_, PyAny>) -> PyResult<ClientCertConfig> {
    if let Ok(path) = obj.extract::<String>() {
        return Ok(ClientCertConfig {
            cert_path: PathBuf::from(path),
            key_path: None,
        });
    }
    if let Ok((cert_path, key_path)) = obj.extract::<(String, String)>() {
        return Ok(ClientCertConfig {
            cert_path: PathBuf::from(cert_path),
            key_path: Some(PathBuf::from(key_path)),
        });
    }
    Err(PyTypeError::new_err(
        "cert must be a cert file path or (cert_path, key_path) tuple",
    ))
}

pub fn py_to_json_value(obj: &Bound<'_, PyAny>) -> PyResult<JsonValue> {
    if obj.is_none() {
        Ok(JsonValue::Null)
    } else if obj.is_instance_of::<PyBool>() {
        Ok(JsonValue::Bool(obj.extract::<bool>()?))
    } else if obj.is_instance_of::<PyInt>() {
        if let Ok(i) = obj.extract::<i64>() {
            Ok(JsonValue::Number(i.into()))
        } else {
            let u = obj.extract::<u64>()?;
            Ok(JsonValue::Number(u.into()))
        }
    } else if obj.is_instance_of::<PyFloat>() {
        let f = obj.extract::<f64>()?;
        serde_json::Number::from_f64(f)
            .map(JsonValue::Number)
            .ok_or_else(|| PyValueError::new_err("Out of range float for JSON"))
    } else if obj.is_instance_of::<PyString>() {
        Ok(JsonValue::String(obj.extract::<String>()?))
    } else if let Ok(list) = obj.downcast::<PyList>() {
        let mut arr = Vec::with_capacity(list.len());
        for item in list.iter() {
            arr.push(py_to_json_value(&item)?);
        }
        Ok(JsonValue::Array(arr))
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        let mut arr = Vec::with_capacity(tuple.len());
        for item in tuple.iter() {
            arr.push(py_to_json_value(&item)?);
        }
        Ok(JsonValue::Array(arr))
    } else if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = serde_json::Map::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            let key = k.extract::<String>()?;
            map.insert(key, py_to_json_value(&v)?);
        }
        Ok(JsonValue::Object(map))
    } else {
        Err(PyTypeError::new_err(format!(
            "Object of type '{}' is not JSON serializable",
            obj.get_type().name()?
        )))
    }
}

pub fn json_value_to_py(py: Python<'_>, val: &JsonValue) -> PyResult<Py<PyAny>> {
    match val {
        JsonValue::Null => Ok(py.None()),
        JsonValue::Bool(b) => Ok(b.into_py(py)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_py(py))
            } else if let Some(u) = n.as_u64() {
                Ok(u.into_py(py))
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_py(py))
            } else {
                Ok(py.None())
            }
        }
        JsonValue::String(s) => Ok(s.into_py(py)),
        JsonValue::Array(arr) => {
            let list = PyList::empty_bound(py);
            for item in arr {
                list.append(json_value_to_py(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        JsonValue::Object(map) => {
            let dict = PyDict::new_bound(py);
            for (k, v) in map {
                dict.set_item(k, json_value_to_py(py, v)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

fn read_py_bytes_or_file(obj: &Bound<'_, PyAny>) -> PyResult<Bytes> {
    if let Ok(b) = obj.downcast::<PyBytes>() {
        return Ok(Bytes::copy_from_slice(b.as_bytes()));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(Bytes::from(s.into_bytes()));
    }
    if obj.hasattr("read")? {
        let data = obj.call_method0("read")?;
        if let Ok(b) = data.downcast::<PyBytes>() {
            return Ok(Bytes::copy_from_slice(b.as_bytes()));
        }
        if let Ok(s) = data.extract::<String>() {
            return Ok(Bytes::from(s.into_bytes()));
        }
    }
    Err(PyTypeError::new_err(
        "Expected bytes, str, or file-like object with .read()",
    ))
}

fn extract_files_into_multipart(
    files: &Bound<'_, PyAny>,
    data: Option<&Bound<'_, PyAny>>,
) -> PyResult<MultipartEncoder> {
    let mut encoder = MultipartEncoder::new();

    if let Some(d) = data {
        if !d.is_none() {
            for (k, v) in extract_params(d)? {
                encoder.add_field(k, v);
            }
        }
    }

    let items: Vec<(String, Bound<'_, PyAny>)> = if let Ok(dict) = files.downcast::<PyDict>() {
        dict.iter()
            .map(|(k, v)| Ok((k.extract::<String>()?, v)))
            .collect::<PyResult<Vec<_>>>()?
    } else if let Ok(seq) = files.iter() {
        seq.map(|item_res| {
            let item = item_res?;
            item.extract::<(String, Bound<'_, PyAny>)>()
        })
        .collect::<PyResult<Vec<_>>>()?
    } else {
        return Err(PyTypeError::new_err(
            "files must be a dict or sequence of (field, file) tuples",
        ));
    };

    for (field_name, spec) in items {
        if let Ok(tuple) = spec.downcast::<PyTuple>() {
            let len = tuple.len();
            if !(2..=4).contains(&len) {
                return Err(PyValueError::new_err(
                    "File tuple must be (filename, content[, content_type[, headers]])",
                ));
            }
            let filename_obj = tuple.get_item(0)?;
            let filename: Option<String> = if filename_obj.is_none() {
                None
            } else {
                Some(filename_obj.extract::<String>()?)
            };
            let content = read_py_bytes_or_file(&tuple.get_item(1)?)?;
            let content_type: Option<String> = if len >= 3 {
                let ct = tuple.get_item(2)?;
                if ct.is_none() {
                    None
                } else {
                    Some(ct.extract::<String>()?)
                }
            } else {
                None
            };
            let mut extra_headers = Vec::new();
            if len == 4 {
                let hdr_obj = tuple.get_item(3)?;
                if !hdr_obj.is_none() {
                    extra_headers = extract_headers(&hdr_obj)?.multi_items();
                }
            }
            encoder.add_file(MultipartFile {
                field_name,
                filename,
                content_type,
                content,
                headers: extra_headers,
            });
        } else {
            let content = read_py_bytes_or_file(&spec)?;
            let filename = if spec.hasattr("name")? {
                spec.getattr("name")
                    .ok()
                    .and_then(|n| n.extract::<String>().ok())
            } else {
                Some(field_name.clone())
            };
            encoder.add_file(MultipartFile {
                field_name,
                filename,
                content_type: None,
                content,
                headers: Vec::new(),
            });
        }
    }

    Ok(encoder)
}

#[allow(clippy::too_many_arguments)]
pub fn populate_request_builder(
    _py: Python<'_>,
    mut builder: RequestBuilder,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    client_cookies: Option<&CookieJar>,
    content: Option<&Bound<'_, PyAny>>,
    data: Option<&Bound<'_, PyAny>>,
    files: Option<&Bound<'_, PyAny>>,
    json: Option<&Bound<'_, PyAny>>,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<Request> {
    if let Some(p) = params {
        if !p.is_none() {
            builder = builder.params(extract_params(p)?);
        }
    }
    if let Some(h) = headers {
        if !h.is_none() {
            builder = builder.headers(extract_headers(h)?);
        }
    }
    if let Some(c) = cookies {
        if !c.is_none() {
            let map = extract_cookie_map(c)?;
            let jar = client_cookies.cloned().unwrap_or_default();
            builder = builder.apply_cookies(&jar, Some(&map))?;
        }
    }

    if let Some(f) = files {
        if !f.is_none() {
            let encoder = extract_files_into_multipart(f, data)?;
            builder = builder.multipart(&encoder)?;
        }
    } else if let Some(j) = json {
        if !j.is_none() {
            let val = py_to_json_value(j)?;
            builder = builder.json(&val)?;
        }
    } else if let Some(d) = data {
        if !d.is_none() {
            if let Ok(s) = d.extract::<String>() {
                builder = builder.content(Bytes::from(s.into_bytes()));
            } else if let Ok(b) = d.downcast::<PyBytes>() {
                builder = builder.content(Bytes::copy_from_slice(b.as_bytes()));
            } else {
                let fields = extract_params(d)?;
                builder = builder.form(&fields)?;
            }
        }
    } else if let Some(c) = content {
        if !c.is_none() {
            if let Ok(b) = c.downcast::<PyBytes>() {
                builder = builder.content(Bytes::copy_from_slice(b.as_bytes()));
            } else if let Ok(s) = c.extract::<String>() {
                builder = builder.content(Bytes::from(s.into_bytes()));
            } else if let Ok(iter) = c.iter() {
                // Collect synchronous Python byte generator/iterator cleanly into a Rust stream
                let mut chunks: Vec<crate::exceptions::Result<Bytes>> = Vec::new();
                for item_res in iter {
                    let item = item_res?;
                    if let Ok(b) = item.downcast::<PyBytes>() {
                        chunks.push(Ok(Bytes::copy_from_slice(b.as_bytes())));
                    } else if let Ok(s) = item.extract::<String>() {
                        chunks.push(Ok(Bytes::from(s.into_bytes())));
                    } else {
                        return Err(PyTypeError::new_err(
                            "Streaming content iterable must yield bytes or str",
                        ));
                    }
                }
                let body_stream: BoxByteStream = Box::pin(stream::iter(chunks));
                builder = builder.stream(body_stream);
            } else {
                return Err(PyTypeError::new_err(
                    "content must be bytes, str, or an iterable of bytes",
                ));
            }
        }
    }

    if let Some(ext) = extensions {
        if !ext.is_none() {
            let val = py_to_json_value(ext)?;
            if let JsonValue::Object(map) = val {
                builder = builder.extensions(map.into_iter().collect());
            }
        }
    }

    Ok(builder.build()?)
}

/// Native PyO3 module entrypoint `_httpx_rs` imported by the `httpx` Python package.
#[pymodule]
pub fn _httpx_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    register_exceptions(m)?;

    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<PyHeaders>()?;
    m.add_class::<PyCookies>()?;
    m.add_class::<PyURL>()?;
    m.add_class::<PyTimeout>()?;
    m.add_class::<PyLimits>()?;
    m.add_class::<PyBasicAuth>()?;
    m.add_class::<PyDigestAuth>()?;
    m.add_class::<PyBearerAuth>()?;
    m.add_class::<PyRequest>()?;
    m.add_class::<PyResponse>()?;
    m.add_class::<PySyncByteIter>()?;
    m.add_class::<PySyncTextIter>()?;
    m.add_class::<PySyncLineIter>()?;
    m.add_class::<PyAsyncByteIter>()?;
    m.add_class::<PyAsyncTextIter>()?;
    m.add_class::<PyAsyncLineIter>()?;

    sync::register_sync(m)?;
    asynchronous::register_async(m)?;

    Ok(())
}
