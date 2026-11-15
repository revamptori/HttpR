//! Asynchronous Python client (`httpx.AsyncClient`) powered by Tokio and `pyo3-async-runtimes`.

use crate::client::{AsyncClient, ClientBuilder, SendOptions};
use crate::exceptions::HttpxError;
use crate::middleware::{RetryConfig, TimeoutConfig};
use crate::python::sync::build_send_options;
use crate::python::{
    extract_auth, extract_cert, extract_cookie_jar, extract_cookies_or_none, extract_headers,
    extract_limits, extract_params, extract_proxies, extract_timeout, extract_url_str,
    extract_verify, populate_request_builder, PyCookies, PyHeaders, PyRequest, PyResponse,
    PyTimeout, PyURL,
};
use crate::request::{BoxByteStream, Request};
use crate::response::Response;
use bytes::Bytes;
use futures_util::stream;
use pyo3::exceptions::{PyRuntimeError, PyStopAsyncIteration, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Async context manager returned by `AsyncClient.stream(...)`.
#[pyclass(name = "AsyncStreamContext", module = "httpx")]
pub struct PyAsyncStreamContext {
    client: AsyncClient,
    request: Option<Request>,
    options: SendOptions,
    response: Arc<Mutex<Option<Response>>>,
}

#[pymethods]
impl PyAsyncStreamContext {
    fn __aenter__<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let req = self
            .request
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("AsyncStreamContext already entered"))?;
        let client = self.client.clone();
        let options = self.options.clone();
        let resp_slot = Arc::clone(&self.response);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let resp = client.send(req, options).await.map_err(PyErr::from)?;
            *resp_slot.lock().await = Some(resp.clone());
            Ok(PyResponse { inner: resp })
        })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let resp_slot = Arc::clone(&self.response);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            if let Some(resp) = resp_slot.lock().await.take() {
                resp.close().await;
            }
            Ok(false)
        })
    }
}

/// Asynchronous HTTP client compatible with `httpx.AsyncClient`.
#[pyclass(name = "AsyncClient", module = "httpx")]
#[derive(Clone)]
pub struct PyAsyncClient {
    inner: AsyncClient,
    default_timeout: TimeoutConfig,
    base_url: Option<url::Url>,
}

#[pymethods]
impl PyAsyncClient {
    #[new]
    #[pyo3(signature = (*, auth=None, params=None, headers=None, cookies=None, verify=None, cert=None, http2=false, proxies=None, timeout=None, follow_redirects=false, limits=None, max_redirects=20, base_url=None, trust_env=true, retries=0, http2_prior_knowledge=false))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        auth: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        verify: Option<&Bound<'_, PyAny>>,
        cert: Option<&Bound<'_, PyAny>>,
        http2: bool,
        proxies: Option<&Bound<'_, PyAny>>,
        timeout: Option<&Bound<'_, PyAny>>,
        follow_redirects: bool,
        limits: Option<&Bound<'_, PyAny>>,
        max_redirects: usize,
        base_url: Option<&Bound<'_, PyAny>>,
        trust_env: bool,
        retries: usize,
        http2_prior_knowledge: bool,
    ) -> PyResult<Self> {
        let mut builder = ClientBuilder::new()
            .http2(http2)
            .follow_redirects(follow_redirects)
            .max_redirects(max_redirects)
            .trust_env(trust_env)
            .retries(RetryConfig::new(retries))
            .http2_prior_knowledge(http2_prior_knowledge);

        let mut parsed_base_url = None;
        if let Some(bu) = base_url {
            if !bu.is_none() {
                let s = extract_url_str(bu)?;
                builder = builder.base_url(&s)?;
                parsed_base_url = Some(
                    url::Url::parse(&s)
                        .map_err(|e| PyErr::from(HttpxError::InvalidUrl(e.to_string())))?,
                );
            }
        }
        if let Some(a) = auth {
            if !a.is_none() {
                builder = builder.auth(extract_auth(a)?);
            }
        }
        if let Some(p) = params {
            if !p.is_none() {
                builder = builder.params(extract_params(p)?);
            }
        }
        if let Some(h) = headers {
            if !h.is_none() {
                builder = builder.default_headers(extract_headers(h)?);
            }
        }
        if let Some(c) = cookies {
            if !c.is_none() {
                builder = builder.cookies(extract_cookie_jar(c)?);
            }
        }
        if let Some(v) = verify {
            if !v.is_none() {
                builder = builder.verify(extract_verify(v)?);
            }
        }
        if let Some(c) = cert {
            if !c.is_none() {
                builder = builder.cert(extract_cert(c)?);
            }
        }
        if let Some(px) = proxies {
            if !px.is_none() {
                builder = builder.proxies(extract_proxies(px)?);
            }
        }
        let default_timeout = match timeout {
            Some(t) => extract_timeout(t)?,
            None => TimeoutConfig::default(),
        };
        builder = builder.timeout(default_timeout);

        if let Some(l) = limits {
            if !l.is_none() {
                builder = builder.limits(extract_limits(l)?);
            }
        }

        let inner = builder.build_async()?;
        Ok(Self {
            inner,
            default_timeout,
            base_url: parsed_base_url,
        })
    }

    #[getter]
    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    #[getter]
    fn headers(&self) -> PyHeaders {
        PyHeaders {
            inner: self.inner.headers().clone(),
        }
    }

    #[getter]
    fn cookies(&self) -> PyCookies {
        PyCookies {
            inner: self.inner.cookies().clone(),
        }
    }

    #[getter]
    fn base_url(&self) -> Option<PyURL> {
        self.base_url.clone().map(|u| PyURL { inner: u })
    }

    #[getter]
    fn timeout(&self) -> PyTimeout {
        PyTimeout {
            inner: self.default_timeout,
        }
    }

    fn aclose<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client.aclose().await;
            Ok(())
        })
    }

    fn __aenter__<'py>(slf: PyRef<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let me: Py<Self> = slf.into();
        pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(me) })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client.aclose().await;
            Ok(false)
        })
    }

    #[pyo3(signature = (method, url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn build_request(
        &self,
        py: Python<'_>,
        method: &str,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyRequest> {
        let _ = timeout;
        let req = self.build_py_req_internal(
            py, method, url, content, data, files, json, params, headers, cookies, extensions,
        )?;
        Ok(PyRequest { inner: req })
    }

    #[pyo3(signature = (request, *, stream=false, auth=None, follow_redirects=None))]
    fn send<'py>(
        &self,
        py: Python<'py>,
        request: &PyRequest,
        stream: bool,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let parsed_auth = match auth {
            Some(a) if !a.is_none() => Some(extract_auth(a)?),
            _ => None,
        };
        let options = SendOptions {
            auth: parsed_auth,
            follow_redirects,
            timeout: None,
            stream,
        };
        let client = self.inner.clone();
        let req = request.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let resp = client.send(req, options).await.map_err(PyErr::from)?;
            Ok(PyResponse { inner: resp })
        })
    }

    #[pyo3(signature = (method, url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn request<'py>(
        &self,
        py: Python<'py>,
        method: &str,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let req = self.build_py_req_internal(
            py, method, url, content, data, files, json, params, headers, cookies, extensions,
        )?;
        let options = build_send_options(auth, follow_redirects, timeout, false)?;
        let client = self.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let resp = client.send(req, options).await.map_err(PyErr::from)?;
            Ok(PyResponse { inner: resp })
        })
    }

    #[pyo3(signature = (method, url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn stream(
        &self,
        py: Python<'_>,
        method: &str,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyAsyncStreamContext> {
        let req = self.build_py_req_internal(
            py, method, url, content, data, files, json, params, headers, cookies, extensions,
        )?;
        let options = build_send_options(auth, follow_redirects, timeout, true)?;
        Ok(PyAsyncStreamContext {
            client: self.inner.clone(),
            request: Some(req),
            options,
            response: Arc::new(Mutex::new(None)),
        })
    }

    #[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn get<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "GET",
            url,
            None,
            None,
            None,
            None,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    #[pyo3(signature = (url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn post<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "POST",
            url,
            content,
            data,
            files,
            json,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    #[pyo3(signature = (url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn put<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "PUT",
            url,
            content,
            data,
            files,
            json,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    #[pyo3(signature = (url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn patch<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "PATCH",
            url,
            content,
            data,
            files,
            json,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    #[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn delete<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "DELETE",
            url,
            None,
            None,
            None,
            None,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    #[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn head<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "HEAD",
            url,
            None,
            None,
            None,
            None,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    #[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn options<'py>(
        &self,
        py: Python<'py>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "OPTIONS",
            url,
            None,
            None,
            None,
            None,
            params,
            headers,
            cookies,
            auth,
            follow_redirects,
            timeout,
            extensions,
        )
    }

    fn __repr__(&self) -> String {
        "<httpx.AsyncClient>".to_string()
    }
}

impl PyAsyncClient {
    #[allow(clippy::too_many_arguments)]
    fn build_py_req_internal(
        &self,
        py: Python<'_>,
        method: &str,
        url: &Bound<'_, PyAny>,
        content: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        files: Option<&Bound<'_, PyAny>>,
        json: Option<&Bound<'_, PyAny>>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Request> {
        let url_str = extract_url_str(url)?;
        let extra_cookies = extract_cookies_or_none(cookies)?;
        let builder = self
            .inner
            .build_request(method, &url_str, extra_cookies.as_ref())?;

        // Check if `content` is an async iterator (__aiter__ without bytes/str/__iter__)
        if let Some(c) = content {
            if !c.is_none()
                && c.downcast::<PyBytes>().is_err()
                && c.extract::<String>().is_err()
                && !c.hasattr("__iter__")?
                && c.hasattr("__aiter__")?
            {
                let aiter = c.call_method0("__aiter__")?.unbind();
                let mut builder_with_meta = builder;
                if let Some(p) = params {
                    if !p.is_none() {
                        builder_with_meta = builder_with_meta.params(extract_params(p)?);
                    }
                }
                if let Some(h) = headers {
                    if !h.is_none() {
                        builder_with_meta = builder_with_meta.headers(extract_headers(h)?);
                    }
                }
                builder_with_meta = builder_with_meta.stream(py_async_iter_to_stream(aiter));
                return Ok(builder_with_meta.build()?);
            }
        }

        populate_request_builder(
            py,
            builder,
            params,
            headers,
            None,
            Some(self.inner.cookies()),
            content,
            data,
            files,
            json,
            extensions,
        )
    }
}

fn py_async_iter_to_stream(aiter: Py<PyAny>) -> BoxByteStream {
    let s = stream::unfold((aiter, false), |(iter_obj, done)| async move {
        if done {
            return None;
        }
        let next_fut_res = Python::with_gil(|py| -> PyResult<_> {
            let awaitable = iter_obj.bind(py).call_method0("__anext__")?;
            pyo3_async_runtimes::tokio::into_future(awaitable)
        });

        let fut = match next_fut_res {
            Ok(f) => f,
            Err(e) => {
                return Some((
                    Err(HttpxError::RequestError(format!(
                        "Error calling __anext__ on request body stream: {e}"
                    ))),
                    (iter_obj, true),
                ));
            }
        };

        match fut.await {
            Ok(item) => {
                let bytes_res = Python::with_gil(|py| -> PyResult<Bytes> {
                    let bound = item.bind(py);
                    if let Ok(b) = bound.downcast::<PyBytes>() {
                        Ok(Bytes::copy_from_slice(b.as_bytes()))
                    } else if let Ok(s) = bound.extract::<String>() {
                        Ok(Bytes::from(s.into_bytes()))
                    } else {
                        Err(PyTypeError::new_err(
                            "Async request body stream must yield bytes or str",
                        ))
                    }
                });
                match bytes_res {
                    Ok(chunk) => Some((Ok(chunk), (iter_obj, false))),
                    Err(e) => Some((
                        Err(HttpxError::RequestError(e.to_string())),
                        (iter_obj, true),
                    )),
                }
            }
            Err(err) => {
                let is_stop = Python::with_gil(|py| err.is_instance_of::<PyStopAsyncIteration>(py));
                if is_stop {
                    None
                } else {
                    Some((
                        Err(HttpxError::RequestError(format!(
                            "Async request stream error: {err}"
                        ))),
                        (iter_obj, true),
                    ))
                }
            }
        }
    });
    Box::pin(s)
}

/// Registers asynchronous client classes on the Python module.
pub fn register_async(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAsyncClient>()?;
    m.add_class::<PyAsyncStreamContext>()?;
    Ok(())
}
