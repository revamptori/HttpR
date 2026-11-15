//! Rust integration tests exercising the core HTTP engine against local Tokio TCP servers.

use _httpx_rs::{
    AsyncClient, BasicAuth, BearerAuth, Client, ClientBuilder, DigestAuth, HttpxError,
    PoolLimits, RetryConfig, TimeoutConfig,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn spawn_http_server<F, Fut>(handler: F) -> String
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Vec<u8>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = std::sync::Arc::new(handler);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let h = std::sync::Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    let n = match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let req_str = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let resp = h(req_str).await;
                    if socket.write_all(&resp).await.is_err() {
                        break;
                    }
                    let _ = socket.flush().await;
                }
            });
        }
    });

    format!("http://127.0.0.1:{}", addr.port())
}

#[tokio::test]
async fn test_async_get_json_and_headers() {
    let base = spawn_http_server(|req| async move {
        assert!(req.starts_with("GET /hello?foo=bar HTTP/1.1"));
        let body = r#"{"greeting":"world"}"#;
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .into_bytes()
    })
    .await;

    let client = AsyncClient::new().unwrap();
    let req = client
        .build_request("GET", &format!("{base}/hello"), None)
        .unwrap()
        .param("foo", "bar")
        .build()
        .unwrap();

    let resp = client.send(req, Default::default()).await.unwrap();
    assert_eq!(resp.status_code, 200);
    assert!(resp.is_success());
    let json: serde_json::Value = resp.json().unwrap();
    assert_eq!(json["greeting"], "world");
}

#[tokio::test]
async fn test_redirects_and_cookie_persistence() {
    let base = spawn_http_server(|req| async move {
        if req.starts_with("GET /start ") {
            b"HTTP/1.1 302 Found\r\nLocation: /final\r\nSet-Cookie: sid=abc123; Path=/\r\nContent-Length: 0\r\n\r\n".to_vec()
        } else {
            let has_cookie = req.contains("sid=abc123");
            let body = if has_cookie { "ok" } else { "missing" };
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .into_bytes()
        }
    })
    .await;

    let client = ClientBuilder::new()
        .follow_redirects(true)
        .build_async()
        .unwrap();
    let resp = client.get(&format!("{base}/start")).await.unwrap();
    assert_eq!(resp.status_code, 200);
    assert_eq!(resp.text().unwrap(), "ok");
    assert_eq!(resp.history.len(), 1);
    assert_eq!(client.cookies().get("sid", None, None).as_deref(), Some("abc123"));
}

#[tokio::test]
async fn test_too_many_redirects() {
    let base = spawn_http_server(|_| async move {
        b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n".to_vec()
    })
    .await;

    let client = ClientBuilder::new()
        .follow_redirects(true)
        .max_redirects(3)
        .build_async()
        .unwrap();
    let err = client.get(&format!("{base}/loop")).await.unwrap_err();
    assert!(matches!(err, HttpxError::TooManyRedirects { max: 3, .. }));
}

#[tokio::test]
async fn test_read_timeout() {
    let base = spawn_http_server(|_| async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
    })
    .await;

    let client = ClientBuilder::new()
        .timeout(TimeoutConfig::uniform(Some(Duration::from_millis(50))))
        .build_async()
        .unwrap();

    let err = client.get(&format!("{base}/slow")).await.unwrap_err();
    assert!(matches!(err, HttpxError::ReadTimeout(_)));
}

#[tokio::test]
async fn test_connection_pool_reuse() {
    let base = spawn_http_server(|_| async move {
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
    })
    .await;

    let client = ClientBuilder::new()
        .limits(PoolLimits {
            max_connections: Some(10),
            max_keepalive_connections: Some(5),
            keepalive_expiry: Some(Duration::from_secs(10)),
        })
        .build_async()
        .unwrap();

    for _ in 0..5 {
        let resp = client.get(&format!("{base}/ping")).await.unwrap();
        assert_eq!(resp.status_code, 200);
    }
    assert_eq!(client.transport().pool().idle_http1_count(), 1);
}

#[tokio::test]
async fn test_auth_methods_basic_bearer_digest() {
    let basic = BasicAuth::new("u", "p");
    assert_eq!(basic.header_value(), "Basic dTpw");

    let bearer = BearerAuth::new("tok-123");
    assert_eq!(bearer.header_value(), "Bearer tok-123");

    let base = spawn_http_server(|req| async move {
        if req.contains("Authorization: Digest ") {
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nauth".to_vec()
        } else {
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"test\", qop=\"auth\", nonce=\"12345\"\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    })
    .await;

    let client = ClientBuilder::new()
        .auth(_httpx_rs::AuthMethod::Digest(DigestAuth::new("alice", "secret")))
        .build_async()
        .unwrap();
    let resp = client.get(&format!("{base}/protected")).await.unwrap();
    assert_eq!(resp.status_code, 200);
    assert_eq!(resp.text().unwrap(), "auth");
}

#[tokio::test]
async fn test_concurrent_async_requests_and_large_body() {
    let payload = "A".repeat(512 * 1024);
    let payload_clone = payload.clone();
    let base = spawn_http_server(move |_| {
        let p = payload_clone.clone();
        async move {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                p.len(),
                p
            )
            .into_bytes()
        }
    })
    .await;

    let client = AsyncClient::new().unwrap();
    let mut handles = Vec::new();
    for _ in 0..10 {
        let c = client.clone();
        let url = format!("{base}/large");
        handles.push(tokio::spawn(async move { c.get(&url).await }));
    }
    for h in handles {
        let resp = h.await.unwrap().unwrap();
        assert_eq!(resp.content().unwrap().len(), 512 * 1024);
    }
}

#[test]
fn test_blocking_client_with_retries_and_connect_error() {
    let client = ClientBuilder::new()
        .retries(RetryConfig::new(1))
        .timeout(TimeoutConfig::uniform(Some(Duration::from_millis(200))))
        .build_blocking(_httpx_rs::shared_runtime().unwrap())
        .unwrap();

    // Closed local port should yield ConnectError
    let err = client.get("http://127.0.0.1:1").unwrap_err();
    assert!(matches!(err, HttpxError::ConnectError(_)));
    let _ = Client::new().unwrap();
}
