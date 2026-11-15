# HttpR

A high-performance, HTTPX-compatible HTTP client for Python built from scratch in Rust using **Tokio**, **Hyper**, **Rustls**, **Tower**, and **PyO3**.

## Features

- **100% Rust HTTP Engine**: All TCP/TLS networking, DNS resolution (IPv4/IPv6), HTTP/1.1 & HTTP/2 protocol handling, connection pooling, gzip/deflate decompression, and JSON/multipart encoding run natively in Rust.
- **HTTPX-Compatible Python API (`import httpr` or `import httpx`)**:
  - Top-level functions: `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `request`, `stream`
  - Synchronous `Client` (with context manager & shared Tokio runtime, releasing the GIL during I/O)
  - Asynchronous `AsyncClient` (with `async with`, non-blocking Tokio execution via `pyo3-async-runtimes`)
  - Streaming responses (`iter_bytes`, `iter_text`, `iter_lines`, `aiter_bytes`, `aiter_text`, `aiter_lines`) and streaming request bodies
  - Full HTTPX exception hierarchy (`HTTPError`, `RequestError`, `TransportError`, `TimeoutException`, `ConnectTimeout`, `ReadTimeout`, `WriteTimeout`, `PoolTimeout`, `ConnectError`, `HTTPStatusError`, `InvalidURL`, `TooManyRedirects`, `UnsupportedProtocol`, `ProxyError`, `DecodingError`, etc.)
- **Modern Protocols & Security**:
  - HTTP/1.1 keep-alive & HTTP/2 multiplexing via ALPN (`h2`)
  - TLS via `rustls` with system + Mozilla WebPKI roots, custom CA bundles (`verify=...`), mTLS client certificates (`cert=...`), or opt-out verification (`verify=False`)
  - HTTP, HTTPS, and SOCKS5 (`socks5://`, `socks5h://`) proxies plus `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` environment support
  - Basic, Bearer, and Digest authentication

## Benchmarks vs `httpx` (Python), `requests`, and `aiohttp`

Measured on Linux `x86_64` (Python 3.11) against a local `asyncio` HTTP/1.1 keep-alive server ([full results & methodology](benchmarks/RESULTS.md)):

| Scenario | HttpR | httpx (py) | requests | aiohttp |
|---|---:|---:|---:|---:|
| Simple GET (reuse) | **2,296 RPS** (0.43 ms) | 373 RPS (2.68 ms) | 305 RPS (3.27 ms) | — |
| JSON GET | **3,388 RPS** (0.29 ms) | 328 RPS (3.04 ms) | 241 RPS (4.15 ms) | — |
| POST JSON | **3,871 RPS** (0.26 ms) | 266 RPS (3.75 ms) | 255 RPS (3.91 ms) | — |
| Large Download (1 MiB) | **1,300 RPS** (1.0 MiB PyMem) | 168 RPS (26.1 MiB PyMem) | 101 RPS (2.0 MiB PyMem) | — |
| Async Concurrent GET (c=64) | **2,008 RPS** | 74 RPS | — | 1,009 RPS |
| Cold-start (`import` + 1 GET) | **18.7 ms** | 149.2 ms | — | — |

## Installation

### From GitHub (with Rust toolchain installed)

```bash
# Using pip3
pip3 install "httpr @ git+https://github.com/revamptori/HttpR.git"

# Using uv
uv add "git+https://github.com/revamptori/HttpR.git"
```

### Local Development Build

```bash
pip install maturin
maturin develop --release
```

### PyPI Release Setup (Maintainers)

`.github/workflows/release.yml` automatically builds precompiled `abi3` wheels for Linux (`manylinux_2_28` `x86_64`/`aarch64`), macOS (`x86_64`/`arm64`), Windows (`x64`), and `sdist` when a `v*` tag is pushed.

> **Note:** The distribution name `httpr` is already occupied on public PyPI by an unrelated package (`httpr 0.7.8`). Before publishing to PyPI for `pip3 install <name>` / `uv add <name>`, update `[project].name` in `pyproject.toml` to an available PyPI name (for example `httpr-py` or `pyhttpr`) and configure PyPI Trusted Publishing for `.github/workflows/release.yml`.

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

## Usage

See the full **[HttpR Coding Guide & API Syntax Reference](docs/USAGE.md)** for complete examples covering streaming, multipart uploads, timeouts, TLS/mTLS, proxies, authentication, cookies, and error handling.

You can import `httpr` directly (or `httpx` as a drop-in replacement):

### Synchronous

```python
import httpr

response = httpr.get("https://example.com", timeout=5.0)
response.raise_for_status()
print(response.status_code, response.http_version)
print(response.text)

with httpr.Client(base_url="https://api.example.com", http2=True) as client:
    r = client.post("/items", json={"name": "widget"})
    print(r.json())
```

### Asynchronous

```python
import asyncio
import httpr

async def main() -> None:
    async with httpr.AsyncClient(http2=True) as client:
        r = await client.get("https://example.com")
        print(r.status_code, r.elapsed)

asyncio.run(main())
```

### Streaming, Auth, Timeouts & Proxies

```python
import httpr

with httpr.Client(
    auth=httpr.BearerAuth("my-token"),
    timeout=httpr.Timeout(5.0, connect=2.0, read=30.0),
    proxies={"https://": "socks5h://127.0.0.1:1080"},
    follow_redirects=True,
) as client:
    with client.stream("GET", "https://example.com/large") as resp:
        resp.raise_for_status()
        for chunk in resp.iter_bytes(chunk_size=65536):
            process(chunk)
```

## Testing & Benchmarking

```bash
# Rust engine tests
cargo test

# Python API compatibility tests
pytest -v

# Benchmarks against httpx / requests / aiohttp
python benchmarks/benchmark.py --mode httpr
```
