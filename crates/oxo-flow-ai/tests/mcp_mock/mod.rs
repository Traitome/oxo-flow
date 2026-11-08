//! Loopback JSON-RPC-over-HTTP mock MCP server used by the transport tests.
//!
//! Every request is recorded (request line, headers, body) so tests assert
//! on what actually reached the wire — the MCP Streamable HTTP contract
//! (POST, `Accept: application/json, text/event-stream`,
//! `Mcp-Session-Id`, `MCP-Protocol-Version`) is only proven by headers the
//! server observed.

// Each integration-test binary compiles this shared module separately, so
// helpers used only by a sibling test binary look dead here.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Received {
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// Scripted response for one JSON-RPC request.
pub enum Reply {
    /// `200 application/json`, optional `Mcp-Session-Id` header.
    Json {
        body: Value,
        session: Option<&'static str>,
    },
    /// `text/event-stream` body; the mock closes the connection afterwards
    /// so the client's bounded body reader reaches end-of-stream.
    Sse(String),
    /// Arbitrary status/content-type/body (notifications, HTTP errors).
    Raw {
        status: u16,
        content_type: &'static str,
        body: String,
    },
    /// `302 Found` pointing at `location`.
    Redirect(String),
}

pub struct MockMcp {
    pub url: String,
    requests: Arc<Mutex<Vec<Received>>>,
}

impl MockMcp {
    /// Serve on a fresh loopback port. `respond` sees each parsed request.
    pub fn spawn(respond: impl Fn(&Received) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock MCP server");
        let addr = listener.local_addr().expect("mock MCP addr");
        let requests: Arc<Mutex<Vec<Received>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let respond: Arc<dyn Fn(&Received) -> Reply + Send + Sync> = Arc::new(respond);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // One thread per connection: reqwest keeps idle connections
                // pooled, and a serial accept loop would block new clients
                // behind a lingering keep-alive read.
                let recorded = Arc::clone(&recorded);
                let respond = Arc::clone(&respond);
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
                    while let Some(request) = read_request(&mut stream) {
                        let reply = respond(&request);
                        let (bytes, close) = encode(reply);
                        recorded.lock().unwrap().push(request);
                        if stream.write_all(&bytes).is_err() {
                            break;
                        }
                        if close {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            url: format!("mcp://{addr}"),
            requests,
        }
    }

    pub fn requests(&self) -> Vec<Received> {
        self.requests.lock().unwrap().clone()
    }

    pub fn requests_from(&self, index: usize) -> Vec<Received> {
        self.requests.lock().unwrap()[index..].to_vec()
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

fn encode(reply: Reply) -> (Vec<u8>, bool) {
    match reply {
        Reply::Json { body, session } => {
            let body = body.to_string();
            let session = session
                .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
                .unwrap_or_default();
            (
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{session}Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .into_bytes(),
                false,
            )
        }
        Reply::Sse(body) => (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
            true,
        ),
        Reply::Raw {
            status,
            content_type,
            body,
        } => (
            format!(
                "HTTP/1.1 {status} Status\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
            false,
        ),
        Reply::Redirect(location) => (
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
            false,
        ),
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Received> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = find_subslice(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.lines();
    let method = lines.next()?.split_whitespace().next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    let content_length = headers
        .iter()
        .find(|(key, _)| key == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length.min(body.len()));
    Some(Received {
        method,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
