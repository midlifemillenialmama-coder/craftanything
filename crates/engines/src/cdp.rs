//! A small, bounded Chrome DevTools Protocol client.
//!
//! Everything here talks plain, unencrypted HTTP and WebSocket to a *loopback* address only
//! (`127.0.0.1` or `[::1]`), either an endpoint the user started (Chrome launched with
//! `--remote-debugging-port`) or one we launched ourselves through
//! [`craft_anything_platform::process`]. There is no TLS because `ws:` to loopback never needs it.
//!
//! The WebSocket framing is hand-rolled over [`std::net::TcpStream`] so that every read and write
//! is bounded by a wall-clock deadline, cooperatively cancellable, and capped in size. This mirrors
//! the loopback HTTP client used for the JEB bridge. We never follow redirects, never keep more than
//! a fixed number of bytes, and poison the connection on any malformed frame.

use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::cancel::CancelToken;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Largest HTTP discovery response we accept (`/json/version`, `/json/list`).
const MAX_HTTP_RESPONSE: usize = 8 << 20;
/// Default WebSocket message cap for a page connection (bytes).
pub const PAGE_MAX_MESSAGE: usize = 64 << 20;
/// Default WebSocket message cap for the V8 inspector (bytes).
pub const INSPECTOR_MAX_MESSAGE: usize = 16 << 20;
/// Hard upper bound on any single frame length we will even attempt to buffer.
const FRAME_HARD_CAP: u64 = 0x7fff_ffff;
/// How long a single poll on the socket blocks before we re-check the deadline and cancel flag.
const POLL: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------------------------
// Endpoint

/// A validated loopback CDP endpoint, e.g. `http://127.0.0.1:9222`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The canonical `http://host:port` with no trailing slash.
    pub origin: String,
    host: String,
    port: u16,
    addr: SocketAddr,
}

impl Endpoint {
    pub fn port(&self) -> u16 {
        self.port
    }
}

fn endpoint_err(msg: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, msg).with_remediation(
        "Give a loopback endpoint with an explicit port, e.g. http://127.0.0.1:9222, after starting the browser with --remote-debugging-port.",
    )
}

/// Parse a CDP endpoint. Only literal loopback hosts with an explicit port are accepted; no
/// `localhost`, no LAN address, no https, no credentials, no path, query or fragment.
pub fn parse_endpoint(url: &str) -> Result<Endpoint, ToolError> {
    let u = url.trim();
    if u.is_empty() {
        return Err(endpoint_err("the CDP endpoint is empty"));
    }
    if u.contains(['?', '#', '@']) || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(endpoint_err(format!("the CDP endpoint {u:?} must not contain credentials, a query, a fragment or spaces")));
    }
    let Some(rest) = u.strip_prefix("http://") else {
        return Err(endpoint_err(format!("the CDP endpoint {u:?} must start with http:// (plain, loopback)")));
    };
    // Reject any path beyond a single trailing slash.
    let authority = match rest.split_once('/') {
        Some((a, "")) => a,
        Some(_) => return Err(endpoint_err(format!("the CDP endpoint {u:?} must not contain a path"))),
        None => rest,
    };
    let (host_raw, port_raw) = split_host_port(authority)?;
    let port: u16 = port_raw
        .ok_or_else(|| endpoint_err(format!("the CDP endpoint {u:?} needs an explicit port")))?
        .parse()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| endpoint_err(format!("the CDP endpoint {u:?} has an invalid port")))?;
    let addr = loopback_addr(&host_raw, port)?;
    Ok(Endpoint { origin: format!("http://{host_raw}:{port}"), host: host_raw, port, addr })
}

/// Split `host:port` honouring `[ipv6]:port`. Returns (host, optional port text).
fn split_host_port(authority: &str) -> Result<(String, Option<&str>), ToolError> {
    if let Some(after) = authority.strip_prefix('[') {
        let (h, tail) = after.split_once(']').ok_or_else(|| endpoint_err("unclosed [ in the host"))?;
        let port = match tail {
            "" => None,
            t => Some(t.strip_prefix(':').ok_or_else(|| endpoint_err("junk after ] in the host"))?),
        };
        Ok((format!("[{h}]"), port))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => Ok((h.to_string(), Some(p))),
            None => Ok((authority.to_string(), None)),
        }
    }
}

/// Resolve a host string to its single loopback socket address, rejecting anything else.
fn loopback_addr(host: &str, port: u16) -> Result<SocketAddr, ToolError> {
    match host.to_ascii_lowercase().as_str() {
        "127.0.0.1" => Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
        "[::1]" => Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)),
        other => Err(endpoint_err(format!("host {other:?} is not loopback; only 127.0.0.1 or [::1] are allowed (not even localhost)"))),
    }
}

// ---------------------------------------------------------------------------------------------
// Deadline

/// A shared wall-clock deadline and cancel flag used by every socket operation.
#[derive(Clone)]
pub struct Deadline {
    at: Instant,
    cancel: CancelToken,
}

impl Deadline {
    pub fn new(timeout: Duration, cancel: CancelToken) -> Self {
        Deadline { at: Instant::now().checked_add(timeout).unwrap_or_else(Instant::now), cancel }
    }

    /// Remaining time, or an error if the deadline passed or the caller cancelled.
    fn remaining(&self) -> Result<Duration, ToolError> {
        self.cancel.check()?;
        let r = self.at.saturating_duration_since(Instant::now());
        if r.is_zero() { Err(timeout_err()) } else { Ok(r) }
    }

    pub fn expired(&self) -> bool {
        self.cancel.is_cancelled() || self.at.saturating_duration_since(Instant::now()).is_zero()
    }
}

fn timeout_err() -> ToolError {
    ToolError::new(ErrorCode::TimedOut, "the browser did not answer in time.").with_remediation("Raise the timeout, or check the browser is responsive.")
}

fn unavailable(ep: &Endpoint, detail: impl std::fmt::Display) -> ToolError {
    ToolError::new(ErrorCode::EngineMissing, format!("couldn't reach the browser at {} ({detail}).", ep.origin))
        .with_remediation("Start the browser with --remote-debugging-port, or launch one from Settings → Engines, and check the endpoint.")
}

// ---------------------------------------------------------------------------------------------
// HTTP discovery (GET only, loopback only, bounded)

fn connect_tcp(ep: &Endpoint, dl: &Deadline) -> Result<TcpStream, ToolError> {
    let t = dl.remaining()?.min(Duration::from_secs(10));
    match TcpStream::connect_timeout(&ep.addr, t) {
        Ok(s) => Ok(s),
        Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Err(timeout_err()),
        Err(e) => Err(unavailable(ep, e)),
    }
}

/// A bounded HTTP/1.1 GET against the loopback endpoint. Returns (status, body).
pub fn http_get(ep: &Endpoint, path: &str, dl: &Deadline) -> Result<(u16, Vec<u8>), ToolError> {
    let mut stream = connect_tcp(ep, dl)?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: CraftAnything-dev\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        host_only(&ep.host),
        ep.port
    );
    stream.set_write_timeout(Some(dl.remaining()?)).map_err(|e| ToolError::internal(e.to_string()))?;
    stream.write_all(req.as_bytes()).and_then(|_| stream.flush()).map_err(|e| unavailable(ep, e))?;
    read_http_response(&mut stream, ep, dl)
}

fn host_only(host: &str) -> &str {
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).map(|_| host).unwrap_or(host)
}

fn read_http_response(stream: &mut TcpStream, ep: &Endpoint, dl: &Deadline) -> Result<(u16, Vec<u8>), ToolError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 16 * 1024];
    let mut header_end: Option<usize> = None;
    let mut status = 0u16;
    let mut content_len: Option<usize> = None;
    loop {
        if let Some(end) = header_end {
            let body = buf.get(end..).unwrap_or_default();
            if content_len.is_some_and(|n| body.len() >= n) {
                let n = content_len.unwrap_or(0);
                return Ok((status, body.get(..n).unwrap_or(body).to_vec()));
            }
        }
        let wait = dl.remaining()?.min(POLL);
        stream.set_read_timeout(Some(wait)).map_err(|e| ToolError::internal(e.to_string()))?;
        match stream.read(&mut tmp) {
            Ok(0) => {
                let end = header_end.ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "the browser closed the HTTP connection without answering"))?;
                let body = buf.get(end..).unwrap_or_default();
                let body = content_len.map(|n| body.get(..n).unwrap_or(body)).unwrap_or(body);
                return Ok((status, body.to_vec()));
            }
            Ok(n) => {
                buf.extend_from_slice(tmp.get(..n).unwrap_or_default());
                if buf.len() > MAX_HTTP_RESPONSE {
                    return Err(ToolError::new(ErrorCode::OverLimit, "the browser's HTTP answer is too large"));
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
            Err(e) => return Err(unavailable(ep, e)),
        }
        if header_end.is_none()
            && let Some(pos) = find(&buf, b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(buf.get(..pos).unwrap_or_default()).into_owned();
            let mut lines = head.split("\r\n");
            status = lines
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "malformed HTTP status line from the browser"))?;
            for l in lines {
                if let Some((k, v)) = l.split_once(':')
                    && k.trim().eq_ignore_ascii_case("content-length")
                {
                    content_len = v.trim().parse::<usize>().ok();
                }
            }
            if content_len.is_some_and(|n| n > MAX_HTTP_RESPONSE) {
                return Err(ToolError::new(ErrorCode::OverLimit, "the browser's HTTP answer is too large"));
            }
            header_end = Some(pos + 4);
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn get_json(ep: &Endpoint, path: &str, dl: &Deadline) -> Result<Value, ToolError> {
    let (status, body) = http_get(ep, path, dl)?;
    if !(200..300).contains(&status) {
        return Err(ToolError::new(ErrorCode::EngineMissing, format!("the browser answered HTTP {status} for {path}"))
            .with_remediation("Check this is a Chromium DevTools endpoint (it serves /json/version)."));
    }
    serde_json::from_slice(&body).map_err(|e| {
        ToolError::new(ErrorCode::GarbledOutput, format!("the browser's {path} answer was not JSON: {e}"))
            .with_remediation("Check the endpoint is a Chromium remote-debugging port.")
    })
}

/// `GET /json/version`: browser product, protocol version and the browser-level WebSocket URL.
pub fn json_version(ep: &Endpoint, dl: &Deadline) -> Result<Value, ToolError> {
    get_json(ep, "/json/version", dl)
}

/// `GET /json/list`: the debuggable targets.
pub fn json_list(ep: &Endpoint, dl: &Deadline) -> Result<Vec<Value>, ToolError> {
    match get_json(ep, "/json/list", dl)? {
        Value::Array(a) => Ok(a),
        _ => Err(ToolError::new(ErrorCode::GarbledOutput, "the browser's /json/list answer was not an array")),
    }
}

// ---------------------------------------------------------------------------------------------
// WebSocket URL

/// A validated `ws://` URL on the same loopback host/port as `ep`.
pub struct WsTarget {
    addr: SocketAddr,
    host_header: String,
    path: String,
}

/// Validate a `ws://` debugger URL and confirm it stays on the given loopback endpoint's port.
pub fn ws_target(ws_url: &str, ep: &Endpoint) -> Result<WsTarget, ToolError> {
    let u = ws_url.trim();
    if u.contains(['@', ' ']) || u.chars().any(char::is_control) {
        return Err(endpoint_err(format!("the WebSocket URL {u:?} is malformed")));
    }
    let Some(rest) = u.strip_prefix("ws://") else {
        return Err(endpoint_err(format!("the WebSocket URL {u:?} must start with ws://")));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
        None => (rest, "/"),
    };
    let (host, port_raw) = split_host_port(authority)?;
    let port: u16 = match port_raw {
        Some(p) => p.parse().ok().filter(|p| *p != 0).ok_or_else(|| endpoint_err("the WebSocket URL has an invalid port"))?,
        None => return Err(endpoint_err("the WebSocket URL needs an explicit port")),
    };
    let addr = loopback_addr(&host, port)?;
    if port != ep.port() {
        return Err(endpoint_err(format!("the WebSocket URL is on port {port}, not the endpoint's port {}", ep.port())));
    }
    Ok(WsTarget { addr, host_header: format!("{}:{port}", host_only(&host)), path: path.to_string() })
}

// ---------------------------------------------------------------------------------------------
// Pseudo-random bytes for masking and the handshake key (not security-sensitive).

fn fill_random(out: &mut [u8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut state = nanos ^ (std::process::id() as u64).rotate_left(21) ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed) ^ (out.as_ptr() as u64);
    for b in out.iter_mut() {
        // SplitMix64 step.
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        *b = (z ^ (z >> 31)) as u8;
    }
}

// ---------------------------------------------------------------------------------------------
// WebSocket connection (hand-rolled framing)

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// HTTP upgrade result: status, response headers, and any bytes read past the header block.
type Handshake = (u16, Vec<(String, String)>, Vec<u8>);

/// One open WebSocket to a CDP target. Not `Clone`; owns its socket.
pub struct WsConn {
    stream: TcpStream,
    buf: Vec<u8>,
    max_message: usize,
    closed: bool,
}

impl WsConn {
    /// Open a WebSocket to `target`, performing the HTTP upgrade handshake, bounded by `dl`.
    pub fn connect(target: &WsTarget, max_message: usize, dl: &Deadline) -> Result<WsConn, ToolError> {
        let ep = Endpoint { origin: String::new(), host: target.host_header.clone(), port: target.addr.port(), addr: target.addr };
        let mut stream = connect_tcp(&ep, dl)?;
        let mut key_bytes = [0u8; 16];
        fill_random(&mut key_bytes);
        let key = base64_encode(&key_bytes);
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: CraftAnything-dev\r\n\r\n",
            target.path, target.host_header
        );
        stream.set_write_timeout(Some(dl.remaining()?)).map_err(|e| ToolError::internal(e.to_string()))?;
        stream
            .write_all(req.as_bytes())
            .and_then(|_| stream.flush())
            .map_err(|e| ToolError::new(ErrorCode::EngineMissing, format!("WebSocket upgrade failed: {e}")))?;
        let (status, headers, leftover) = Self::read_handshake(&mut stream, dl)?;
        if status != 101 {
            return Err(ToolError::new(ErrorCode::EngineMissing, format!("the browser refused the WebSocket upgrade (HTTP {status})"))
                .with_remediation("If this browser was launched elsewhere, start it with --remote-allow-origins so DevTools clients may connect."));
        }
        let accept = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-accept")).map(|(_, v)| v.as_str()).unwrap_or("");
        if accept != expected_accept(&key) {
            return Err(ToolError::new(ErrorCode::GarbledOutput, "the WebSocket handshake accept token did not match"));
        }
        Ok(WsConn { stream, buf: leftover, max_message, closed: false })
    }

    fn read_handshake(stream: &mut TcpStream, dl: &Deadline) -> Result<Handshake, ToolError> {
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            if let Some(pos) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(buf.get(..pos).unwrap_or_default()).into_owned();
                let mut lines = head.split("\r\n");
                let status = lines
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "malformed WebSocket handshake response"))?;
                let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).take(100).collect();
                let leftover = buf.get(pos + 4..).unwrap_or_default().to_vec();
                return Ok((status, headers, leftover));
            }
            if buf.len() > 64 * 1024 {
                return Err(ToolError::new(ErrorCode::GarbledOutput, "the WebSocket handshake headers were too large"));
            }
            let wait = dl.remaining()?.min(POLL);
            stream.set_read_timeout(Some(wait)).map_err(|e| ToolError::internal(e.to_string()))?;
            match stream.read(&mut tmp) {
                Ok(0) => return Err(ToolError::new(ErrorCode::EngineMissing, "the browser closed the connection during the WebSocket handshake")),
                Ok(n) => buf.extend_from_slice(tmp.get(..n).unwrap_or_default()),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
                Err(e) => return Err(ToolError::new(ErrorCode::EngineMissing, format!("WebSocket handshake read failed: {e}"))),
            }
        }
    }

    /// A socket option refused on the open connection: macOS answers `setsockopt` with "Invalid
    /// argument" once the browser has closed the socket both ways, so this is the connection
    /// lost, not a fault of ours.
    fn lost(&mut self, doing: &str, e: &std::io::Error) -> ToolError {
        self.closed = true;
        ToolError::new(ErrorCode::EngineMissing, format!("{doing} the browser failed: {e}"))
    }

    /// Send one JSON text message.
    pub fn send_text(&mut self, text: &str, dl: &Deadline) -> Result<(), ToolError> {
        if self.closed {
            return Err(ToolError::new(ErrorCode::EngineMissing, "the WebSocket is closed"));
        }
        self.write_frame(OP_TEXT, text.as_bytes(), dl)
    }

    fn write_frame(&mut self, opcode: u8, payload: &[u8], dl: &Deadline) -> Result<(), ToolError> {
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | (opcode & 0x0f)); // FIN + opcode
        let len = payload.len();
        let mask_bit = 0x80u8;
        if len < 126 {
            frame.push(mask_bit | len as u8);
        } else if len <= u16::MAX as usize {
            frame.push(mask_bit | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(mask_bit | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mut mask = [0u8; 4];
        fill_random(&mut mask);
        frame.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i & 3]);
        }
        let wait = dl.remaining()?;
        self.stream.set_write_timeout(Some(wait)).map_err(|e| self.lost("writing to", &e))?;
        self.stream.write_all(&frame).and_then(|_| self.stream.flush()).map_err(|e| {
            self.closed = true;
            ToolError::new(ErrorCode::EngineMissing, format!("writing to the browser failed: {e}"))
        })
    }

    /// Read the next complete application message (text or binary), auto-answering pings and
    /// handling fragmentation. Returns `None` on a clean close.
    pub fn read_message(&mut self, dl: &Deadline) -> Result<Option<Vec<u8>>, ToolError> {
        let mut assembled: Vec<u8> = Vec::new();
        let mut assembling = false;
        loop {
            let frame = match self.read_frame(dl)? {
                Some(f) => f,
                None => return Ok(None),
            };
            match frame.opcode {
                OP_PING => {
                    self.write_frame(OP_PONG, &frame.payload, dl)?;
                }
                OP_PONG => {}
                OP_CLOSE => {
                    self.closed = true;
                    let _ = self.write_frame(OP_CLOSE, &[], dl);
                    return Ok(None);
                }
                OP_TEXT | OP_BINARY if !assembling => {
                    if frame.fin {
                        return Ok(Some(frame.payload));
                    }
                    assembled = frame.payload;
                    assembling = true;
                }
                OP_CONTINUATION if assembling => {
                    if assembled.len().saturating_add(frame.payload.len()) > self.max_message {
                        self.closed = true;
                        return Err(message_too_big(self.max_message));
                    }
                    assembled.extend_from_slice(&frame.payload);
                    if frame.fin {
                        return Ok(Some(assembled));
                    }
                }
                _ => {
                    self.closed = true;
                    return Err(ToolError::new(ErrorCode::GarbledOutput, "the browser sent an out-of-order WebSocket frame"));
                }
            }
        }
    }

    /// Read exactly one frame (any opcode), buffering from the socket as needed.
    fn read_frame(&mut self, dl: &Deadline) -> Result<Option<Frame>, ToolError> {
        loop {
            if let Some(frame) = self.try_parse_frame()? {
                return Ok(Some(frame));
            }
            let wait = dl.remaining()?.min(POLL);
            let mut tmp = [0u8; 32 * 1024];
            self.stream.set_read_timeout(Some(wait)).map_err(|e| self.lost("reading from", &e))?;
            match self.stream.read(&mut tmp) {
                Ok(0) => {
                    self.closed = true;
                    return Ok(None);
                }
                Ok(n) => {
                    self.buf.extend_from_slice(tmp.get(..n).unwrap_or_default());
                    if self.buf.len() as u64 > FRAME_HARD_CAP {
                        self.closed = true;
                        return Err(message_too_big(self.max_message));
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
                Err(e) => {
                    self.closed = true;
                    return Err(ToolError::new(ErrorCode::EngineMissing, format!("reading from the browser failed: {e}")));
                }
            }
        }
    }

    /// Try to parse one frame from the buffer. `Ok(None)` means "need more bytes".
    fn try_parse_frame(&mut self) -> Result<Option<Frame>, ToolError> {
        let b0 = match self.buf.first() {
            Some(b) => *b,
            None => return Ok(None),
        };
        let b1 = match self.buf.get(1) {
            Some(b) => *b,
            None => return Ok(None),
        };
        let fin = b0 & 0x80 != 0;
        let opcode = b0 & 0x0f;
        let masked = b1 & 0x80 != 0;
        if masked {
            self.closed = true;
            return Err(ToolError::new(ErrorCode::GarbledOutput, "the browser sent a masked frame (servers must not mask)"));
        }
        let short_len = (b1 & 0x7f) as usize;
        let (len, mut offset): (u64, usize) = match short_len {
            126 => match self.buf.get(2..4) {
                Some(b) => (u16::from_be_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]) as u64, 4),
                None => return Ok(None),
            },
            127 => match self.buf.get(2..10) {
                Some(b) => {
                    let mut arr = [0u8; 8];
                    arr.copy_from_slice(b);
                    (u64::from_be_bytes(arr), 10)
                }
                None => return Ok(None),
            },
            n => (n as u64, 2),
        };
        if len > FRAME_HARD_CAP || len as usize > self.max_message {
            self.closed = true;
            return Err(message_too_big(self.max_message));
        }
        let total = offset.saturating_add(len as usize);
        if self.buf.len() < total {
            return Ok(None);
        }
        let payload = self.buf.get(offset..total).unwrap_or_default().to_vec();
        offset = total;
        self.buf.drain(..offset);
        Ok(Some(Frame { fin, opcode, payload }))
    }

    /// Send a courteous close. Errors are ignored; the socket drops regardless.
    pub fn close(&mut self, dl: &Deadline) {
        if !self.closed {
            let _ = self.write_frame(OP_CLOSE, &[], dl);
            self.closed = true;
        }
    }
}

fn message_too_big(max: usize) -> ToolError {
    ToolError::new(ErrorCode::OverLimit, format!("a WebSocket message exceeded the {max}-byte limit"))
        .with_remediation("Narrow what is captured, or raise the message limit.")
}

// ---------------------------------------------------------------------------------------------
// JSON-RPC over a WsConn

/// A CDP command/event channel over one WebSocket. Buffers events seen while awaiting a reply.
pub struct CdpClient {
    conn: WsConn,
    next_id: u64,
    events: VecDeque<Value>,
    max_events: usize,
    dropped_events: u64,
    dialogs: Dialogs,
    /// The `confirm` or `prompt` the page is waiting on, held for the caller ([`Dialogs::Notices`]).
    waiting: Option<Value>,
    /// How many questions have been held, so a call can tell a new one from the one it answers.
    questions: u64,
    /// An answer that was sent but whose reply hadn't come when its call ended ([`Unsettled`]).
    unsettled: Option<Unsettled>,
    /// The frames that started loading and haven't stopped (or gone), by id, as events arrive:
    /// kept with the connection, so a wait can resume after a question. Bounded ([`MAX_LOADING_FRAMES`]).
    loading_frames: std::collections::HashSet<String>,
    /// A frame start was dropped at the cap: whether everything stopped can't be known, so the
    /// page counts as loading and a wait runs to its own bound.
    loading_unknown: bool,
}

/// Most frames a client tracks as loading; a page can start any number of them.
const MAX_LOADING_FRAMES: usize = 256;

/// An answer sent without a word back yet. The browser applies an answer to whatever dialog is
/// showing, with no id for it, so another answer isn't sent until this one's outcome is read: it
/// may have been taken, and the page may already be asking something else.
#[derive(Debug)]
struct Unsettled {
    /// The request id its reply will carry.
    id: u64,
    /// [`CdpClient::questions`] when it was sent: a higher count means the page went on.
    asked_before: u64,
    /// Its reply once seen: taken, or the browser's refusal.
    reply: Option<Result<(), ToolError>>,
    /// Its `accept`, so what the page took can be said if it was taken.
    accept: Option<bool>,
}

/// Which of a page's own dialogs a client answers by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialogs {
    /// None: a person (or nobody) answers them.
    Left,
    /// `alert` and leave-page questions; a `confirm` or `prompt` is held for the caller.
    Notices,
    /// Every dialog, by [`dialog_answer`].
    All,
}

/// The event that says a page's dialog went away (answered by us, by a person or by the page).
pub const DIALOG_CLOSED: &str = "Page.javascriptDialogClosed";

/// The key in a [`ToolError`]'s details that carries the dialog a call stopped at: the
/// [`DIALOG_OPENING`] params (`type`, `message`, `defaultPrompt`, `url`).
pub const DIALOG_DETAIL: &str = "dialog_open";

/// Whether a dialog only tells the visitor something, so its one answer can be given without
/// asking anyone: an `alert` (OK) or a leave-page question (going on with the navigation).
pub fn is_notice(opening: &Value) -> bool {
    matches!(opening.get("type").and_then(Value::as_str), Some("alert" | "beforeunload"))
}

/// The event a page's `alert`, `confirm`, `prompt` or leave-page question raises. Until it's
/// answered the page's scripts stop, no load event comes and every evaluation waits.
pub const DIALOG_OPENING: &str = "Page.javascriptDialogOpening";

/// The `Page.handleJavaScriptDialog` answer for a dialog a page opened by itself: an `alert` is
/// closed and a leave-page question lets the page go (what a visitor's only click would do), while
/// a `confirm` or `prompt` is dismissed, so nothing the page asks to do is agreed to on our behalf.
pub fn dialog_answer(opening: &Value) -> Value {
    json!({"accept": is_notice(opening)})
}

/// What became of [`CdpClient::answer_dialog`].
#[derive(Debug)]
pub enum DialogAnswer {
    /// The page took the answer and went on.
    Answered,
    /// An earlier answer, sent but not confirmed when its call ended, turned out to be taken; this
    /// one wasn't sent, so it can't land on a question asked since. `accept` is the one the page took.
    AnsweredBefore { accept: Option<bool> },
    /// The browser refused: no dialog is showing (the page closed it itself). Its error is kept.
    NotShowing(ToolError),
}

/// A CDP event: domain-qualified method and its params.
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

impl CdpClient {
    pub fn new(conn: WsConn) -> Self {
        CdpClient {
            conn,
            next_id: 1,
            events: VecDeque::new(),
            max_events: 200_000,
            dropped_events: 0,
            dialogs: Dialogs::Left,
            waiting: None,
            questions: 0,
            unsettled: None,
            loading_frames: std::collections::HashSet::new(),
            loading_unknown: false,
        }
    }

    /// Answer every dialog the page opens by itself ([`dialog_answer`]) as soon as it's seen, so a
    /// page that greets new visitors with an `alert` loads instead of holding every call until its
    /// deadline. The [`DIALOG_OPENING`] events stay in the queue for the caller to report.
    /// Needs `Page.enable` on the session; leave it off where a person answers the page.
    pub fn answering_dialogs(mut self) -> Self {
        self.dialogs = Dialogs::All;
        self
    }

    /// Answer only the dialogs that have one answer ([`is_notice`]); hold a `confirm` or `prompt`
    /// for the caller to decide. While one is open the page's scripts wait, so a call that is
    /// waiting for its reply when it opens ends at once with an error carrying it in its details
    /// ([`DIALOG_DETAIL`]), and [`Self::waiting_dialog`] says what it asks until it closes.
    pub fn answering_notices(mut self) -> Self {
        self.dialogs = Dialogs::Notices;
        self
    }

    /// The `confirm` or `prompt` this page is waiting on ([`Self::answering_notices`]): its
    /// [`DIALOG_OPENING`] params, until a [`DIALOG_CLOSED`] event is seen.
    pub fn waiting_dialog(&self) -> Option<&Value> {
        self.waiting.as_ref()
    }

    /// Whether this client dismisses a `confirm` or `prompt` itself ([`Self::answering_dialogs`]).
    pub fn dismisses_questions(&self) -> bool {
        self.dialogs == Dialogs::All
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped_events
    }

    /// From now on, dismiss every dialog the page opens (alerts and leave-page questions still
    /// accepted, [`dialog_answer`]), for leaving a page: a question asked in reply to the one being
    /// dismissed is answered too, rather than left with nothing to answer it.
    pub fn dismiss_from_now(&mut self) {
        self.dialogs = Dialogs::All;
    }

    /// Whether a frame of the page is still loading, from the frame events seen so far.
    /// Past [`MAX_LOADING_FRAMES`] it stays true for the connection, so a wait ends at its bound.
    pub fn frames_loading(&self) -> bool {
        self.loading_unknown || !self.loading_frames.is_empty()
    }

    /// Drop the queued openings of questions (`confirm`, `prompt`), keeping every other event in
    /// order. Before an answer, the only one queued is the question being answered, already
    /// reported; read again afterwards it would look like something the page asked after.
    pub fn forget_queued_questions(&mut self) {
        self.events.retain(|v| !(v.get("method").and_then(Value::as_str) == Some(DIALOG_OPENING) && !is_notice(&v["params"])));
    }

    /// Whether an answer was sent and its outcome isn't known yet: the next
    /// [`Self::answer_dialog`] reads it instead of sending another.
    pub fn answer_unconfirmed(&self) -> bool {
        self.unsettled.is_some()
    }

    /// Whether the connection can still carry commands: false once the browser closed it or a
    /// write or read on it failed. A call that only ran out of time leaves it open.
    pub fn connected(&self) -> bool {
        !self.conn.closed
    }

    /// Answer the question the page is waiting on with `Page.handleJavaScriptDialog` params
    /// (`accept`, and `promptText` for a prompt). The browser's refusal (it says no dialog is
    /// showing) is [`DialogAnswer::NotShowing`]; an error is a call that couldn't tell, such as
    /// one out of time or on a lost connection, and [`Self::waiting_dialog`] still says what's held.
    /// A new question the page asks before the reply means this one was answered. While an
    /// earlier answer's outcome is unknown ([`Unsettled`]), nothing is sent: its outcome is read
    /// and returned instead ([`DialogAnswer::AnsweredBefore`] or [`DialogAnswer::NotShowing`]).
    pub fn answer_dialog(&mut self, answer: Value, dl: &Deadline) -> Result<DialogAnswer, ToolError> {
        if self.unsettled.is_some() {
            return self.settle_earlier_answer(dl);
        }
        let asked_before = self.questions;
        match self.request("Page.handleJavaScriptDialog", answer, None, dl) {
            Ok(Ok(_)) => Ok(DialogAnswer::Answered),
            Ok(Err(refused)) => Ok(DialogAnswer::NotShowing(refused)),
            Err(_) if self.questions != asked_before => Ok(DialogAnswer::Answered),
            Err(e) => Err(e),
        }
    }

    /// Read until the [`Unsettled`] answer's outcome is known: its reply, or a new question.
    fn settle_earlier_answer(&mut self, dl: &Deadline) -> Result<DialogAnswer, ToolError> {
        loop {
            let Some(u) = &mut self.unsettled else { return Ok(DialogAnswer::AnsweredBefore { accept: None }) };
            let accept = u.accept;
            if let Some(reply) = u.reply.take() {
                self.unsettled = None;
                return Ok(match reply {
                    Ok(()) => DialogAnswer::AnsweredBefore { accept },
                    Err(refused) => DialogAnswer::NotShowing(refused),
                });
            }
            if self.questions != u.asked_before {
                self.unsettled = None;
                return Ok(DialogAnswer::AnsweredBefore { accept });
            }
            let raw = match self.conn.read_message(dl) {
                Ok(Some(raw)) => raw,
                Ok(None) => return Err(ToolError::new(ErrorCode::EngineMissing, "the browser closed the connection before confirming the earlier answer")),
                Err(e) => {
                    return Err(ToolError::new(e.code, format!("the browser hasn't confirmed the earlier answer yet, so no other was sent: {}", e.message)));
                }
            };
            let v: Value =
                serde_json::from_slice(&raw).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("the browser sent malformed JSON: {e}")))?;
            self.on_message(v);
        }
    }

    /// An event, or a reply to a command no call is waiting for: the one that settles an
    /// [`Unsettled`] answer is kept, any other is skipped.
    fn on_message(&mut self, v: Value) {
        if v.get("method").is_some() {
            self.on_event(v);
            return;
        }
        let Some(u) = &mut self.unsettled else { return };
        if v.get("id").and_then(Value::as_u64) != Some(u.id) || u.reply.is_some() {
            return;
        }
        u.reply = Some(match v.get("error") {
            Some(err) => {
                let m = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
                Err(cdp_error("Page.handleJavaScriptDialog", m).with_details(json!({"error": err})))
            }
            None => Ok(()),
        });
        // Taken, and the page hasn't asked anything since: the question it answered is gone.
        if v.get("error").is_none() && self.questions == u.asked_before {
            self.waiting = None;
        }
    }

    /// Keep an event, answering it first when it's a dialog this client answers.
    fn on_event(&mut self, v: Value) {
        match v.get("method").and_then(Value::as_str) {
            Some(DIALOG_CLOSED) => self.waiting = None,
            // Each frame's own stop (or its removal) ends its load: the root document's load event
            // doesn't, since an iframe can still be loading then.
            Some("Page.frameStartedLoading") => {
                let id = v["params"]["frameId"].as_str().unwrap_or_default();
                if self.loading_frames.len() < MAX_LOADING_FRAMES {
                    self.loading_frames.insert(id.to_string());
                } else if !self.loading_frames.contains(id) {
                    self.loading_unknown = true;
                }
            }
            Some("Page.frameStoppedLoading" | "Page.frameDetached") => {
                self.loading_frames.remove(v["params"]["frameId"].as_str().unwrap_or_default());
            }
            Some(DIALOG_OPENING) if self.dialogs == Dialogs::Notices && !is_notice(&v["params"]) => {
                self.waiting = Some(v["params"].clone());
                self.questions = self.questions.wrapping_add(1);
            }
            _ => {}
        }
        let answers = match self.dialogs {
            Dialogs::Left => false,
            Dialogs::Notices => is_notice(&v["params"]),
            Dialogs::All => true,
        };
        if answers && v.get("method").and_then(Value::as_str) == Some(DIALOG_OPENING) {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            let mut msg = json!({"id": id, "method": "Page.handleJavaScriptDialog", "params": dialog_answer(&v["params"])});
            if let (Some(s), Some(o)) = (v.get("sessionId").cloned(), msg.as_object_mut()) {
                o.insert("sessionId".into(), s);
            }
            // The reply comes back as a stray id and is skipped. A send that fails leaves the page
            // waiting, and the caller's own deadline ends the wait as before.
            if let Ok(text) = serde_json::to_string(&msg) {
                let _ = self.conn.send_text(&text, &Deadline::new(Duration::from_secs(5), CancelToken::default()));
            }
        }
        self.enqueue_event(v);
    }

    fn enqueue_event(&mut self, v: Value) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
            self.dropped_events += 1;
        }
        self.events.push_back(v);
    }

    /// Send a command and wait for its reply, dispatching any events seen meanwhile into the queue.
    pub fn call(&mut self, method: &str, params: Value, session_id: Option<&str>, dl: &Deadline) -> Result<Value, ToolError> {
        self.request(method, params, session_id, dl)?
    }

    /// [`Self::call`], keeping the browser's own error reply (inner) apart from a call that got no
    /// reply (outer: refused, out of time, a lost connection or a question that stopped it).
    fn request(&mut self, method: &str, params: Value, session_id: Option<&str>, dl: &Deadline) -> Result<Result<Value, ToolError>, ToolError> {
        // A page waiting on a question doesn't run anything else; only the answer gets through.
        if method != "Page.handleJavaScriptDialog"
            && let Some(d) = &self.waiting
        {
            return Err(dialog_waiting(d));
        }
        let asked_before = self.questions;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let accept = params.get("accept").and_then(Value::as_bool);
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session_id
            && let Some(o) = msg.as_object_mut()
        {
            o.insert("sessionId".into(), json!(s));
        }
        let text = serde_json::to_string(&msg).map_err(|e| ToolError::internal(e.to_string()))?;
        self.conn.send_text(&text, dl)?;
        let r = self.await_reply(id, method, asked_before, dl);
        // Sent, with no word back and no new question: what it did is read before another is sent.
        if method == "Page.handleJavaScriptDialog" && r.is_err() && self.questions == asked_before {
            self.unsettled = Some(Unsettled { id, asked_before, reply: None, accept });
        }
        r
    }

    /// Wait for the reply to request `id`, keeping the events that arrive meanwhile.
    fn await_reply(&mut self, id: u64, method: &str, asked_before: u64, dl: &Deadline) -> Result<Result<Value, ToolError>, ToolError> {
        loop {
            let raw = match self.conn.read_message(dl)? {
                Some(r) => r,
                None => return Err(ToolError::new(ErrorCode::EngineMissing, "the browser closed the connection before replying")),
            };
            let v: Value = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => return Err(ToolError::new(ErrorCode::GarbledOutput, format!("the browser sent malformed JSON: {e}"))),
            };
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    let m = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
                    return Ok(Err(cdp_error(method, m).with_details(json!({"error": err}))));
                }
                // Answered: the page goes on, even before its closed event arrives.
                if method == "Page.handleJavaScriptDialog" {
                    self.waiting = None;
                }
                return Ok(Ok(v.get("result").cloned().unwrap_or(Value::Null)));
            }
            // Anything else is an event or a reply to a different id (on_message).
            self.on_message(v);
            // Only a question asked during this call stops it: the answer to the one already
            // held waits for its own reply.
            if self.questions != asked_before
                && let Some(d) = &self.waiting
            {
                return Err(dialog_waiting(d));
            }
        }
    }

    /// Pump incoming messages until the deadline passes, collecting events. Replies to prior
    /// commands are ignored, except the one an [`Unsettled`] answer waits for. Used by
    /// observation windows.
    pub fn pump_until(&mut self, until: &Deadline) -> Result<(), ToolError> {
        loop {
            if until.expired() {
                return Ok(());
            }
            match self.conn.read_message(until) {
                Ok(Some(raw)) => {
                    if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
                        self.on_message(v);
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) if e.code == ErrorCode::TimedOut => return Ok(()),
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) => return Err(e),
            }
        }
    }

    /// Drain the queued events, parsed into `CdpEvent`s.
    pub fn take_events(&mut self) -> Vec<CdpEvent> {
        self.events
            .drain(..)
            .map(|v| CdpEvent {
                method: v.get("method").and_then(Value::as_str).unwrap_or_default().to_string(),
                params: v.get("params").cloned().unwrap_or(Value::Null),
                session_id: v.get("sessionId").and_then(Value::as_str).map(String::from),
            })
            .collect()
    }

    pub fn close(&mut self, dl: &Deadline) {
        self.conn.close(dl);
    }
}

/// The error for a call the page can't answer because it's waiting on a `confirm` or `prompt`.
fn dialog_waiting(opening: &Value) -> ToolError {
    let kind = opening.get("type").and_then(Value::as_str).unwrap_or("dialog");
    let message = opening.get("message").and_then(Value::as_str).unwrap_or_default();
    let first = message.lines().next().unwrap_or_default().chars().take(300).collect::<String>();
    ToolError::new(ErrorCode::ProgramFailed, format!("the page is waiting for an answer to its {kind} (\"{first}\")"))
        .with_remediation("Answer the page's question first; nothing else runs in the page until it's answered.")
        .with_details(json!({ DIALOG_DETAIL: opening }))
}

fn cdp_error(method: &str, message: &str) -> ToolError {
    // A method/domain the browser doesn't implement is reported as unavailable, not a failure.
    let lower = message.to_ascii_lowercase();
    if lower.contains("wasn't found") || lower.contains("was not found") || lower.contains("not implemented") || lower.contains("unknown method") {
        ToolError::new(ErrorCode::FeatureMissing, format!("the browser does not support {method}: {message}"))
    } else {
        ToolError::new(ErrorCode::ProgramFailed, format!("the browser rejected {method}: {message}"))
    }
}

// ---------------------------------------------------------------------------------------------
// base64 and the WebSocket accept token (small, dependency-light)

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn expected_accept(key: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(key.as_bytes());
    h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64_encode(&h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_loopback_only() {
        assert_eq!(parse_endpoint("http://127.0.0.1:9222").unwrap().port(), 9222);
        assert_eq!(parse_endpoint("http://[::1]:9222/").unwrap().port(), 9222);
        for bad in [
            "http://localhost:9222",
            "http://127.0.0.1",
            "https://127.0.0.1:9222",
            "http://10.0.0.1:9222",
            "http://user:pw@127.0.0.1:9222",
            "http://127.0.0.1:9222/json",
            "http://127.0.0.1:9222?x=1",
            "http://127.0.0.1:0",
            "ws://127.0.0.1:9222",
            "",
        ] {
            assert_eq!(parse_endpoint(bad).unwrap_err().code, ErrorCode::BadSettings, "{bad}");
        }
    }

    #[test]
    fn ws_target_must_share_the_port() {
        let ep = parse_endpoint("http://127.0.0.1:9222").unwrap();
        let t = ws_target("ws://127.0.0.1:9222/devtools/page/ABC", &ep).unwrap();
        assert_eq!(t.path, "/devtools/page/ABC");
        assert!(ws_target("ws://127.0.0.1:9333/x", &ep).is_err());
        assert!(ws_target("ws://localhost:9222/x", &ep).is_err());
        assert!(ws_target("wss://127.0.0.1:9222/x", &ep).is_err());
    }

    #[test]
    fn accept_token_matches_rfc_example() {
        // RFC 6455 §1.3 worked example.
        assert_eq!(expected_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn find_helper() {
        assert_eq!(find(b"abc\r\n\r\nxyz", b"\r\n\r\n"), Some(3));
        assert_eq!(find(b"abc", b"\r\n\r\n"), None);
    }

    #[test]
    fn a_dialog_is_closed_but_never_agreed_to() {
        let answer = |kind: Value| dialog_answer(&json!({"type": kind, "message": "m"}))["accept"].clone();
        assert_eq!(answer(json!("alert")), json!(true), "an alert has only OK");
        assert_eq!(answer(json!("beforeunload")), json!(true), "leaving the page lets the navigation go on");
        assert_eq!(answer(json!("confirm")), json!(false), "what the page asks to do isn't agreed to");
        assert_eq!(answer(json!("prompt")), json!(false));
        assert_eq!(answer(json!("something-new")), json!(false), "an unknown kind is dismissed");
        assert_eq!(answer(Value::Null), json!(false));
    }

    /// A stand-in page over a real WebSocket: `script` runs on the server side of the connection.
    fn stand_in(script: impl FnOnce(&mut TcpStream) + Send + 'static) -> CdpClient {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while find(&head, b"\r\n\r\n").is_none() && s.read(&mut b).unwrap_or(0) == 1 {
                head.push(b[0]);
            }
            let text = String::from_utf8_lossy(&head).into_owned();
            let key = text.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).unwrap_or_default().trim().to_string();
            let reply = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                expected_accept(&key)
            );
            s.write_all(reply.as_bytes()).unwrap();
            script(&mut s);
        });
        let ep = parse_endpoint(&format!("http://127.0.0.1:{port}")).unwrap();
        let target = ws_target(&format!("ws://127.0.0.1:{port}/devtools/page/P"), &ep).unwrap();
        CdpClient::new(WsConn::connect(&target, 1 << 20, &Deadline::new(Duration::from_secs(5), CancelToken::default())).unwrap())
    }

    /// Send one unmasked text frame, as a browser does.
    fn send(s: &mut TcpStream, v: Value) {
        let body = v.to_string().into_bytes();
        let mut frame = vec![0x81u8];
        if body.len() < 126 {
            frame.push(body.len() as u8);
        } else {
            frame.push(126);
            frame.extend_from_slice(&(body.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(&body);
        s.write_all(&frame).unwrap();
    }

    /// Read one masked client frame's JSON.
    fn receive(s: &mut TcpStream) -> Value {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).unwrap();
        let mut len = usize::from(h[1] & 0x7f);
        if len == 126 {
            let mut l = [0u8; 2];
            s.read_exact(&mut l).unwrap();
            len = usize::from(u16::from_be_bytes(l));
        }
        let mut mask = [0u8; 4];
        s.read_exact(&mut mask).unwrap();
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).unwrap();
        for (i, byte) in body.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
        serde_json::from_slice(&body).unwrap()
    }

    #[test]
    fn an_answer_waits_for_its_reply_and_a_new_question_stops_a_call() {
        let mut client = stand_in(|s| {
            send(s, json!({"method": DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            // The answer: an unrelated event comes first, then its reply.
            let answer = receive(s);
            assert_eq!(answer["method"], "Page.handleJavaScriptDialog");
            send(s, json!({"method": "Network.requestWillBeSent", "params": {"requestId": "1"}}));
            send(s, json!({"id": answer["id"], "result": {}}));
            // A call the page interrupts with a new question gets no reply.
            let _evaluate = receive(s);
            send(s, json!({"method": DIALOG_OPENING, "params": {"type": "prompt", "message": "Your team?", "defaultPrompt": "Sales"}}));
            let _ = receive(s);
        })
        .answering_notices();
        let dl = Deadline::new(Duration::from_secs(5), CancelToken::default());
        client.pump_until(&Deadline::new(Duration::from_millis(300), CancelToken::default())).unwrap();
        assert_eq!(client.waiting_dialog().map(|d| d["type"].clone()), Some(json!("confirm")), "the question is held, not answered");
        let refused = client.call("Runtime.evaluate", json!({}), None, &dl).unwrap_err();
        assert!(refused.message.contains("waiting for an answer"), "nothing else runs meanwhile: {}", refused.message);
        client.call("Page.handleJavaScriptDialog", json!({"accept": true}), None, &dl).unwrap();
        assert!(client.waiting_dialog().is_none(), "answered");
        let started = Instant::now();
        let stopped = client.call("Runtime.evaluate", json!({}), None, &dl).unwrap_err();
        assert_eq!(stopped.details.as_ref().map(|d| d[DIALOG_DETAIL]["type"].clone()), Some(json!("prompt")));
        assert!(started.elapsed() < Duration::from_secs(2), "at once, not at the deadline");
    }

    /// An answer tells a refusal (no dialog showing) apart from a call that couldn't tell: one out
    /// of time keeps the question and the connection, and a retry reads what that answer did
    /// instead of sending another, which would land on the question asked since. A lost
    /// connection says so.
    #[test]
    fn an_answer_tells_a_refusal_from_no_word() {
        // The late reply is released only once the first call has timed out.
        let (release, released) = std::sync::mpsc::channel::<()>();
        let mut client = stand_in(move |s| {
            send(s, json!({"method": DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            // The first answer is taken late, after its call ran out of time, and the page asks next.
            let first = receive(s);
            assert_eq!(first["params"]["accept"], true);
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            send(s, json!({"id": first["id"], "result": {}}));
            send(s, json!({"method": DIALOG_OPENING, "params": {"type": "prompt", "message": "Your team?", "defaultPrompt": ""}}));
            // The retry sent nothing: the next answer to arrive is the one given to the prompt,
            // and it's refused (the page closed the prompt itself).
            let next = receive(s);
            assert_eq!(next["params"], json!({"accept": true, "promptText": "Design"}), "the retry's answer never reached the prompt: {next}");
            send(s, json!({"id": next["id"], "error": {"code": -32000, "message": "No dialog is showing"}}));
            // Then the browser goes away.
        })
        .answering_notices();
        let dl = Deadline::new(Duration::from_secs(5), CancelToken::default());
        client.pump_until(&Deadline::new(Duration::from_millis(300), CancelToken::default())).unwrap();
        let short = Deadline::new(Duration::from_millis(300), CancelToken::default());
        let late = client.answer_dialog(json!({"accept": true}), &short).unwrap_err();
        assert_eq!(late.code, ErrorCode::TimedOut, "{late:?}");
        assert!(client.connected() && client.waiting_dialog().is_some(), "the question and its connection are kept for another try");
        release.send(()).unwrap();
        // The retry's own choice differs: what's reported is the answer the page took.
        assert!(
            matches!(client.answer_dialog(json!({"accept": false}), &dl), Ok(DialogAnswer::AnsweredBefore { accept: Some(true) })),
            "the late answer was taken"
        );
        // Its reply came first; the question after it arrives next.
        client.pump_until(&Deadline::new(Duration::from_millis(300), CancelToken::default())).unwrap();
        assert_eq!(client.waiting_dialog().map(|d| d["type"].clone()), Some(json!("prompt")), "the next question is held");
        match client.answer_dialog(json!({"accept": true, "promptText": "Design"}), &dl) {
            Ok(DialogAnswer::NotShowing(e)) => assert!(e.message.contains("No dialog is showing"), "{}", e.message),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let lost = client.answer_dialog(json!({"accept": false}), &dl).unwrap_err();
        assert_eq!(lost.code, ErrorCode::EngineMissing, "{lost:?}");
        assert!(!client.connected(), "a lost connection can't be held");
    }

    /// A late reply that refuses the earlier answer means the question was already gone.
    #[test]
    fn a_late_refusal_settles_an_earlier_answer() {
        let (release, released) = std::sync::mpsc::channel::<()>();
        let mut client = stand_in(move |s| {
            send(s, json!({"method": DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = receive(s);
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            send(s, json!({"id": first["id"], "error": {"code": -32000, "message": "No dialog is showing"}}));
            // Nothing else is sent to the page.
            let _ = receive(s);
        })
        .answering_notices();
        let dl = Deadline::new(Duration::from_secs(5), CancelToken::default());
        client.pump_until(&Deadline::new(Duration::from_millis(300), CancelToken::default())).unwrap();
        let short = Deadline::new(Duration::from_millis(300), CancelToken::default());
        assert_eq!(client.answer_dialog(json!({"accept": true}), &short).unwrap_err().code, ErrorCode::TimedOut);
        release.send(()).unwrap();
        match client.answer_dialog(json!({"accept": false}), &dl) {
            Ok(DialogAnswer::NotShowing(e)) => assert!(e.message.contains("No dialog is showing"), "{}", e.message),
            other => panic!("expected the earlier answer's refusal, got {other:?}"),
        }
    }

    /// Past the frame cap, a dropped start leaves the load unknown: still loading, never idle.
    #[test]
    fn frames_past_the_cap_keep_the_page_loading() {
        let mut client = stand_in(|s| {
            for i in 0..=MAX_LOADING_FRAMES {
                send(s, json!({"method": "Page.frameStartedLoading", "params": {"frameId": format!("f{i}")}}));
            }
            for i in 0..MAX_LOADING_FRAMES {
                send(s, json!({"method": "Page.frameStoppedLoading", "params": {"frameId": format!("f{i}")}}));
            }
            probe_reply(s);
        });
        // The probe's reply comes after every event, so all of them have been read.
        let dl = Deadline::new(Duration::from_secs(5), CancelToken::default());
        client.call("Probe.read", json!({}), None, &dl).unwrap();
        assert!(client.loading_frames.is_empty(), "every tracked frame stopped, so only the dropped start keeps it loading");
        assert!(client.frames_loading(), "the frame past the cap may still be loading");
        let mut few = stand_in(|s| {
            send(s, json!({"method": "Page.frameStartedLoading", "params": {"frameId": "F"}}));
            probe_reply(s);
            send(s, json!({"method": "Page.frameStoppedLoading", "params": {"frameId": "F"}}));
            probe_reply(s);
        });
        few.call("Probe.read", json!({}), None, &dl).unwrap();
        assert!(few.frames_loading(), "the start was read");
        few.call("Probe.read", json!({}), None, &dl).unwrap();
        assert!(!few.frames_loading(), "every tracked frame stopped");
    }

    /// Reply to the client's next command, after the events sent before it.
    fn probe_reply(s: &mut TcpStream) {
        let m = receive(s);
        send(s, json!({"id": m["id"], "result": {}}));
    }

    #[test]
    fn only_a_notice_is_answered_without_asking() {
        let is = |kind: &str| is_notice(&json!({"type": kind}));
        assert!(is("alert") && is("beforeunload"));
        assert!(!is("confirm") && !is("prompt") && !is("something-new") && !is_notice(&Value::Null));
        let e = dialog_waiting(&json!({"type": "confirm", "message": "Continue to the new version?\nmore", "defaultPrompt": ""}));
        assert_eq!(e.message, "the page is waiting for an answer to its confirm (\"Continue to the new version?\")");
        assert_eq!(e.details.as_ref().and_then(|d| d[DIALOG_DETAIL]["type"].as_str()), Some("confirm"), "the question travels with the error");
    }
}
