"""Comprehensive Python compatibility test suite for httpx-rs using local test servers."""

from __future__ import annotations

import asyncio
import base64
import gzip
import http.server
import json
import os
import socket
import ssl
import subprocess
import tempfile
import threading
import time
from typing import Any

import httpx
import pytest


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


class _MockHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_conn_ids: set[int] = set()

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        return

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", "0"))
        return self.rfile.read(length) if length > 0 else b""

    def _send_json(self, status: int, payload: Any, headers: dict[str, str] | None = None) -> None:
        raw = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(raw)))
        if headers:
            for k, v in headers.items():
                self.send_header(k, v)
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self) -> None:  # noqa: N802
        _MockHandler.server_conn_ids.add(id(self.connection))
        if self.path.startswith("/echo"):
            self._send_json(
                200,
                {
                    "method": "GET",
                    "path": self.path,
                    "headers": {k.lower(): v for k, v in self.headers.items()},
                },
            )
        elif self.path == "/set-cookie":
            self._send_json(
                200,
                {"ok": True},
                headers={"Set-Cookie": "session=xyz789; Path=/"},
            )
        elif self.path == "/check-cookie":
            self._send_json(200, {"cookie": self.headers.get("Cookie", "")})
        elif self.path == "/redirect-1":
            self.send_response(302)
            self.send_header("Location", "/redirect-2")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif self.path == "/redirect-2":
            self._send_json(200, {"landed": True})
        elif self.path == "/redirect-loop":
            self.send_response(302)
            self.send_header("Location", "/redirect-loop")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif self.path == "/slow":
            time.sleep(0.4)
            self._send_json(200, {"slow": True})
        elif self.path == "/gzip":
            compressed = gzip.compress(b'{"compressed": true}')
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Encoding", "gzip")
            self.send_header("Content-Length", str(len(compressed)))
            self.end_headers()
            self.wfile.write(compressed)
        elif self.path == "/stream":
            chunks = [b"chunk-1\n", b"chunk-2\n", b"chunk-3\n"]
            total = sum(len(c) for c in chunks)
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(total))
            self.end_headers()
            for c in chunks:
                self.wfile.write(c)
                self.wfile.flush()
                time.sleep(0.01)
        elif self.path == "/large":
            body = b"X" * (2 * 1024 * 1024)
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/basic-auth":
            auth = self.headers.get("Authorization", "")
            expected = "Basic " + base64.b64encode(b"alice:secret").decode()
            if auth == expected:
                self._send_json(200, {"user": "alice"})
            else:
                self._send_json(401, {"error": "unauthorized"})
        elif self.path == "/bearer-auth":
            if self.headers.get("Authorization") == "Bearer my-token":
                self._send_json(200, {"ok": True})
            else:
                self._send_json(401, {"error": "unauthorized"})
        elif self.path == "/digest-auth":
            auth = self.headers.get("Authorization", "")
            if auth.startswith("Digest ") and 'username="mufasa"' in auth:
                self._send_json(200, {"digest": "ok"})
            else:
                self.send_response(401)
                self.send_header(
                    "WWW-Authenticate",
                    'Digest realm="test", qop="auth", nonce="abcdef012345"',
                )
                self.send_header("Content-Length", "0")
                self.end_headers()
        elif self.path == "/status/404":
            self._send_json(404, {"detail": "not found"})
        else:
            self._send_json(200, {"path": self.path})

    def do_POST(self) -> None:  # noqa: N802
        body = self._read_body()
        self._send_json(
            200,
            {
                "method": "POST",
                "content_type": self.headers.get("Content-Type", ""),
                "body": body.decode("utf-8", errors="replace"),
            },
        )

    def do_PUT(self) -> None:  # noqa: N802
        body = self._read_body()
        self._send_json(200, {"method": "PUT", "len": len(body)})

    def do_PATCH(self) -> None:  # noqa: N802
        body = self._read_body()
        self._send_json(200, {"method": "PATCH", "len": len(body)})

    def do_DELETE(self) -> None:  # noqa: N802
        self._send_json(200, {"method": "DELETE"})

    def do_HEAD(self) -> None:  # noqa: N802
        self.send_response(200)
        self.send_header("Content-Length", "10")
        self.end_headers()

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(204)
        self.send_header("Allow", "GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS")
        self.send_header("Content-Length", "0")
        self.end_headers()


class SimpleHttpProxyHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen_requests: list[str] = []

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        return

    def do_GET(self) -> None:  # noqa: N802
        SimpleHttpProxyHandler.seen_requests.append(self.path)
        payload = json.dumps({"proxied": True, "target": self.path}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


@pytest.fixture(scope="module")
def http_server() -> str:
    port = _free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), _MockHandler)
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    yield f"http://127.0.0.1:{port}"
    srv.shutdown()
    srv.server_close()


@pytest.fixture(scope="module")
def https_server() -> tuple[str, str]:
    tmp = tempfile.TemporaryDirectory()
    ca_key = os.path.join(tmp.name, "ca.key")
    ca_cert = os.path.join(tmp.name, "ca.pem")
    srv_key = os.path.join(tmp.name, "srv.key")
    srv_csr = os.path.join(tmp.name, "srv.csr")
    srv_cert = os.path.join(tmp.name, "srv.pem")
    ext_cfg = os.path.join(tmp.name, "ext.cnf")
    with open(ext_cfg, "w", encoding="utf-8") as f:
        f.write(
            "basicConstraints=CA:FALSE\n"
            "keyUsage=digitalSignature,keyEncipherment\n"
            "extendedKeyUsage=serverAuth\n"
            "subjectAltName=IP:127.0.0.1,DNS:localhost\n"
        )

    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            ca_key,
            "-out",
            ca_cert,
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=httpx-rs-test-ca",
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        [
            "openssl",
            "req",
            "-newkey",
            "rsa:2048",
            "-keyout",
            srv_key,
            "-out",
            srv_csr,
            "-nodes",
            "-subj",
            "/CN=127.0.0.1",
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        [
            "openssl",
            "x509",
            "-req",
            "-in",
            srv_csr,
            "-CA",
            ca_cert,
            "-CAkey",
            ca_key,
            "-CAcreateserial",
            "-out",
            srv_cert,
            "-days",
            "1",
            "-extfile",
            ext_cfg,
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    port = _free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), _MockHandler)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(certfile=srv_cert, keyfile=srv_key)
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    yield f"https://127.0.0.1:{port}", ca_cert
    srv.shutdown()
    srv.server_close()
    tmp.cleanup()


@pytest.fixture(scope="module")
def proxy_server() -> str:
    port = _free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), SimpleHttpProxyHandler)
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    yield f"http://127.0.0.1:{port}"
    srv.shutdown()
    srv.server_close()


def test_exception_hierarchy() -> None:
    assert issubclass(httpx.RequestError, httpx.HTTPError)
    assert issubclass(httpx.TransportError, httpx.RequestError)
    assert issubclass(httpx.TimeoutException, httpx.TransportError)
    assert issubclass(httpx.ConnectTimeout, httpx.TimeoutException)
    assert issubclass(httpx.ReadTimeout, httpx.TimeoutException)
    assert issubclass(httpx.WriteTimeout, httpx.TimeoutException)
    assert issubclass(httpx.PoolTimeout, httpx.TimeoutException)
    assert issubclass(httpx.ConnectError, httpx.TransportError)
    assert issubclass(httpx.ProxyError, httpx.TransportError)
    assert issubclass(httpx.UnsupportedProtocol, httpx.TransportError)
    assert issubclass(httpx.TooManyRedirects, httpx.RequestError)
    assert issubclass(httpx.DecodingError, httpx.RequestError)
    assert issubclass(httpx.HTTPStatusError, httpx.HTTPError)


def test_sync_methods_and_client(http_server: str) -> None:
    resp = httpx.get(f"{http_server}/echo", params={"a": "1"}, headers={"X-Test": "yes"})
    assert resp.status_code == 200
    assert resp.is_success
    assert resp.http_version == "HTTP/1.1"
    data = resp.json()
    assert data["headers"]["x-test"] == "yes"
    assert "a=1" in data["path"]

    with httpx.Client(base_url=http_server) as client:
        assert client.post("/items", json={"x": 42}).json()["content_type"] == "application/json"
        assert client.put("/items", content=b"abc").json()["len"] == 3
        assert client.patch("/items", data={"k": "v"}).json()["method"] == "PATCH"
        assert client.delete("/items").json()["method"] == "DELETE"
        assert client.head("/items").status_code == 200
        assert client.options("/items").status_code == 204

        req = client.build_request("GET", "/echo")
        sent = client.send(req)
        assert sent.status_code == 200


def test_https_and_invalid_certificate(https_server: tuple[str, str]) -> None:
    url, cert_path = https_server
    with pytest.raises(httpx.ConnectError):
        httpx.get(f"{url}/echo", verify=True)

    resp_unverified = httpx.get(f"{url}/echo", verify=False)
    assert resp_unverified.status_code == 200

    resp_ca = httpx.get(f"{url}/echo", verify=cert_path)
    assert resp_ca.status_code == 200


def test_redirects_and_too_many_redirects(http_server: str) -> None:
    r_no = httpx.get(f"{http_server}/redirect-1", follow_redirects=False)
    assert r_no.status_code == 302
    assert r_no.is_redirect

    r_yes = httpx.get(f"{http_server}/redirect-1", follow_redirects=True)
    assert r_yes.status_code == 200
    assert r_yes.json()["landed"] is True
    assert len(r_yes.history) == 1

    with httpx.Client(follow_redirects=True, max_redirects=2) as client:
        with pytest.raises(httpx.TooManyRedirects):
            client.get(f"{http_server}/redirect-loop")


def test_timeouts(http_server: str) -> None:
    with pytest.raises(httpx.ReadTimeout):
        httpx.get(f"{http_server}/slow", timeout=0.05)


def test_cookies_and_auth(http_server: str) -> None:
    with httpx.Client(base_url=http_server) as client:
        client.get("/set-cookie")
        assert client.cookies.get("session") == "xyz789"
        r = client.get("/check-cookie")
        assert "session=xyz789" in r.json()["cookie"]

    assert httpx.get(f"{http_server}/basic-auth", auth=("alice", "secret")).status_code == 200
    assert (
        httpx.get(f"{http_server}/bearer-auth", auth=httpx.BearerAuth("my-token")).status_code
        == 200
    )
    assert (
        httpx.get(
            f"{http_server}/digest-auth", auth=httpx.DigestAuth("mufasa", "Circle Of Life")
        ).status_code
        == 200
    )


def test_multipart_upload_and_gzip(http_server: str) -> None:
    r = httpx.post(
        f"{http_server}/upload",
        data={"field": "val"},
        files={"file": ("hello.txt", b"hello-bytes", "text/plain")},
    )
    assert r.status_code == 200
    payload = r.json()
    assert "multipart/form-data; boundary=" in payload["content_type"]
    assert "hello-bytes" in payload["body"]

    gz = httpx.get(f"{http_server}/gzip")
    assert gz.json() == {"compressed": True}


def test_sync_streaming_and_large_body(http_server: str) -> None:
    with httpx.stream("GET", f"{http_server}/stream") as r:
        lines = list(r.iter_lines())
    assert lines == ["chunk-1", "chunk-2", "chunk-3"]

    large = httpx.get(f"{http_server}/large")
    assert len(large.content) == 2 * 1024 * 1024


def test_connection_pooling_reuse(http_server: str) -> None:
    _MockHandler.server_conn_ids.clear()
    with httpx.Client(base_url=http_server) as client:
        for _ in range(5):
            assert client.get("/echo").status_code == 200
    assert len(_MockHandler.server_conn_ids) == 1


def test_proxy_connection(proxy_server: str) -> None:
    SimpleHttpProxyHandler.seen_requests.clear()
    with httpx.Client(proxies=proxy_server) as client:
        r = client.get("http://example.internal/resource")
    assert r.status_code == 200
    assert r.json()["proxied"] is True


def test_status_error_and_network_failures(http_server: str) -> None:
    r = httpx.get(f"{http_server}/status/404")
    assert r.is_client_error
    assert r.is_error
    with pytest.raises(httpx.HTTPStatusError):
        r.raise_for_status()

    with pytest.raises(httpx.UnsupportedProtocol):
        httpx.get("ftp://127.0.0.1/file")

    with pytest.raises(httpx.InvalidURL):
        httpx.get("not-a-valid-url")

    with pytest.raises(httpx.ConnectError):
        httpx.get("http://127.0.0.1:1", timeout=0.5)

    with pytest.raises(httpx.ConnectError):
        httpx.get("http://nonexistent.invalid.localdomain", timeout=1.0)


@pytest.mark.asyncio
async def test_async_client_concurrency_and_streaming(http_server: str) -> None:
    async with httpx.AsyncClient(base_url=http_server) as client:
        responses = await asyncio.gather(*(client.get("/echo") for _ in range(20)))
        assert all(r.status_code == 200 for r in responses)

        async with client.stream("GET", "/stream") as sr:
            chunks = [chunk async for chunk in sr.aiter_bytes()]
        assert b"".join(chunks) == b"chunk-1\nchunk-2\nchunk-3\n"
