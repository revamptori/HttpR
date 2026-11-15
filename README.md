# httpx-rs

A high-performance, HTTPX-compatible HTTP client for Python built from scratch in Rust using **Tokio**, **Hyper**, **Rustls**, **Tower**, and **PyO3**.

## Features

- **100% Rust HTTP Engine**: All TCP/TLS networking, DNS resolution (IPv4/IPv6), HTTP/1.1 & HTTP/2 protocol handling, connection pooling, gzip/deflate decompression, and JSON/multipart encoding run natively in Rust.
- **HTTPX-Compatible Python API**:
  - Top-level functions: `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `request`, `stream`
  - Synchronous `httpx.Client` (with context manager & shared Tokio runtime, releasing the GIL during I/O)
  - Asynchronous `httpx.AsyncClient` (with `async with`, non-blocking Tokio execution via `pyo3-async-runtimes`)
  - Streaming responses (`iter_bytes`, `iter_text`, `iter_lines`, `aiter_bytes`, `aiter_text`, `aiter_lines`) and streaming request bodies
  - Full HTTPX exception hierarchy (`HTTPError`, `RequestError`, `TransportError`, `TimeoutException`, `ConnectTimeout`, `ReadTimeout`, `WriteTimeout`, `PoolTimeout`, `ConnectError`, `HTTPStatusError`, `InvalidURL`, `TooManyRedirects`, `UnsupportedProtocol`, `ProxyError`, `DecodingError`, etc.)
- **Modern Protocols & Security**:
  - HTTP/1.1 keep-alive & HTTP/2 multiplexing via ALPN (`h2`)
  - TLS via `rustls` with system + Mozilla WebPKI roots, custom CA bundles (`verify=...`), mTLS client certificates (`cert=...`), or opt-out verification (`verify=False`)
  - HTTP, HTTPS, and SOCKS5 (`socks5://`, `socks5h://`) proxies plus `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` environment support
  - Basic, Bearer, and Digest authentication

## Architecture

```text
src/
├── lib.rs
├── client/          # Client, AsyncClient, ClientBuilder, ClientConfig
├── transport/       # HTTP/1.1, HTTP/2, Rustls TLS, ConnectionPool
├── request/         # Request, RequestBuilder, Headers, Cookies, Multipart
├── response/        # Response (lazy decoding), ResponseStream
├── auth/            # BasicAuth, BearerAuth, DigestAuth
├── proxy/           # ProxyConfig, ProxyMounts, NO_PROXY & env discovery
├── middleware/      # Redirects, Retries, Timeouts, Tower Service integration
├── exceptions/      # Rust HttpxError & Python exception hierarchy
└── python/          # PyO3 native Python bindings (sync & async)
```

## Installation & Development

```bash
# Build and install into the active Python environment with Maturin
pip install maturin
maturin develop --release

# Or install as a standard wheel via pip
pip install .
```

## Usage

### Synchronous

```python
import httpx

response = httpx.get("https://example.com", timeout=5.0)
response.raise_for_status()
print(response.status_code, response.http_version)
print(response.text)

with httpx.Client(base_url="https://api.example.com", http2=True) as client:
    r = client.post("/items", json={"name": "widget"})
    print(r.json())
```

### Asynchronous

```python
import asyncio
import httpx

async def main() -> None:
    async with httpx.AsyncClient(http2=True) as client:
        r = await client.get("https://example.com")
        print(r.status_code, r.elapsed)

asyncio.run(main())
```

## Testing & Benchmarking

```bash
# Rust engine tests
cargo test

# Python API compatibility tests
pytest

# Benchmarks against requests / aiohttp / httpx
python benchmarks/benchmark.py
```
