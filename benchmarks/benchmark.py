"""Benchmark suite comparing HttpR against Python httpx, requests, and aiohttp."""

from __future__ import annotations

import argparse
import asyncio
import json
import resource
import socket
import statistics
import subprocess
import sys
import threading
import time
import tracemalloc
from dataclasses import asdict, dataclass
from typing import Any, Callable


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


SMALL_BODY = b"ok"
JSON_BODY = json.dumps({"status": "ok", "items": list(range(50))}).encode()
LARGE_BODY = b"A" * (1024 * 1024)  # 1 MiB


def _build_http_response(status: str, content_type: str, body: bytes) -> bytes:
    header = (
        f"HTTP/1.1 {status}\r\n"
        f"Content-Type: {content_type}\r\n"
        f"Content-Length: {len(body)}\r\n"
        f"Connection: keep-alive\r\n\r\n"
    ).encode("ascii")
    return header + body


RESP_SMALL = _build_http_response("200 OK", "text/plain", SMALL_BODY)
RESP_JSON = _build_http_response("200 OK", "application/json", JSON_BODY)
RESP_LARGE = _build_http_response("200 OK", "application/octet-stream", LARGE_BODY)


class FastBenchServer:
    """High-throughput asyncio keep-alive HTTP/1.1 test server with TCP_NODELAY."""

    def __init__(self) -> None:
        self.port = _free_port()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread: threading.Thread | None = None
        self._ready = threading.Event()
        self._stop_event: asyncio.Event | None = None

    async def _handle_client(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        sock = writer.get_extra_info("socket")
        if sock is not None:
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        try:
            while True:
                head = await reader.readuntil(b"\r\n\r\n")
                first_line = head.split(b"\r\n", 1)[0]
                parts = first_line.split(b" ")
                if len(parts) < 2:
                    break
                method, path = parts[0], parts[1]

                content_length = 0
                for line in head.split(b"\r\n")[1:]:
                    if line.lower().startswith(b"content-length:"):
                        content_length = int(line.split(b":", 1)[1].strip())
                        break
                if content_length > 0:
                    await reader.readexactly(content_length)

                if method == b"POST" or path == b"/json":
                    writer.write(RESP_JSON)
                elif path == b"/large":
                    writer.write(RESP_LARGE)
                else:
                    writer.write(RESP_SMALL)
                await writer.drain()
        except (asyncio.IncompleteReadError, ConnectionResetError, BrokenPipeError):
            pass
        finally:
            writer.close()

    def start(self) -> str:
        def runner() -> None:
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            self._loop = loop
            self._stop_event = asyncio.Event()

            async def main() -> None:
                server = await asyncio.start_server(
                    self._handle_client, "127.0.0.1", self.port, backlog=1024
                )
                self._ready.set()
                assert self._stop_event is not None
                await self._stop_event.wait()
                server.close()
                await server.wait_closed()

            loop.run_until_complete(main())
            loop.close()

        self._thread = threading.Thread(target=runner, daemon=True)
        self._thread.start()
        self._ready.wait(timeout=5.0)
        return f"http://127.0.0.1:{self.port}"

    def stop(self) -> None:
        if self._loop and self._stop_event:
            self._loop.call_soon_threadsafe(self._stop_event.set)
        if self._thread:
            self._thread.join(timeout=2.0)


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
    # Warm up connection pool once before timing
    fn()

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
    await coro_fn()

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


def measure_cold_start(python_exe: str, code: str) -> float:
    t0 = time.perf_counter()
    subprocess.run([python_exe, "-c", code], check=True, stdout=subprocess.DEVNULL)
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


def bench_httpx_like(label: str, base: str, n_sync: int, n_large: int, n_async: int) -> list[BenchResult]:
    import httpx

    out: list[BenchResult] = []
    with httpx.Client(base_url=base) as c:
        out.append(measure_sync(label, "Simple GET (reuse)", n_sync, lambda: c.get("/").content))
        out.append(measure_sync(label, "JSON GET", n_sync, lambda: c.get("/json").json()))
        out.append(measure_sync(label, "POST JSON", n_sync, lambda: c.post("/post", json={"k": "v"}).json()))
        out.append(measure_sync(label, "Large Download 1MiB", n_large, lambda: c.get("/large").content))

    async def bench_async() -> None:
        async with httpx.AsyncClient(base_url=base) as ac:
            out.append(
                await measure_async_concurrent(
                    label,
                    "Async Concurrent GET",
                    n_async,
                    64,
                    lambda: ac.get("/"),
                )
            )

    asyncio.run(bench_async())
    return out


def run_benchmarks(args: argparse.Namespace) -> None:
    srv = FastBenchServer()
    base = srv.start()

    results: list[BenchResult] = []
    n_sync = args.sync_requests
    n_large = args.large_requests
    n_async = args.async_requests

    try:
        if args.mode in ("all", "httpr"):
            results.extend(bench_httpx_like("HttpR", base, n_sync, n_large, n_async))

        if args.mode in ("python-httpx",):
            results.extend(bench_httpx_like("httpx (py)", base, n_sync, n_large, n_async))

        if args.mode in ("all", "requests"):
            try:
                import requests

                with requests.Session() as s:
                    results.append(measure_sync("requests", "Simple GET (reuse)", n_sync, lambda: s.get(f"{base}/").content))
                    results.append(measure_sync("requests", "JSON GET", n_sync, lambda: s.get(f"{base}/json").json()))
                    results.append(measure_sync("requests", "POST JSON", n_sync, lambda: s.post(f"{base}/post", json={"k": "v"}).json()))
                    results.append(measure_sync("requests", "Large Download 1MiB", n_large, lambda: s.get(f"{base}/large").content))
            except ImportError:
                print("[info] 'requests' not installed; skipping requests comparison.")

        if args.mode in ("all", "aiohttp"):
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

        if args.json_out:
            payload = {
                "results": [asdict(r) for r in results],
                "cold_start_ms": measure_cold_start(
                    sys.executable, f"import httpx; httpx.get('{base}/')"
                )
                if args.mode in ("all", "httpr", "python-httpx")
                else None,
            }
            print(json.dumps(payload))
        else:
            print_table(results)
            if args.mode in ("all", "httpr", "python-httpx"):
                cold = measure_cold_start(sys.executable, f"import httpx; httpx.get('{base}/')")
                print(f"\nCold-start (import + 1 GET) {args.mode}: {cold:.2f} ms")
    finally:
        srv.stop()


def main() -> None:
    parser = argparse.ArgumentParser(description="Benchmark HttpR vs Python HTTP clients.")
    parser.add_argument(
        "--mode",
        choices=["all", "httpr", "python-httpx", "requests", "aiohttp"],
        default="all",
    )
    parser.add_argument("--sync-requests", type=int, default=1000)
    parser.add_argument("--large-requests", type=int, default=100)
    parser.add_argument("--async-requests", type=int, default=2000)
    parser.add_argument("--json", dest="json_out", action="store_true")
    args = parser.parse_args()
    run_benchmarks(args)


if __name__ == "__main__":
    main()
