"""Benchmark suite comparing httpx-rs against Python httpx, requests, and aiohttp."""

from __future__ import annotations

import asyncio
import http.server
import json
import resource
import socket
import statistics
import subprocess
import sys
import threading
import time
import tracemalloc
from dataclasses import dataclass
from typing import Any, Callable


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


class BenchHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    SMALL_BODY = b"ok"
    JSON_BODY = json.dumps({"status": "ok", "items": list(range(50))}).encode()
    LARGE_BODY = b"A" * (1024 * 1024)  # 1 MiB

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        return

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/json":
            body = self.JSON_BODY
            ct = "application/json"
        elif self.path == "/large":
            body = self.LARGE_BODY
            ct = "application/octet-stream"
        else:
            body = self.SMALL_BODY
            ct = "text/plain"

        self.send_response(200)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", "0"))
        if length > 0:
            _ = self.rfile.read(length)
        body = self.JSON_BODY
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


@dataclass
class BenchResult:
    client: str
    scenario: str
    requests: int
    total_secs: float
    rps: float
    avg_ms: float
    p50_ms: float
    p95_ms: float
    p99_ms: float
    cpu_secs: float
    peak_py_kib: float
    allocs_per_req: float
    peak_rss_mib: float


def percentile(sorted_vals: list[float], pct: float) -> float:
    if not sorted_vals:
        return 0.0
    idx = min(int(len(sorted_vals) * pct), len(sorted_vals) - 1)
    return sorted_vals[idx]


def measure_sync(
    client_name: str, scenario: str, count: int, fn: Callable[[], Any]
) -> BenchResult:
    latencies: list[float] = []
    tracemalloc.start()
    cpu_start = time.process_time()
    wall_start = time.perf_counter()

    for _ in range(count):
        t0 = time.perf_counter()
        fn()
        latencies.append((time.perf_counter() - t0) * 1000.0)

    total_secs = time.perf_counter() - wall_start
    cpu_secs = time.process_time() - cpu_start
    _, peak_bytes = tracemalloc.get_traced_memory()
    snapshot = tracemalloc.take_snapshot()
    tracemalloc.stop()

    total_allocs = sum(stat.count for stat in snapshot.statistics("lineno"))
    rss_mib = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
    latencies.sort()

    return BenchResult(
        client=client_name,
        scenario=scenario,
        requests=count,
        total_secs=total_secs,
        rps=count / total_secs,
        avg_ms=statistics.mean(latencies),
        p50_ms=percentile(latencies, 0.50),
        p95_ms=percentile(latencies, 0.95),
        p99_ms=percentile(latencies, 0.99),
        cpu_secs=cpu_secs,
        peak_py_kib=peak_bytes / 1024.0,
        allocs_per_req=total_allocs / max(count, 1),
        peak_rss_mib=rss_mib,
    )


async def measure_async_concurrent(
    client_name: str,
    scenario: str,
    total_requests: int,
    concurrency: int,
    coro_fn: Callable[[], Any],
) -> BenchResult:
    sem = asyncio.Semaphore(concurrency)
    latencies: list[float] = []

    async def worker() -> None:
        async with sem:
            t0 = time.perf_counter()
            await coro_fn()
            latencies.append((time.perf_counter() - t0) * 1000.0)

    tracemalloc.start()
    cpu_start = time.process_time()
    wall_start = time.perf_counter()

    await asyncio.gather(*(worker() for _ in range(total_requests)))

    total_secs = time.perf_counter() - wall_start
    cpu_secs = time.process_time() - cpu_start
    _, peak_bytes = tracemalloc.get_traced_memory()
    snapshot = tracemalloc.take_snapshot()
    tracemalloc.stop()

    total_allocs = sum(stat.count for stat in snapshot.statistics("lineno"))
    rss_mib = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
    latencies.sort()

    return BenchResult(
        client=client_name,
        scenario=scenario,
        requests=total_requests,
        total_secs=total_secs,
        rps=total_requests / total_secs,
        avg_ms=statistics.mean(latencies),
        p50_ms=percentile(latencies, 0.50),
        p95_ms=percentile(latencies, 0.95),
        p99_ms=percentile(latencies, 0.99),
        cpu_secs=cpu_secs,
        peak_py_kib=peak_bytes / 1024.0,
        allocs_per_req=total_allocs / max(total_requests, 1),
        peak_rss_mib=rss_mib,
    )


def measure_cold_start(module_expr: str, url: str) -> float:
    code = f"{module_expr}('{url}')"
    t0 = time.perf_counter()
    subprocess.run([sys.executable, "-c", code], check=True, stdout=subprocess.DEVNULL)
    return (time.perf_counter() - t0) * 1000.0


def print_table(results: list[BenchResult]) -> None:
    header = (
        f"{'Client':<14} {'Scenario':<22} {'RPS':>9} {'Avg(ms)':>9} "
        f"{'P50(ms)':>9} {'P95(ms)':>9} {'P99(ms)':>9} {'CPU(s)':>8} "
        f"{'PyMem(KiB)':>11} {'Allocs/req':>11} {'RSS(MiB)':>9}"
    )
    print(header)
    print("-" * len(header))
    for r in results:
        print(
            f"{r.client:<14} {r.scenario:<22} {r.rps:>9.1f} {r.avg_ms:>9.3f} "
            f"{r.p50_ms:>9.3f} {r.p95_ms:>9.3f} {r.p99_ms:>9.3f} {r.cpu_secs:>8.3f} "
            f"{r.peak_py_kib:>11.1f} {r.allocs_per_req:>11.1f} {r.peak_rss_mib:>9.1f}"
        )


def run_benchmarks() -> None:
    port = _free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), BenchHandler)
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    base = f"http://127.0.0.1:{port}"

    results: list[BenchResult] = []
    n_sync = 500
    n_large = 50
    n_async = 1000

    try:
        # 1. httpx-rs (exposed as httpx)
        import httpx as httpx_rs

        with httpx_rs.Client(base_url=base) as c:
            results.append(measure_sync("httpx-rs", "Simple GET (reuse)", n_sync, lambda: c.get("/").content))
            results.append(measure_sync("httpx-rs", "JSON GET", n_sync, lambda: c.get("/json").json()))
            results.append(measure_sync("httpx-rs", "POST JSON", n_sync, lambda: c.post("/post", json={"k": "v"}).json()))
            results.append(measure_sync("httpx-rs", "Large Download 1MiB", n_large, lambda: c.get("/large").content))

        async def bench_rs_async() -> None:
            async with httpx_rs.AsyncClient(base_url=base) as ac:
                results.append(
                    await measure_async_concurrent(
                        "httpx-rs",
                        "Async Concurrent GET",
                        n_async,
                        64,
                        lambda: ac.get("/"),
                    )
                )

        asyncio.run(bench_rs_async())

        # 2. Optional comparison libraries if installed in the benchmark environment
        try:
            import requests

            with requests.Session() as s:
                results.append(measure_sync("requests", "Simple GET (reuse)", n_sync, lambda: s.get(f"{base}/").content))
                results.append(measure_sync("requests", "JSON GET", n_sync, lambda: s.get(f"{base}/json").json()))
                results.append(measure_sync("requests", "POST JSON", n_sync, lambda: s.post(f"{base}/post", json={"k": "v"}).json()))
                results.append(measure_sync("requests", "Large Download 1MiB", n_large, lambda: s.get(f"{base}/large").content))
        except ImportError:
            print("[info] 'requests' not installed; skipping requests comparison.")

        try:
            import aiohttp

            async def bench_aiohttp() -> None:
                async with aiohttp.ClientSession() as s:
                    async def fetch() -> bytes:
                        async with s.get(f"{base}/") as resp:
                            return await resp.read()

                    results.append(
                        await measure_async_concurrent(
                            "aiohttp", "Async Concurrent GET", n_async, 64, fetch
                        )
                    )

            asyncio.run(bench_aiohttp())
        except ImportError:
            print("[info] 'aiohttp' not installed; skipping aiohttp comparison.")

        print_table(results)

        cold_rs = measure_cold_start("import httpx; httpx.get", f"{base}/")
        print(f"\nCold-start (import + 1 GET) httpx-rs: {cold_rs:.2f} ms")
    finally:
        srv.shutdown()
        srv.server_close()


if __name__ == "__main__":
    run_benchmarks()
