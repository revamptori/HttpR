# HttpR Benchmarks

Benchmark comparison of **HttpR** (`0.1.0`) against Python **httpx** (`0.28.1`), **requests** (`2.34.2`), and **aiohttp** (`3.14.5`) under identical local server conditions.

## Test Environment

- **OS / Arch:** Linux `x86_64` (Debian 12, 2 vCPUs)
- **Python:** `3.11.2`
- **Server:** Local `asyncio` HTTP/1.1 keep-alive server with `TCP_NODELAY` (`benchmarks/benchmark.py`)
- **Workload:**
  - `Simple GET (reuse)`: 1,000 sequential requests reusing a pooled connection
  - `JSON GET`: 1,000 sequential JSON responses parsed to Python objects
  - `POST JSON`: 1,000 sequential JSON POST requests + JSON response parsing
  - `Large Download 1MiB`: 100 sequential 1 MiB downloads
  - `Async Concurrent GET`: 2,000 concurrent async GET requests with concurrency limit `64`
  - `Cold-start`: Fresh Python subprocess importing the client and performing 1 GET request

## Results

| Client | Scenario | RPS | Avg (ms) | P50 (ms) | P95 (ms) | P99 (ms) | CPU (s) | Peak PyMem (KiB) | Allocs/req | Peak RSS (MiB) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **HttpR** | Simple GET (reuse) | **2,296.0** | **0.432** | 0.391 | 0.597 | 1.561 | **0.450** | **288.4** | **1.0** | **28.1** |
| httpx (py) | Simple GET (reuse) | 373.0 | 2.677 | 2.934 | 3.678 | 5.084 | 2.673 | 523.4 | 3.6 | 32.4 |
| requests | Simple GET (reuse) | 305.4 | 3.270 | 3.383 | 3.901 | 6.444 | 3.258 | 310.1 | 1.1 | 48.4 |
| **HttpR** | JSON GET | **3,387.6** | **0.292** | 0.297 | 0.381 | 0.425 | **0.332** | **287.2** | **0.9** | **28.1** |
| httpx (py) | JSON GET | 328.4 | 3.041 | 3.216 | 3.931 | 4.943 | 3.035 | 554.9 | 3.4 | 32.4 |
| requests | JSON GET | 240.8 | 4.147 | 3.831 | 6.693 | 10.022 | 4.137 | 303.2 | 1.0 | 48.4 |
| **HttpR** | POST JSON | **3,871.4** | **0.256** | 0.249 | 0.316 | 0.388 | **0.290** | **287.3** | **0.9** | **28.1** |
| httpx (py) | POST JSON | 266.1 | 3.753 | 3.610 | 5.091 | 7.114 | 3.779 | 585.3 | 3.8 | 32.7 |
| requests | POST JSON | 255.2 | 3.914 | 3.900 | 5.210 | 8.957 | 3.886 | 304.3 | 1.0 | 48.4 |
| **HttpR** | Large Download 1MiB | **1,299.8** | **0.768** | 0.724 | 1.067 | 1.875 | **0.097** | **1,026.9** | **0.2** | **33.8** |
| httpx (py) | Large Download 1MiB | 167.9 | 5.952 | 5.934 | 6.549 | 9.229 | 0.621 | 26,762.1 | 9.3 | 58.9 |
| requests | Large Download 1MiB | 100.9 | 9.909 | 9.669 | 11.798 | 12.775 | 1.050 | 2,072.5 | 0.6 | 48.6 |
| **HttpR** | Async Concurrent GET | **2,007.9** | **30.500** | 27.918 | 55.439 | 107.186 | **1.141** | **3,551.0** | **9.2** | **45.4** |
| aiohttp | Async Concurrent GET | 1,008.7 | 54.009 | 53.252 | 77.711 | 138.465 | 2.047 | 4,325.6 | 10.3 | 63.8 |
| httpx (py) | Async Concurrent GET | 73.5 | 831.549 | 688.660 | 2,208.579 | 4,029.398 | 28.501 | 6,111.8 | 26.3 | 58.9 |

### Cold-Start Overhead (`import + 1 GET`)

| Client | Cold-Start Latency | Speedup |
|---|---:|---|
| **HttpR** | **18.66 ms** | **8.0× faster** |
| httpx (py) | 149.23 ms | 1.0× |

### Key Takeaways vs Python `httpx`

- **6.2×–14.5× higher sync throughput** (`Simple GET`, `JSON GET`, `POST JSON`)
- **27.3× higher async concurrency throughput** (`2,007.9` vs `73.5` RPS) and **2.0× faster than `aiohttp`**
- **26× lower peak Python heap allocation** on 1 MiB downloads (`1,026.9 KiB` vs `26,762.1 KiB`)
- **8.0× faster cold-start** (`18.66 ms` vs `149.23 ms`)

## Reproducing

```bash
python benchmarks/benchmark.py --mode httpr
python benchmarks/benchmark.py --mode requests
python benchmarks/benchmark.py --mode aiohttp
# In a separate venv with upstream httpx installed:
python benchmarks/benchmark.py --mode python-httpx
```
