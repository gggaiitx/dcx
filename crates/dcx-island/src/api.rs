//! Local push API — the island's front door for other programs.
//!
//! A script that just finished building, a watcher that found a download, a
//! game that hit a checkpoint: they all want thirty pixels of the user's
//! attention without stealing the desktop. `POST /v1/island/push` gives them
//! exactly that — a card in the 动态 tab and a pulse on the capsule, never an
//! auto-opened panel.
//!
//! # Why hand-rolled HTTP
//!
//! The workspace already hand-writes IMAP, SMTP and a WinHTTP client; a
//! loopback-only HTTP/1.1 server is the same kind of code and keeps the tree
//! free of tokio/hyper (axum's runtime model does not fit a single eframe
//! loop). The surface is deliberately tiny: four endpoints, no chunked
//! encoding, no keep-alive, 8 KB of headers and 64 KB of body, five-second
//! timeouts, at most 16 concurrent connections.
//!
//! # Security boundary
//!
//! * Binds `127.0.0.1` only — nothing is reachable from the network.
//! * Every endpoint but `GET /v1/health` requires `Authorization: Bearer`
//!   with the token from `island_api_token.txt` (see `dcx_island_config::
//!   api_token` for why the token is a file and not TOML/Credential Manager).
//! * Actions are a closed whitelist: open an http(s) URL, open an existing
//!   folder, show a panel tab. There is no "run command" and there will not
//!   be one — a push must not become remote execution.
//!
//! # Threading
//!
//! One accept thread, one short-lived thread per connection. Accepted cards
//! go two places: an `Arc<Mutex>` snapshot that `GET active` reads, and an
//! mpsc channel the UI thread drains in housekeeping. The UI store
//! ([`PushCards`]) keeps the single-writer rule every other island model
//! follows; TTL expiry also runs there, so a card never disappears mid-frame.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::inbox::now_secs;
use crate::ui::Tab;

pub const DEFAULT_TTL: u64 = 12;
pub const MAX_TTL: u64 = 3600;
/// Cards are attention, not storage: a backlog longer than this drops its
/// oldest entries instead of growing.
pub const MAX_CARDS: usize = 16;
const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 64 * 1024;
const MAX_CONNS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushAction {
    OpenUrl { url: String },
    OpenFolder { path: String },
    ShowPanel { tab: String },
}

impl PushAction {
    pub fn label(&self) -> &'static str {
        match self {
            PushAction::OpenUrl { .. } => "打开链接",
            PushAction::OpenFolder { .. } => "打开文件夹",
            PushAction::ShowPanel { .. } => "查看面板",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushCard {
    /// Caller-chosen identity, `[A-Za-z0-9._-]{1,64}`. Pushing the same id
    /// again updates the card and renews its TTL — that is how a progress
    /// card is driven.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// 0..=1; `None` means "not a progress card".
    #[serde(default)]
    pub progress: Option<f32>,
    #[serde(default = "default_ttl")]
    pub ttl_secs: u64,
    #[serde(default)]
    pub action: Option<PushAction>,
    /// Set by the server on accept; a client cannot backdate or pre-expire.
    #[serde(default)]
    pub created_at: u64,
}

fn default_ttl() -> u64 {
    DEFAULT_TTL
}

impl PushCard {
    pub fn expires_at(&self) -> u64 {
        self.created_at.saturating_add(self.ttl_secs)
    }

    /// Shape and limit checks. Returns a human-readable reason for the 400.
    pub fn validate(&mut self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 64 || !id_chars_ok(&self.id) {
            return Err("id 需为 1–64 位字母/数字/点/下划线/连字符".to_string());
        }
        if self.title.trim().is_empty() {
            return Err("title 不能为空".to_string());
        }
        if self.title.chars().count() > 200 {
            return Err("title 最长 200 字符".to_string());
        }
        if self.body.chars().count() > 2000 {
            return Err("body 最长 2000 字符".to_string());
        }
        if let Some(p) = self.progress {
            if !p.is_finite() || !(0.0..=1.0).contains(&p) {
                return Err("progress 需在 0..1 之间".to_string());
            }
        }
        // 0 reads as "unset" (the wire default), overlong TTLs clamp instead
        // of failing: a build script should not die on a typo in a lifetime.
        if self.ttl_secs == 0 {
            self.ttl_secs = DEFAULT_TTL;
        }
        self.ttl_secs = self.ttl_secs.min(MAX_TTL);
        if let Some(action) = &self.action {
            match action {
                PushAction::OpenUrl { url } => {
                    if !valid_url(url) {
                        return Err("open_url 仅支持 http/https 链接".to_string());
                    }
                }
                PushAction::OpenFolder { path } => {
                    if path.trim().is_empty() || path.contains('\0') {
                        return Err("open_folder 路径为空".to_string());
                    }
                    // Existence is checked at execution time: a build may
                    // legitimately push "open the output dir" before the
                    // first artifact lands.
                }
                PushAction::ShowPanel { tab } => {
                    if !Tab::all().iter().any(|t| t.key() == tab) {
                        return Err(format!("show_panel 不认识 tab「{tab}」"));
                    }
                }
            }
        }
        Ok(())
    }
}

fn id_chars_ok(id: &str) -> bool {
    id.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn valid_url(url: &str) -> bool {
    let url = url.trim();
    if url.contains('\0') || url.chars().any(|c| c.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"));
    rest.is_some_and(|r| !r.is_empty())
}

/// What the capsule prints for a card: a progress card shows its percentage,
/// everything else its title. The capsule clips to its own width.
pub fn capsule_text(card: &PushCard) -> String {
    match card.progress {
        Some(p) => format!("{} {}%", card.title, (p * 100.0).round() as u32),
        None => card.title.clone(),
    }
}

// ---------------------------------------------------------------------------
// UI-side store (single writer: the eframe thread)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct PushCards {
    /// Newest first.
    cards: Vec<PushCard>,
    /// Bumped on every change so the UI can repaint without diffing.
    pub version: u64,
}

impl PushCards {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace by id. Returns true when the id is new — the caller
    /// flashes the capsule only for new cards, not for every progress tick.
    pub fn upsert(&mut self, card: PushCard) -> bool {
        let is_new = !self.cards.iter().any(|c| c.id == card.id);
        self.cards.retain(|c| c.id != card.id);
        self.cards.insert(0, card);
        self.cards.truncate(MAX_CARDS);
        self.version += 1;
        is_new
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.id != id);
        let changed = self.cards.len() != before;
        if changed {
            self.version += 1;
        }
        changed
    }

    /// Drop expired cards. Returns true when anything changed.
    pub fn expire(&mut self, now: u64) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.expires_at() > now);
        let changed = self.cards.len() != before;
        if changed {
            self.version += 1;
        }
        changed
    }

    pub fn get(&self, id: &str) -> Option<&PushCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PushCard> {
        self.cards.iter()
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    /// Newest unexpired card — what the capsule shows when no reminder runs.
    pub fn latest(&self) -> Option<&PushCard> {
        let now = now_secs();
        self.cards.iter().find(|c| c.expires_at() > now)
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

pub enum ApiMsg {
    Upsert(PushCard),
    Remove(String),
}

/// Everything a connection handler may touch. The snapshot is the server's
/// own read model for `GET active`; the channel feeds the UI's store.
struct Shared {
    snapshot: Arc<Mutex<Vec<PushCard>>>,
    tx: Sender<ApiMsg>,
    token_cache: Mutex<Option<(SystemTime, String)>>,
}

pub struct ApiServer {
    pub rx: Receiver<ApiMsg>,
    stop: Arc<AtomicBool>,
    port: u16,
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The accept thread is blocked in `accept`; one loopback connection
        // wakes it so it can see the flag and close the listener.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// Start the API. `None` means disabled or degraded (bind failure is logged,
/// never fatal — the island must run with the port taken).
pub fn start(enabled: bool, port: u16) -> Option<ApiServer> {
    if !enabled {
        return None;
    }
    if let Err(e) = dcx_island_config::api_token::ensure() {
        crate::win::trace(&format!("api: token unavailable ({e}); API disabled"));
        return None;
    }
    // A settings-driven restart drops the old server and binds the same port
    // immediately; the old accept thread may not have closed the listener
    // yet, so give it a moment instead of reporting a false conflict.
    let listener = {
        let mut last = None;
        let mut bound = None;
        for _ in 0..20 {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(l) => {
                    bound = Some(l);
                    break;
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        match bound {
            Some(l) => l,
            None => {
                crate::win::trace(&format!(
                    "api: bind 127.0.0.1:{port} failed ({}); API disabled",
                    last.map(|e| e.to_string()).unwrap_or_default()
                ));
                return None;
            }
        }
    };
    let (tx, rx) = channel();
    let shared = Arc::new(Shared {
        snapshot: Arc::new(Mutex::new(Vec::new())),
        tx,
        token_cache: Mutex::new(None),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let conns = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("dcx-island-api".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                if conns.load(Ordering::SeqCst) >= MAX_CONNS {
                    let mut stream = stream;
                    respond(&mut stream, 503, "Service Unavailable", "{\"ok\":false,\"error\":\"too many connections\"}");
                    continue;
                }
                conns.fetch_add(1, Ordering::SeqCst);
                let shared = Arc::clone(&shared);
                let conns = Arc::clone(&conns);
                // Named for the debugger; a handler lives at most seconds.
                let _ = std::thread::Builder::new()
                    .name("dcx-island-api-conn".to_string())
                    .spawn(move || {
                        handle(stream, &shared);
                        conns.fetch_sub(1, Ordering::SeqCst);
                    });
            }
        })
        .ok()?;
    crate::win::trace(&format!("api: listening on 127.0.0.1:{port}"));
    Some(ApiServer { rx, stop, port })
}

/// Re-read the token only when its file changed — a settings-side reset
/// takes effect on the next request, no restart involved.
fn current_token(shared: &Shared) -> Option<String> {
    let stamp = dcx_island_config::api_token::stamp()?;
    let mut slot = shared.token_cache.lock().ok()?;
    if let Some((t, tok)) = slot.as_ref() {
        if *t == stamp {
            return Some(tok.clone());
        }
    }
    let tok = dcx_island_config::api_token::read()?;
    *slot = Some((stamp, tok.clone()));
    Some(tok)
}

struct Request {
    method: String,
    target: String,
    authorization: Option<String>,
    content_length: usize,
    chunked: bool,
}

/// `(code, reason, body)` — the body is always JSON.
type Reply = (u16, &'static str, String);

fn handle(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let (req, body) = match read_request(&mut stream) {
        Ok(x) => x,
        Err(reply) => {
            respond(&mut stream, reply.0, reply.1, &reply.2);
            return;
        }
    };
    let authorized = match current_token(shared) {
        Some(token) => req
            .authorization
            .as_deref()
            .map(|a| constant_time_eq(a, &format!("Bearer {token}")))
            .unwrap_or(false),
        None => false,
    };
    let reply = route(&req, &body, authorized, shared);
    respond(&mut stream, reply.0, reply.1, &reply.2);
}

/// Byte-wise compare that does not bail on the first mismatch. The secret is
/// a local bearer token, not a password hash — this is cheap insurance, not
/// the security model.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn read_request(stream: &mut TcpStream) -> Result<(Request, Vec<u8>), Reply> {
    let mut reader = BufReader::new(stream.try_clone().map_err(io_reply)?);
    // Header block, byte at a time through the buffer: a line-based reader
    // would let one unterminated line grow without bound.
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).map_err(io_reply)?;
        if n == 0 {
            return Err(bad_request("连接在头部读完前关闭"));
        }
        head.push(byte[0]);
        if head.len() > MAX_HEADER {
            return Err((431, "Request Header Fields Too Large", error_json("头部超过 8 KB")));
        }
        if head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let version = parts.next().unwrap_or("");
    if method.is_empty() || target.is_empty() || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(bad_request("请求行不是 HTTP/1.x"));
    }
    let mut req = Request {
        method,
        target,
        authorization: None,
        content_length: 0,
        chunked: false,
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => req.authorization = Some(value.to_string()),
            "content-length" => {
                req.content_length = value
                    .parse()
                    .map_err(|_| bad_request("Content-Length 不是数字"))?;
            }
            "transfer-encoding" => req.chunked = true,
            _ => {}
        }
    }
    if req.chunked {
        return Err(bad_request("不支持 chunked 编码，请用 Content-Length"));
    }
    if req.content_length > MAX_BODY {
        return Err((413, "Payload Too Large", error_json("body 超过 64 KB")));
    }
    let mut body = Vec::with_capacity(req.content_length.min(4096));
    reader
        .take(req.content_length as u64)
        .read_to_end(&mut body)
        .map_err(io_reply)?;
    if body.len() != req.content_length {
        return Err(bad_request("body 短于 Content-Length"));
    }
    Ok((req, body))
}

fn route(req: &Request, body: &[u8], authorized: bool, shared: &Shared) -> Reply {
    // The only unauthenticated endpoint: a liveness probe scripts can use
    // before they have a token.
    if req.method == "GET" && req.target == "/v1/health" {
        return (
            200,
            "OK",
            format!(
                "{{\"ok\":true,\"app\":\"dcx-island\",\"version\":\"{}\"}}",
                env!("CARGO_PKG_VERSION")
            ),
        );
    }
    if !authorized {
        return (401, "Unauthorized", error_json("需要 Authorization: Bearer <token>"));
    }
    match (req.method.as_str(), req.target.as_str()) {
        ("POST", "/v1/island/push") => {
            let mut card: PushCard = match serde_json::from_slice(body) {
                Ok(c) => c,
                Err(e) => return bad_request(&format!("JSON 解析失败：{e}")),
            };
            if let Err(e) = card.validate() {
                return bad_request(&e);
            }
            card.created_at = now_secs();
            let expires_at = card.expires_at();
            let id = card.id.clone();
            if let Ok(mut snap) = shared.snapshot.lock() {
                snap.retain(|c| c.id != id);
                snap.insert(0, card.clone());
                snap.truncate(MAX_CARDS);
            }
            if shared.tx.send(ApiMsg::Upsert(card)).is_err() {
                return (503, "Service Unavailable", error_json("信息岛正在退出"));
            }
            (200, "OK", format!("{{\"ok\":true,\"id\":\"{id}\",\"expires_at\":{expires_at}}}"))
        }
        ("GET", "/v1/island/active") => {
            let now = now_secs();
            let cards: Vec<PushCard> = shared
                .snapshot
                .lock()
                .map(|s| s.iter().filter(|c| c.expires_at() > now).cloned().collect())
                .unwrap_or_default();
            match serde_json::to_string(&serde_json::json!({ "ok": true, "cards": cards })) {
                Ok(text) => (200, "OK", text),
                Err(_) => (500, "Internal Server Error", error_json("序列化失败")),
            }
        }
        ("DELETE", target) if target.starts_with("/v1/island/push/") => {
            let id = &target["/v1/island/push/".len()..];
            if id.is_empty() || id.len() > 64 || !id_chars_ok(id) {
                return bad_request("id 需为 1–64 位字母/数字/点/下划线/连字符");
            }
            if let Ok(mut snap) = shared.snapshot.lock() {
                snap.retain(|c| c.id != id);
            }
            if shared.tx.send(ApiMsg::Remove(id.to_string())).is_err() {
                return (503, "Service Unavailable", error_json("信息岛正在退出"));
            }
            // Idempotent: deleting an absent id is still success.
            (200, "OK", format!("{{\"ok\":true,\"id\":\"{id}\"}}"))
        }
        _ => (404, "Not Found", error_json("没有这个端点")),
    }
}

fn bad_request(msg: &str) -> Reply {
    (400, "Bad Request", error_json(msg))
}

fn io_reply(e: std::io::Error) -> Reply {
    (400, "Bad Request", error_json(&format!("读取失败：{e}")))
}

fn error_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}

fn respond(stream: &mut TcpStream, code: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

// ---------------------------------------------------------------------------
// Action execution (UI thread)
// ---------------------------------------------------------------------------

/// Run a card's action. `ShowPanel` is the caller's job (it needs the window);
/// everything here is validated again at execution time — a folder that was
/// pushed before it existed may exist now, or still not.
pub fn run_action(action: &PushAction) -> Result<(), String> {
    match action {
        PushAction::OpenUrl { url } => {
            if !valid_url(url) {
                return Err("链接仅支持 http/https".to_string());
            }
            shell_open(url.trim())
        }
        PushAction::OpenFolder { path } => {
            if !std::path::Path::new(path).is_dir() {
                return Err(format!("目录不存在：{path}"));
            }
            shell_open(path)
        }
        PushAction::ShowPanel { .. } => Ok(()),
    }
}

/// `ShellExecuteW("open", …)`: the default browser for a URL, Explorer for a
/// directory — the same call `dcx-box` uses, no command line to quote.
fn shell_open(target: &str) -> Result<(), String> {
    let op = dcx_win32::wide("open");
    let file = dcx_win32::wide(target);
    let code = unsafe {
        dcx_win32::ShellExecuteW(
            0,
            op.as_ptr(),
            file.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    if code > 32 {
        Ok(())
    } else {
        Err(format!("打开失败（ShellExecuteW 返回 {code}）"))
    }
}

// ---------------------------------------------------------------------------
// CLI client: `dcx-island.exe --push '<json>'`
// ---------------------------------------------------------------------------

/// Post one card and print the server's answer. Exit code 0 only on HTTP 200.
/// Runs in a fresh process *before* the singleton logic, so it never touches
/// `island_show` and never toggles the panel.
pub fn push_cli(json: &str) -> i32 {
    if json.trim().is_empty() {
        eprintln!("用法：dcx-island --push '{{\"id\":\"build\",\"title\":\"构建完成\"}}'");
        return 1;
    }
    if let Err(e) = serde_json::from_str::<serde_json::Value>(json) {
        eprintln!("JSON 解析失败：{e}");
        return 1;
    }
    let cfg = dcx_island_config::load();
    if !cfg.api.enabled {
        eprintln!("推送 API 未启用（设置中心 → 桌面信息岛 → 推送 API）");
        return 1;
    }
    let Some(token) = dcx_island_config::api_token::read() else {
        eprintln!("token 不存在：请先启动信息岛生成");
        return 1;
    };
    let mut stream = match TcpStream::connect(("127.0.0.1", cfg.api.port)) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("无法连接 127.0.0.1:{}（信息岛未运行或 API 端口不符）", cfg.api.port);
            return 1;
        }
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let head = format!(
        "POST /v1/island/push HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        json.len()
    );
    if stream.write_all(head.as_bytes()).is_err() || stream.write_all(json.as_bytes()).is_err() {
        eprintln!("发送失败");
        return 1;
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    let mut response = Vec::new();
    if stream
        .take(MAX_BODY as u64 + MAX_HEADER as u64)
        .read_to_end(&mut response)
        .is_err()
    {
        eprintln!("读取响应失败");
        return 1;
    }
    let text = String::from_utf8_lossy(&response);
    let code = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    println!("{body}");
    if code == 200 {
        0
    } else {
        1
    }
}

// ---------------------------------------------------------------------------
// Tests — the pure halves: validation, store, routing
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, title: &str) -> PushCard {
        PushCard {
            id: id.to_string(),
            title: title.to_string(),
            body: String::new(),
            progress: None,
            ttl_secs: DEFAULT_TTL,
            action: None,
            created_at: 0,
        }
    }

    fn shared() -> (Arc<Shared>, Receiver<ApiMsg>) {
        let (tx, rx) = channel();
        (
            Arc::new(Shared {
                snapshot: Arc::new(Mutex::new(Vec::new())),
                tx,
                token_cache: Mutex::new(None),
            }),
            rx,
        )
    }

    fn req(method: &str, target: &str) -> Request {
        Request {
            method: method.to_string(),
            target: target.to_string(),
            authorization: None,
            content_length: 0,
            chunked: false,
        }
    }

    #[test]
    fn validation_rejects_bad_shapes() {
        assert!(card("build-1", "构建完成").validate().is_ok());
        assert!(card("", "t").validate().is_err());
        assert!(card("has space", "t").validate().is_err());
        assert!(card("ok", "  ").validate().is_err());
        assert!(card(&"x".repeat(65), "t").validate().is_err());
        assert!(card("ok", &"题".repeat(201)).validate().is_err());
        let mut c = card("ok", "t");
        c.progress = Some(1.5);
        assert!(c.validate().is_err());
        c.progress = Some(f32::NAN);
        assert!(c.validate().is_err());
    }

    #[test]
    fn ttl_zero_defaults_and_overlong_clamps() {
        let mut c = card("a", "t");
        c.ttl_secs = 0;
        c.validate().unwrap();
        assert_eq!(c.ttl_secs, DEFAULT_TTL);
        c.ttl_secs = 99_999;
        c.validate().unwrap();
        assert_eq!(c.ttl_secs, MAX_TTL);
    }

    #[test]
    fn actions_are_a_closed_whitelist() {
        let mut c = card("a", "t");
        c.action = Some(PushAction::OpenUrl { url: "https://example.com".into() });
        assert!(c.validate().is_ok());
        c.action = Some(PushAction::OpenUrl { url: "file:///C:/Windows".into() });
        assert!(c.validate().is_err());
        c.action = Some(PushAction::OpenUrl { url: "javascript:alert(1)".into() });
        assert!(c.validate().is_err());
        c.action = Some(PushAction::ShowPanel { tab: "live".into() });
        assert!(c.validate().is_ok());
        c.action = Some(PushAction::ShowPanel { tab: "nope".into() });
        assert!(c.validate().is_err());
        // Unknown kinds do not even deserialize.
        assert!(serde_json::from_str::<PushAction>(r#"{"kind":"run_command","cmd":"calc"}"#).is_err());
    }

    #[test]
    fn store_upsert_renews_and_only_new_ids_flash() {
        let mut store = PushCards::new();
        let mut c = card("build", "构建中");
        c.created_at = 100;
        assert!(store.upsert(c.clone()));
        c.created_at = 200;
        c.progress = Some(0.5);
        assert!(!store.upsert(c));
        assert_eq!(store.len(), 1);
        assert_eq!(store.get("build").unwrap().created_at, 200);
        assert_eq!(store.get("build").unwrap().progress, Some(0.5));
    }

    #[test]
    fn store_is_capped_and_expires() {
        let mut store = PushCards::new();
        for i in 0..(MAX_CARDS + 4) {
            let mut c = card(&format!("c{i}"), "t");
            c.created_at = 1000 + i as u64;
            store.upsert(c);
        }
        assert_eq!(store.len(), MAX_CARDS);
        // Newest first: c19 survived, c0 dropped.
        assert!(store.get("c0").is_none());
        assert!(store.get(&format!("c{}", MAX_CARDS + 3)).is_some());
        assert!(store.expire(1000 + MAX_CARDS as u64 + 4 + DEFAULT_TTL + 1));
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn health_needs_no_token_but_push_does() {
        let (sh, rx) = shared();
        let reply = route(&req("GET", "/v1/health"), b"", false, &sh);
        assert_eq!(reply.0, 200);
        let reply = route(&req("POST", "/v1/island/push"), b"{}", false, &sh);
        assert_eq!(reply.0, 401);
        let reply = route(&req("GET", "/v1/island/active"), b"", false, &sh);
        assert_eq!(reply.0, 401);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn push_upserts_snapshot_and_channel_then_delete_removes() {
        let (sh, rx) = shared();
        let body = r#"{"id":"build","title":"构建完成","ttl_secs":30}"#.as_bytes();
        let reply = route(&req("POST", "/v1/island/push"), body, true, &sh);
        assert_eq!(reply.0, 200, "{}", reply.2);
        assert!(matches!(rx.try_recv(), Ok(ApiMsg::Upsert(_))));
        assert_eq!(sh.snapshot.lock().unwrap().len(), 1);

        // Same id again: still one card, renewed.
        let reply = route(&req("POST", "/v1/island/push"), body, true, &sh);
        assert_eq!(reply.0, 200);
        assert_eq!(sh.snapshot.lock().unwrap().len(), 1);
        assert!(matches!(rx.try_recv(), Ok(ApiMsg::Upsert(_))));

        let active = route(&req("GET", "/v1/island/active"), b"", true, &sh);
        assert_eq!(active.0, 200);
        assert!(active.2.contains("\"build\""));

        let reply = route(&req("DELETE", "/v1/island/push/build"), b"", true, &sh);
        assert_eq!(reply.0, 200);
        assert!(matches!(rx.try_recv(), Ok(ApiMsg::Remove(id)) if id == "build"));
        assert!(sh.snapshot.lock().unwrap().is_empty());
        // Idempotent.
        assert_eq!(route(&req("DELETE", "/v1/island/push/build"), b"", true, &sh).0, 200);
    }

    #[test]
    fn push_rejects_bad_json_and_unknown_routes_404() {
        let (sh, _rx) = shared();
        assert_eq!(route(&req("POST", "/v1/island/push"), b"not json", true, &sh).0, 400);
        assert_eq!(route(&req("POST", "/v1/island/push"), b"{}", true, &sh).0, 400);
        assert_eq!(route(&req("GET", "/v1/nope"), b"", true, &sh).0, 404);
        assert_eq!(route(&req("DELETE", "/v1/island/push/bad id"), b"", true, &sh).0, 400);
    }

    #[test]
    fn capsule_text_shows_percent_for_progress() {
        let mut c = card("build", "构建");
        assert_eq!(capsule_text(&c), "构建");
        c.progress = Some(0.42);
        assert_eq!(capsule_text(&c), "构建 42%");
    }

    #[test]
    fn bearer_compare_is_exact() {
        assert!(constant_time_eq("Bearer abc", "Bearer abc"));
        assert!(!constant_time_eq("Bearer abc", "Bearer abd"));
        assert!(!constant_time_eq("Bearer abc", "Bearer ab"));
        assert!(!constant_time_eq("bearer abc", "Bearer abc"));
    }
}