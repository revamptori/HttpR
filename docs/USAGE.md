# HttpR Coding Guide & API Syntax Reference

HttpR is API-compatible with Python's `httpx` while running the entire networking, TLS, HTTP/1.1, HTTP/2, pooling, and parsing engine in Rust.

You can import either `httpr` directly or `httpx` as a drop-in replacement:

```python
import httpr
# or:
import httpx
```

---

## 1. Quick Start (Top-Level Functions)

For one-off synchronous requests:

```python
import httpr

r = httpr.get("https://httpbin.org/get", params={"page": 1, "tag": ["a", "b"]})
print(r.status_code)   # 200
print(r.http_version)  # "HTTP/1.1" or "HTTP/2"
print(r.headers["content-type"])
print(r.text)
data = r.json()
```

All standard HTTP methods are available at module level:

```python
httpr.get(url, *, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=False, timeout=None, verify=None, cert=None, trust_env=True, extensions=None)
httpr.post(url, *, content=None, data=None, files=None, json=None, params=None, headers=None, cookies=None, auth=None, proxies=None, follow_redirects=False, timeout=None, verify=None, cert=None, trust_env=True, extensions=None)
httpr.put(url, ...)
httpr.patch(url, ...)
httpr.delete(url, ...)
httpr.head(url, ...)
httpr.options(url, ...)
httpr.request(method, url, ...)
```

---

## 2. Synchronous `Client` (Connection Pooling)

Use `httpr.Client` as a context manager to reuse TCP/TLS connections and persist cookies across requests:

```python
import httpr

with httpr.Client(
    base_url="https://api.example.com",
    headers={"X-App": "my-service"},
    timeout=10.0,
    http2=True,
    follow_redirects=True,
) as client:
    r = client.get("/users", params={"active": True})
    r.raise_for_status()
    users = r.json()

    created = client.post("/users", json={"name": "Ada"})
    print(created.status_code, created.elapsed)
```

If you don't use a `with` block, close the client explicitly:

```python
client = httpr.Client()
try:
    r = client.get("https://example.com")
finally:
    client.close()
```

---

## 3. Asynchronous `AsyncClient`

Use `httpr.AsyncClient` with `async with` for concurrent async I/O powered by Tokio:

```python
import asyncio
import httpr

async def main() -> None:
    async with httpr.AsyncClient(base_url="https://api.example.com", http2=True) as client:
        responses = await asyncio.gather(
            client.get("/items/1"),
            client.get("/items/2"),
            client.post("/items", json={"name": "widget"}),
        )
        for r in responses:
            r.raise_for_status()
            print(r.status_code, r.json())

asyncio.run(main())
```

Or manage closing manually with `await client.aclose()`.

---

## 4. Request Bodies (`json`, `data`, `files`, `content`)

### JSON payloads

```python
r = client.post("/json", json={"id": 123, "tags": ["rust", "python"]})
```

### Form-encoded data (`application/x-www-form-urlencoded`)

```python
r = client.post("/login", data={"username": "alice", "password": "secret"})
```

### Multipart file uploads (`multipart/form-data`)

```python
# Tuple forms: (filename, content), (filename, content, content_type), or (filename, content, content_type, headers)
with open("report.pdf", "rb") as f:
    r = client.post(
        "/upload",
        data={"description": "Q4 report"},
        files={
            "file": ("report.pdf", f, "application/pdf"),
            "notes": ("notes.txt", b"plain bytes", "text/plain", {"X-Part": "1"}),
        },
    )
```

### Raw bytes / text / streaming request bodies

```python
# Raw bytes or string
client.put("/blob", content=b"\x00\x01\x02", headers={"Content-Type": "application/octet-stream"})

# Synchronous streaming request body
def gen_chunks():
    yield b"hello "
    yield b"world"

client.post("/stream-up", content=gen_chunks())

# Asynchronous streaming request body (AsyncClient)
async def async_chunks():
    yield b"part-1"
    await asyncio.sleep(0.01)
    yield b"part-2"

await async_client.post("/stream-up", content=async_chunks())
```

---

## 5. Building & Sending Requests Manually

```python
req = client.build_request("POST", "/items", json={"item": "book"})
req.headers["X-Signature"] = "abc123"
resp = client.send(req, follow_redirects=True)

# Standalone Request object
standalone = httpr.Request("GET", "https://example.com/search", params={"q": "httpr"})
resp = client.send(standalone)
```

---

## 6. Streaming Responses

Use `stream(...)` when downloading large payloads or reading chunked/SSE/line-oriented responses without buffering the entire body into RAM:

### Synchronous streaming

```python
with client.stream("GET", "/large-file") as r:
    r.raise_for_status()
    for chunk in r.iter_bytes(chunk_size=65536):
        process(chunk)

# Line-by-line or decoded text streaming
with httpr.stream("GET", "https://example.com/events") as r:
    for line in r.iter_lines():
        print(line)
```

### Asynchronous streaming

```python
async with async_client.stream("GET", "/large-file") as r:
    r.raise_for_status()
    async for chunk in r.aiter_bytes():
        process(chunk)

# Or read full body later on a streamed response:
async with async_client.stream("GET", "/data") as r:
    raw = await r.aread()
    print(r.text)
```

---

## 7. Timeouts & Connection Pool Limits

```python
# Uniform 10s timeout across all phases
client = httpr.Client(timeout=10.0)

# Disable all timeouts
client = httpr.Client(timeout=None)

# Granular 4-phase timeout (seconds)
timeout = httpr.Timeout(5.0, connect=2.0, read=30.0, write=10.0, pool=5.0)
client = httpr.Client(timeout=timeout)

# Per-request timeout override
r = client.get("/slow", timeout=httpr.Timeout(1.0, read=60.0))

# Connection pool limits
limits = httpr.Limits(
    max_connections=100,
    max_keepalive_connections=20,
    keepalive_expiry=15.0,
)
client = httpr.Client(limits=limits)
```

---

## 8. Authentication

```python
# Basic Auth via tuple or BasicAuth
httpr.get("https://example.com/private", auth=("alice", "secret"))
httpr.get("https://example.com/private", auth=httpr.BasicAuth("alice", "secret"))

# Bearer Token Auth
client = httpr.Client(auth=httpr.BearerAuth("my-jwt-token"))

# HTTP Digest Auth (MD5 / SHA-256, handles 401 challenge automatically)
client = httpr.Client(auth=httpr.DigestAuth("alice", "secret"))
```

---

## 9. TLS Verification, Custom CAs & Client Certificates (mTLS)

```python
# Default: system + Mozilla WebPKI root CAs
httpr.get("https://example.com", verify=True)

# Custom CA PEM bundle (file or directory)
httpr.get("https://internal.example.com", verify="/path/to/ca-bundle.pem")

# Disable TLS verification (development/testing only)
httpr.get("https://self-signed.local", verify=False)

# Client mTLS certificate + key
client = httpr.Client(cert=("/path/to/client.crt", "/path/to/client.key"))
# Or combined PEM file:
client = httpr.Client(cert="/path/to/client.pem")
```

---

## 10. Proxies (HTTP, HTTPS, SOCKS5) & Environment Variables

```python
# Single proxy for all traffic
client = httpr.Client(proxies="http://127.0.0.1:8080")

# Per-scheme or per-host proxy mounts (None bypasses proxy for that pattern)
client = httpr.Client(
    proxies={
        "http://": "http://proxy.internal:8080",
        "https://": "socks5h://user:pass@127.0.0.1:1080",
        "https://localhost": None,
    }
)

# Disable reading HTTP_PROXY / HTTPS_PROXY / ALL_PROXY / NO_PROXY from environment
client = httpr.Client(trust_env=False)
```

---

## 11. Cookies, Headers, URLs & Status Helpers

```python
# Headers (case-insensitive, multi-value)
headers = httpr.Headers({"Content-Type": "application/json", "X-Custom": "1"})
print(headers["content-type"])
print(headers.multi_items())

# Cookies
cookies = httpr.Cookies({"session": "abc"})
cookies.set("prefs", "dark", domain="example.com", path="/")
client = httpr.Client(cookies=cookies)

# URL inspection
url = httpr.URL("https://example.com:8443/v1/items?page=2#top")
print(url.scheme, url.host, url.port, url.path, url.query, url.fragment)
next_url = url.join("/v1/items/3")

# Response helpers
r = httpr.get("https://example.com")
if r.is_success:          # 2xx (also: is_informational, is_redirect, is_client_error, is_server_error, is_error)
    print(r.status_code == httpr.codes.OK)
```

---

## 12. Exception Handling

All HTTPX-compatible exceptions are exposed at top level (`httpr.<Exception>`):

```text
Exception
├── InvalidURL
├── StreamError (RuntimeError)
└── HTTPError
    ├── HTTPStatusError
    └── RequestError
        ├── DecodingError
        ├── TooManyRedirects
        └── TransportError
            ├── UnsupportedProtocol
            ├── ProxyError
            ├── ProtocolError
            │   ├── LocalProtocolError
            │   └── RemoteProtocolError
            ├── NetworkError
            │   ├── ConnectError
            │   ├── ReadError
            │   ├── WriteError
            │   └── CloseError
            └── TimeoutException
                ├── ConnectTimeout
                ├── ReadTimeout
                ├── WriteTimeout
                └── PoolTimeout
```

Example:

```python
try:
    r = httpr.get("https://example.com/resource", timeout=2.0)
    r.raise_for_status()
except httpr.TimeoutException as exc:
    print("Timed out:", exc)
except httpr.HTTPStatusError as exc:
    print(f"Server returned {exc.response.status_code}: {exc}")
except httpr.HTTPError as exc:
    print("Request failed:", exc)
```
