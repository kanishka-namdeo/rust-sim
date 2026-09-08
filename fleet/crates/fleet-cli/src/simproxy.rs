//! Minimal localhost HTTP/1.1 client for the per-vehicle sim control plane
//! (ADR-0018: the fleet's fault proxy). The sims sit on 127.0.0.1:8200+i and
//! speak the same `{"ok","data","error"}` envelope as this plane; the proxy
//! POSTs a JSON body and relays the sim's status + envelope verbatim, so the
//! fault catalog has exactly one source of truth (the sim's own serde types)
//! and validation errors surface with the sim's own reasons.
//!
//! Deliberately hand-rolled (no reqwest/hyper client): the repo carries no
//! heavyweight HTTP client dependency, the peer is always localhost, and the
//! request shape is fixed — `Connection: close` makes the response framing
//! trivial (read to EOF). Timeouts are short: the supervisor calls this from
//! its tick path, never from a request path that can afford to hang.

#![forbid(unsafe_code)]

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Total budget for one proxied POST (connect + write + read).
const BUDGET: Duration = Duration::from_secs(2);

/// POST `body` as JSON to `127.0.0.1:port/path`.
///
/// Returns the sim's HTTP status code and its (parsed-when-possible) JSON
/// body. Errors are strings: transport failures, timeouts, malformed
/// responses. `Connection: close` in the request makes read-to-EOF the
/// correct body framing.
pub async fn post_json(
    port: u16,
    path: &str,
    body: &serde_json::Value,
) -> Result<(u16, serde_json::Value), String> {
    let payload = serde_json::to_string(body).map_err(|e| format!("body encode: {e}"))?;
    tokio::time::timeout(BUDGET, async {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| format!("sim control plane :{port} unreachable: {e}"))?;
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("write to :{port}: {e}"))?;
        // No half-close: hyper aborts the response when it sees a client
        // write-EOF mid-request. `Connection: close` asks the server to
        // close after its response instead — read-to-EOF stays the correct
        // body framing (verified against the sim's axum plane in R-1).
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 4096];
        loop {
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|e| format!("read from :{port}: {e}"))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 1 << 20 {
                return Err(format!("response from :{port} exceeds 1 MiB"));
            }
        }
        parse_response(&buf)
    })
    .await
    .map_err(|_| format!("sim control plane :{port} did not answer within 2 s"))?
}

/// Parse a raw HTTP/1.1 response: status line + headers + body split.
/// Kept pure so the framing logic is unit-testable without a server.
fn parse_response(raw: &[u8]) -> Result<(u16, serde_json::Value), String> {
    let text_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "malformed response: no header terminator".to_string())?;
    let head = std::str::from_utf8(&raw[..text_end])
        .map_err(|_| "malformed response: non-UTF-8 headers".to_string())?;
    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| "malformed response: empty".to_string())?;
    // "HTTP/1.1 200 OK" -> code is the second token.
    let code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed status line: {status_line:?}"))?;
    let body = &raw[text_end + 4..];
    let body_json: serde_json::Value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        // Tolerant: a non-JSON body (e.g. an HTML error page) is wrapped,
        // not fatal — the caller relays whatever the sim actually said.
        serde_json::from_slice(body).unwrap_or_else(|_| {
            serde_json::json!({
                "raw": String::from_utf8_lossy(body),
            })
        })
    };
    Ok((code, body_json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_json_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true,\"data\":{\"id\":\"runtime-1\"}}";
        let (code, body) = parse_response(raw).unwrap();
        assert_eq!(code, 200);
        assert_eq!(body["data"]["id"], "runtime-1");
    }

    #[test]
    fn parses_error_status() {
        let raw = b"HTTP/1.1 400 Bad Request\r\n\r\n{\"ok\":false,\"error\":\"unknown fault type\"}";
        let (code, body) = parse_response(raw).unwrap();
        assert_eq!(code, 400);
        assert!(body["error"].as_str().unwrap().contains("fault"));
    }

    #[test]
    fn wraps_non_json_body() {
        let raw = b"HTTP/1.1 502 Bad Gateway\r\n\r\n<html>boom</html>";
        let (code, body) = parse_response(raw).unwrap();
        assert_eq!(code, 502);
        assert_eq!(body["raw"], "<html>boom</html>");
    }

    #[test]
    fn rejects_headerless_garbage() {
        assert!(parse_response(b"not http at all").is_err());
        assert!(parse_response(b"HTTP/1.1\r\n").is_err());
    }

    /// Round-trip against a real listener on an ephemeral port — one that
    /// reads the request by Content-Length and answers, then closes (the
    /// same framing hyper/the sim's axum plane uses). Proves the request
    /// line, headers and read-to-EOF response framing work over TCP.
    #[tokio::test]
    async fn post_json_round_trips_over_tcp() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(&mut sock);
            // hyper-style: read headers, then exactly Content-Length bytes.
            let mut raw = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                reader.read_exact(&mut byte).await.unwrap();
                raw.push(byte[0]);
                if raw.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&raw).to_string();
            let cl: usize = head
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; cl];
            if cl > 0 {
                reader.read_exact(&mut body).await.unwrap();
            }
            let text = format!("{}{}", head, String::from_utf8_lossy(&body));
            let resp = format!(
                "HTTP/1.1 201 Created\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{{\"ok\":true,\"echo\":\"injected\"}}"
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
            text
        });
        let (code, body) =
            post_json(port, "/api/faults", &serde_json::json!({"type": "motor_cut"}))
                .await
                .unwrap();
        assert_eq!(code, 201);
        assert_eq!(body["echo"], "injected");
        let seen = server.await.unwrap();
        assert!(seen.starts_with("POST /api/faults HTTP/1.1\r\n"));
        assert!(seen.contains("Content-Type: application/json\r\n"));
        assert!(seen.contains("Connection: close\r\n"));
        assert!(seen.ends_with(r#"{"type":"motor_cut"}"#));
    }

    /// A closed port surfaces as a transport error string (the 503 path).
    #[tokio::test]
    async fn unreachable_port_is_an_err() {
        use tokio::net::TcpListener;
        // bind then drop -> guaranteed-closed port (no listener squat risk)
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = post_json(port, "/api/faults", &serde_json::json!({}))
            .await
            .err()
            .expect("must fail");
        assert!(err.contains("unreachable"), "got: {err}");
    }
}
