//! Synchronous Python client (`httpx.Client`) and top-level request functions.

use crate::client::{shared_runtime, Client, ClientBuilder, SendOptions};
use crate::middleware::{RetryConfig, TimeoutConfig};
use crate::python::{
    extract_auth, extract_cert, extract_cookies_or_none, extract_headers, extract_limits,
    extract_params, extract_proxies, extract_timeout, extract_url_str, extract_verify,
    populate_request_builder, PyCookies, PyHeaders, PyRequest, PyResponse, PyTimeout, PyURL,
};
use crate::request::Request;
use crate::response::Response;
use pyo3::prelude::*;

/// Context manager returned by `Client.stream(...)` and `httpx.stream(...)`.
#[pyclass(name = "SyncStreamContext", module = "httpx")]
pub struct PySyncStreamContext {
    client: Client,
    request: Option<Request>,
    options: SendOptions,
    own_client: bool,
    response: Option<Response>,
}

#[pymethods]
impl PySyncStreamContext {
    fn __enter__(&mut self, py: Python<'_>) -> PyResult<PyResponse> {
        let req = self.request.take().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Stream context already entered")
        })?;
        let client = self.client.clone();
        let options = self.options.clone();
        let resp = py.allow_threads(|| client.send(req, options))?;
        self.response = Some(resp.clone());
        Ok(PyResponse { inner: resp })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        if let Some(resp) = self.response.take() {
            py.allow_threads(|| {
                let rt = shared_runtime()?;
                rt.block_on(resp.close());
                Ok::<_, crate::exceptions::HttpxError>(())
            })?;
        }
        if self.own_client {
            let client = self.client.clone();
            py.allow_threads(|| client.close());
        }
        Ok(false)
    }
}

/// Synchronous HTTP client compatible with `httpx.Client`.
#[pyclass(name = "Client", module = "httpx")]
#[derive(Clone)]
pub struct PyClient {
    inner: Client,
    default_timeout: TimeoutConfig,
    base_url: Option<url::Url>,
}

#[pymethods]
impl PyClient {
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
                parsed_base_url = Some(url::Url::parse(&s).map_err(|e| {
                    PyErr::from(crate::exceptions::HttpxError::InvalidUrl(e.to_string()))
                })?);
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
                builder = builder.cookies(crate::python::extract_cookie_jar(c)?);
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

        let rt = shared_runtime()?;
        let inner = builder.build_blocking(rt)?;
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

    fn close(&self, py: Python<'_>) {
        let client = self.inner.clone();
        py.allow_threads(|| client.close());
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        &self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> bool {
        self.close(py);
        false
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
        let url_str = extract_url_str(url)?;
        let builder = self.inner.build_request(method, &url_str, None)?;
        let req = populate_request_builder(
            py,
            builder,
            params,
            headers,
            cookies,
            Some(self.inner.cookies()),
            content,
            data,
            files,
            json,
            extensions,
        )?;
        Ok(PyRequest { inner: req })
    }

    #[pyo3(signature = (request, *, stream=false, auth=None, follow_redirects=None))]
    fn send(
        &self,
        py: Python<'_>,
        request: &PyRequest,
        stream: bool,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
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
        let resp = py.allow_threads(|| client.send(req, options))?;
        Ok(PyResponse { inner: resp })
    }

    #[pyo3(signature = (method, url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn request(
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
    ) -> PyResult<PyResponse> {
        let req = self.build_py_req_internal(
            py,
            method,
            url,
            content,
            data,
            files,
            json,
            params,
            headers,
            cookies,
            extensions,
        )?;
        let options = build_send_options(auth, follow_redirects, timeout, false)?;
        let client = self.inner.clone();
        let resp = py.allow_threads(|| client.send(req, options))?;
        Ok(PyResponse { inner: resp })
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
    ) -> PyResult<PySyncStreamContext> {
        let req = self.build_py_req_internal(
            py,
            method,
            url,
            content,
            data,
            files,
            json,
            params,
            headers,
            cookies,
            extensions,
        )?;
        let options = build_send_options(auth, follow_redirects, timeout, true)?;
        Ok(PySyncStreamContext {
            client: self.inner.clone(),
            request: Some(req),
            options,
            own_client: false,
            response: None,
        })
    }

    #[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, follow_redirects=None, timeout=None, extensions=None))]
    #[allow(clippy::too_many_arguments)]
    fn get(
        &self,
        py: Python<'_>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyResponse> {
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
    fn post(
        &self,
        py: Python<'_>,
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
    ) -> PyResult<PyResponse> {
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
    fn put(
        &self,
        py: Python<'_>,
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
    ) -> PyResult<PyResponse> {
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
    fn patch(
        &self,
        py: Python<'_>,
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
    ) -> PyResult<PyResponse> {
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
    fn delete(
        &self,
        py: Python<'_>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyResponse> {
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
    fn head(
        &self,
        py: Python<'_>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyResponse> {
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
    fn options(
        &self,
        py: Python<'_>,
        url: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
        headers: Option<&Bound<'_, PyAny>>,
        cookies: Option<&Bound<'_, PyAny>>,
        auth: Option<&Bound<'_, PyAny>>,
        follow_redirects: Option<bool>,
        timeout: Option<&Bound<'_, PyAny>>,
        extensions: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyResponse> {
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
        "<httpx.Client>".to_string()
    }
}

impl PyClient {
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

pub fn build_send_options(
    auth: Option<&Bound<'_, PyAny>>,
    follow_redirects: Option<bool>,
    timeout: Option<&Bound<'_, PyAny>>,
    stream: bool,
) -> PyResult<SendOptions> {
    let parsed_auth = match auth {
        Some(a) if !a.is_none() => Some(extract_auth(a)?),
        _ => None,
    };
    let parsed_timeout = match timeout {
        Some(t) => Some(extract_timeout(t)?),
        None => None,
    };
    Ok(SendOptions {
        auth: parsed_auth,
        follow_redirects,
        timeout: parsed_timeout,
        stream,
    })
}

fn build_ephemeral_client(
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    timeout: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    trust_env: bool,
) -> PyResult<Client> {
    let mut builder = ClientBuilder::new()
        .follow_redirects(follow_redirects)
        .trust_env(trust_env);
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
    if let Some(p) = proxies {
        if !p.is_none() {
            builder = builder.proxies(extract_proxies(p)?);
        }
    }
    if let Some(t) = timeout {
        builder = builder.timeout(extract_timeout(t)?);
    }
    Ok(builder.build_blocking(shared_runtime()?)?)
}

// ============================================================================
// Top-level synchronous functions (httpx.get, httpx.post, ...)
// ============================================================================

#[pyfunction]
#[pyo3(signature = (method, url, *, params=None, content=None, data=None, files=None, json=None, headers=None, cookies=None, auth=None, proxies=None, timeout=None, follow_redirects=false, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn request(
    py: Python<'_>,
    method: &str,
    url: &Bound<'_, PyAny>,
    params: Option<&Bound<'_, PyAny>>,
    content: Option<&Bound<'_, PyAny>>,
    data: Option<&Bound<'_, PyAny>>,
    files: Option<&Bound<'_, PyAny>>,
    json: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    timeout: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    let client = build_ephemeral_client(verify, cert, proxies, timeout, follow_redirects, trust_env)?;
    let url_str = extract_url_str(url)?;
    let extra_cookies = extract_cookies_or_none(cookies)?;
    let builder = client.build_request(method, &url_str, extra_cookies.as_ref())?;
    let req = populate_request_builder(
        py,
        builder,
        params,
        headers,
        None,
        Some(client.cookies()),
        content,
        data,
        files,
        json,
        extensions,
    )?;
    let options = build_send_options(auth, Some(follow_redirects), timeout, false)?;
    let resp = py.allow_threads(|| {
        let res = client.send(req, options);
        client.close();
        res
    })?;
    Ok(PyResponse { inner: resp })
}

#[pyfunction]
#[pyo3(signature = (method, url, *, params=None, content=None, data=None, files=None, json=None, headers=None, cookies=None, auth=None, proxies=None, timeout=None, follow_redirects=false, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn stream(
    py: Python<'_>,
    method: &str,
    url: &Bound<'_, PyAny>,
    params: Option<&Bound<'_, PyAny>>,
    content: Option<&Bound<'_, PyAny>>,
    data: Option<&Bound<'_, PyAny>>,
    files: Option<&Bound<'_, PyAny>>,
    json: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    timeout: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PySyncStreamContext> {
    let client = build_ephemeral_client(verify, cert, proxies, timeout, follow_redirects, trust_env)?;
    let url_str = extract_url_str(url)?;
    let extra_cookies = extract_cookies_or_none(cookies)?;
    let builder = client.build_request(method, &url_str, extra_cookies.as_ref())?;
    let req = populate_request_builder(
        py,
        builder,
        params,
        headers,
        None,
        Some(client.cookies()),
        content,
        data,
        files,
        json,
        extensions,
    )?;
    let options = build_send_options(auth, Some(follow_redirects), timeout, true)?;
    Ok(PySyncStreamContext {
        client,
        request: Some(req),
        options,
        own_client: true,
        response: None,
    })
}

#[pyfunction]
#[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn get(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "GET",
        url,
        params,
        None,
        None,
        None,
        None,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

#[pyfunction]
#[pyo3(signature = (url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn post(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    content: Option<&Bound<'_, PyAny>>,
    data: Option<&Bound<'_, PyAny>>,
    files: Option<&Bound<'_, PyAny>>,
    json: Option<&Bound<'_, PyAny>>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "POST",
        url,
        params,
        content,
        data,
        files,
        json,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

#[pyfunction]
#[pyo3(signature = (url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn put(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    content: Option<&Bound<'_, PyAny>>,
    data: Option<&Bound<'_, PyAny>>,
    files: Option<&Bound<'_, PyAny>>,
    json: Option<&Bound<'_, PyAny>>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "PUT",
        url,
        params,
        content,
        data,
        files,
        json,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

#[pyfunction]
#[pyo3(signature = (url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn patch(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    content: Option<&Bound<'_, PyAny>>,
    data: Option<&Bound<'_, PyAny>>,
    files: Option<&Bound<'_, PyAny>>,
    json: Option<&Bound<'_, PyAny>>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "PATCH",
        url,
        params,
        content,
        data,
        files,
        json,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

#[pyfunction]
#[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn delete(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "DELETE",
        url,
        params,
        None,
        None,
        None,
        None,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

#[pyfunction]
#[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn head(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "HEAD",
        url,
        params,
        None,
        None,
        None,
        None,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

#[pyfunction]
#[pyo3(signature = (url, *, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=false, timeout=None, verify=None, cert=None, trust_env=true, extensions=None))]
#[allow(clippy::too_many_arguments)]
fn options(
    py: Python<'_>,
    url: &Bound<'_, PyAny>,
    params: Option<&Bound<'_, PyAny>>,
    headers: Option<&Bound<'_, PyAny>>,
    cookies: Option<&Bound<'_, PyAny>>,
    auth: Option<&Bound<'_, PyAny>>,
    proxies: Option<&Bound<'_, PyAny>>,
    follow_redirects: bool,
    timeout: Option<&Bound<'_, PyAny>>,
    verify: Option<&Bound<'_, PyAny>>,
    cert: Option<&Bound<'_, PyAny>>,
    trust_env: bool,
    extensions: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyResponse> {
    request(
        py,
        "OPTIONS",
        url,
        params,
        None,
        None,
        None,
        None,
        headers,
        cookies,
        auth,
        proxies,
        timeout,
        follow_redirects,
        verify,
        cert,
        trust_env,
        extensions,
    )
}

/// Registers synchronous client and convenience functions on the Python module.
pub fn register_sync(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyClient>()?;
    m.add_class::<PySyncStreamContext>()?;
    m.add_function(wrap_pyfunction!(request, m)?)?;
    m.add_function(wrap_pyfunction!(stream, m)?)?;
    m.add_function(wrap_pyfunction!(get, m)?)?;
    m.add_function(wrap_pyfunction!(post, m)?)?;
    m.add_function(wrap_pyfunction!(put, m)?)?;
    m.add_function(wrap_pyfunction!(patch, m)?)?;
    m.add_function(wrap_pyfunction!(delete, m)?)?;
    m.add_function(wrap_pyfunction!(head, m)?)?;
    m.add_function(wrap_pyfunction!(options, m)?)?;
    Ok(())
}
