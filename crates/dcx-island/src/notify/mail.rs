//! Mail: unread headers over IMAP, reminder delivery over SMTP.
//!
//! No mail crate: the protocol surface actually needed here is four commands on
//! the way in and six on the way out, and both fit in this file. TLS comes from
//! `native-tls`, which on Windows is SChannel — no OpenSSL, no vendored build,
//! and the certificate store is the system one.
//!
//! # Scope
//!
//! Only `UNSEEN` message **headers** are fetched (`BODY.PEEK`, so the server
//! does not mark them read). The body is never downloaded and never stored. The
//! first sync of a session records the current unread set without emitting
//! reminders — otherwise opening the island would replay a week of mail.
//!
//! # What it does not do
//!
//! STARTTLS on port 587. QQ/163/Outlook all offer implicit TLS on 465/993,
//! which is what this module implements; a server that only speaks STARTTLS is
//! reported as unsupported rather than silently failing to authenticate.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use native_tls::TlsConnector;

use crate::inbox::ChannelState;

#[derive(Clone, Debug)]
pub struct MailSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub poll_secs: u64,
    pub skip_backlog: bool,
}

#[derive(Clone, Debug)]
pub struct MailHit {
    pub uid: String,
    pub from: String,
    pub subject: String,
}

pub struct MailProbe {
    pub rx: Receiver<MailHit>,
    pub state: Arc<Mutex<ChannelState>>,
    stop: Arc<AtomicBool>,
}

impl Drop for MailProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

static SINK: Mutex<Option<Sender<MailHit>>> = Mutex::new(None);

pub fn start(cfg: MailSettings) -> Option<MailProbe> {
    let (tx, rx) = channel();
    if let Ok(mut slot) = SINK.lock() {
        *slot = Some(tx);
    }
    let state = Arc::new(Mutex::new(ChannelState::Idle));
    let state2 = Arc::clone(&state);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);

    if cfg.user.trim().is_empty() || cfg.password.is_empty() {
        if let Ok(mut s) = state.lock() {
            *s = ChannelState::Off;
        }
    }

    std::thread::Builder::new()
        .name("dcx-island-mail".to_string())
        .spawn(move || {
            let mut seen: HashSet<String> = HashSet::new();
            let mut first = true;
            let mut backoff = Duration::from_secs(5);
            while !stop2.load(Ordering::SeqCst) {
                let period = Duration::from_secs(cfg.poll_secs.max(60));
                match poll_once(&cfg, &mut seen, first, stop2.as_ref()) {
                    Ok(hits) => {
                        first = false;
                        backoff = Duration::from_secs(5);
                        if let Ok(mut s) = state2.lock() {
                            *s = ChannelState::Ok;
                        }
                        for hit in hits {
                            if let Ok(slot) = SINK.lock() {
                                if let Some(tx) = slot.as_ref() {
                                    let _ = tx.send(hit);
                                }
                            }
                        }
                        sleep_unless_stopped(&stop2, period);
                    }
                    Err(e) => {
                        if let Ok(mut s) = state2.lock() {
                            *s = ChannelState::Error(e);
                        }
                        sleep_unless_stopped(&stop2, backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(600));
                    }
                }
            }
        })
        .ok()?;

    Some(MailProbe { rx, state, stop })
}

fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) {
    let mut left = total;
    while !left.is_zero() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let step = Duration::from_millis(200).min(left);
        std::thread::sleep(step);
        left -= step;
    }
}

// ---------------------------------------------------------------------------
// IMAP
// ---------------------------------------------------------------------------

struct Resp {
    line: String,
    literal: Option<Vec<u8>>,
}

struct Imap {
    reader: BufReader<native_tls::TlsStream<TcpStream>>,
    tag: u32,
}

impl Imap {
    fn connect(host: &str, port: u16) -> Result<Imap, String> {
        let stream = TcpStream::connect((host, port))
            .map_err(|e| format!("无法连接 {host}:{port}：{e}"))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(25)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(25)));
        let tls = TlsConnector::new()
            .map_err(|e| format!("TLS 初始化失败：{e}"))?
            .connect(host, stream)
            .map_err(|e| format!("TLS 握手失败：{e}"))?;
        let mut imap = Imap {
            reader: BufReader::new(tls),
            tag: 0,
        };
        let greeting = imap.read_line()?;
        if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
            return Err(format!("服务器拒绝连接：{greeting}"));
        }
        Ok(imap)
    }

    fn read_line(&mut self) -> Result<String, String> {
        let mut raw = Vec::new();
        self.reader
            .read_until(b'\n', &mut raw)
            .map_err(|e| format!("读取失败：{e}"))?;
        if raw.is_empty() {
            return Err("连接被服务器关闭".to_string());
        }
        while raw.last() == Some(&b'\n') || raw.last() == Some(&b'\r') {
            raw.pop();
        }
        Ok(String::from_utf8_lossy(&raw).to_string())
    }

    fn write_line(&mut self, line: &str) -> Result<(), String> {
        let io = self.reader.get_mut();
        io.write_all(line.as_bytes())
            .and_then(|_| io.write_all(b"\r\n"))
            .and_then(|_| io.flush())
            .map_err(|e| format!("发送失败：{e}"))
    }

    fn command(&mut self, text: &str) -> Result<(Vec<Resp>, String), String> {
        self.tag += 1;
        let tag = format!("a{}", self.tag);
        self.write_line(&format!("{tag} {text}"))?;
        let mut out = Vec::new();
        loop {
            let line = self.read_line()?;
            if line.starts_with(&tag) {
                return Ok((out, line));
            }
            let literal = match literal_len(&line) {
                Some(n) => {
                    let mut buf = vec![0u8; n];
                    self.reader
                        .read_exact(&mut buf)
                        .map_err(|e| format!("读取邮件头失败：{e}"))?;
                    // The literal is followed by the remainder of the response
                    // line (usually just `)`).
                    let _ = self.read_line()?;
                    Some(buf)
                }
                None => None,
            };
            out.push(Resp { line, literal });
        }
    }

    fn login(&mut self, user: &str, password: &str) -> Result<(), String> {
        let (_, status) = self.command(&format!(
            "LOGIN {} {}",
            quote(user),
            quote(password)
        ))?;
        if status.contains(" OK") {
            Ok(())
        } else {
            Err(format!("登录失败：{}", trim_status(&status)))
        }
    }

    fn select_inbox(&mut self) -> Result<(), String> {
        let (_, status) = self.command("SELECT \"INBOX\"")?;
        if status.contains(" OK") {
            Ok(())
        } else {
            Err(format!("无法打开收件箱：{}", trim_status(&status)))
        }
    }

    fn unseen_uids(&mut self) -> Result<Vec<String>, String> {
        let (resps, status) = self.command("UID SEARCH UNSEEN")?;
        if !status.contains(" OK") {
            return Err(format!("搜索未读失败：{}", trim_status(&status)));
        }
        let mut uids = Vec::new();
        for r in resps {
            if let Some(list) = r.line.strip_prefix("* SEARCH") {
                uids.extend(
                    list.split_whitespace()
                        .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
                        .map(|s| s.to_string()),
                );
            }
        }
        Ok(uids)
    }

    fn fetch_headers(&mut self, uids: &[String]) -> Result<Vec<MailHit>, String> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids.join(",");
        let (resps, status) = self.command(&format!(
            "UID FETCH {set} (BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE)])"
        ))?;
        if !status.contains(" OK") {
            return Err(format!("拉取邮件头失败：{}", trim_status(&status)));
        }
        let mut out = Vec::new();
        for r in resps {
            let Some(literal) = r.literal else { continue };
            let uid = extract_uid(&r.line).unwrap_or_default();
            let headers = String::from_utf8_lossy(&literal);
            let from = header_value(&headers, "From").unwrap_or_default();
            let subject = header_value(&headers, "Subject").unwrap_or_default();
            out.push(MailHit {
                uid,
                from: decode_words(&from),
                subject: decode_words(&subject),
            });
        }
        Ok(out)
    }

    fn logout(&mut self) {
        let _ = self.command("LOGOUT");
    }
}

fn literal_len(line: &str) -> Option<usize> {
    let end = line.rfind('}')?;
    let start = line[..end].rfind('{')?;
    line[start + 1..end].parse::<usize>().ok()
}

fn extract_uid(line: &str) -> Option<String> {
    // `* 1 FETCH (UID 142 BODY[HEADER.FIELDS ...] {245}`
    let idx = line.find("UID ")?;
    let tail = &line[idx + 4..];
    let uid: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if uid.is_empty() {
        None
    } else {
        Some(uid)
    }
}

fn trim_status(status: &str) -> String {
    status
        .splitn(3, ' ')
        .nth(2)
        .unwrap_or(status)
        .trim()
        .to_string()
}

fn quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Unfold a header and return its value. Continuation lines start with space or
/// tab and belong to the previous field.
pub fn header_value(headers: &str, name: &str) -> Option<String> {
    let needle = format!("{}:", name.to_ascii_lowercase());
    let mut collecting = false;
    let mut value = String::new();
    for line in headers.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if collecting {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if collecting {
            break;
        }
        if line.to_ascii_lowercase().starts_with(&needle) {
            collecting = true;
            value.push_str(line[needle.len()..].trim());
        }
    }
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn poll_once(
    cfg: &MailSettings,
    seen: &mut HashSet<String>,
    first: bool,
    stop: &AtomicBool,
) -> Result<Vec<MailHit>, String> {
    if cfg.user.trim().is_empty() || cfg.password.is_empty() {
        return Err("未配置邮箱账号或授权码".to_string());
    }
    let mut imap = Imap::connect(&cfg.host, cfg.port)?;
    let result = (|| -> Result<Vec<MailHit>, String> {
        imap.login(&cfg.user, &cfg.password)?;
        imap.select_inbox()?;
        let mut uids = imap.unseen_uids()?;
        // Oldest first, newest last: keep the tail so the alert is about the
        // message that just arrived.
        if uids.len() > 20 {
            uids = uids.split_off(uids.len() - 20);
        }
        let fresh: Vec<String> = uids
            .into_iter()
            .filter(|u| !seen.contains(u))
            .collect();
        if fresh.is_empty() {
            return Ok(Vec::new());
        }
        let hits = imap.fetch_headers(&fresh)?;
        for u in &fresh {
            seen.insert(u.clone());
        }
        if seen.len() > 4096 {
            seen.clear();
        }
        if first && cfg.skip_backlog {
            // First contact in this session: record the unread set but do not
            // replay it as new mail.
            return Ok(Vec::new());
        }
        Ok(hits)
    })();
    imap.logout();
    let _ = stop;
    result
}

// ---------------------------------------------------------------------------
// MIME encoded words
// ---------------------------------------------------------------------------

/// Decode RFC 2047 encoded words (`=?utf-8?B?…?=`). A charset the platform
/// cannot map is left as-is instead of being replaced with question marks.
pub fn decode_words(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 1 < bytes.len() && bytes[i + 1] == b'?' {
            if let Some((text, next)) = decode_word(&bytes[i..]) {
                out.push_str(&text);
                i += next;
                continue;
            }
        }
        let ch_len = utf8_len(bytes[i]);
        let end = (i + ch_len).min(bytes.len());
        out.push_str(&String::from_utf8_lossy(&bytes[i..end]));
        i = end;
    }
    out
}

fn decode_word(src: &[u8]) -> Option<(String, usize)> {
    // =?charset?enc?payload?=
    let mut idx = 2;
    let charset_end = find(src, idx, b'?')?;
    let charset = String::from_utf8_lossy(&src[idx..charset_end]).to_string();
    idx = charset_end + 1;
    if idx >= src.len() {
        return None;
    }
    let enc = src[idx];
    if src.get(idx + 1) != Some(&b'?') {
        return None;
    }
    idx += 2;
    let payload_end = find(src, idx, b'?')?;
    let payload = &src[idx..payload_end];
    if src.get(payload_end + 1) != Some(&b'=') {
        return None;
    }
    let raw = match enc {
        b'B' | b'b' => base64_decode(payload)?,
        b'Q' | b'q' => {
            let mut v = Vec::with_capacity(payload.len());
            let mut k = 0;
            while k < payload.len() {
                if payload[k] == b'_' {
                    v.push(b' ');
                    k += 1;
                } else if payload[k] == b'=' && k + 2 < payload.len() {
                    let hex = std::str::from_utf8(&payload[k + 1..k + 3]).ok()?;
                    v.push(u8::from_str_radix(hex, 16).ok()?);
                    k += 3;
                } else {
                    v.push(payload[k]);
                    k += 1;
                }
            }
            v
        }
        _ => return None,
    };
    let encoding = encoding_rs::Encoding::for_label(charset.trim().as_bytes())
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(&raw);
    Some((text.to_string(), payload_end + 2))
}

fn find(src: &[u8], from: usize, needle: u8) -> Option<usize> {
    src.iter().skip(from).position(|b| *b == needle).map(|p| p + from)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(B64[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn base64_decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in data {
        let v = match *b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// SMTP
// ---------------------------------------------------------------------------

/// Send one plain-text message. Returns the server's final response so a
/// failure can be shown verbatim instead of "发送失败".
pub fn send_mail(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<String, String> {
    let stream = TcpStream::connect((host, port))
        .map_err(|e| format!("无法连接 {host}:{port}：{e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(25)));
    let tls = TlsConnector::new()
        .map_err(|e| format!("TLS 初始化失败：{e}"))?
        .connect(host, stream)
        .map_err(|e| format!("TLS 握手失败：{e}"))?;
    let mut r = BufReader::new(tls);

    smtp_expect(&mut r, "220")?;
    smtp_say(r.get_mut(), "EHLO dcx-island")?;
    smtp_expect(&mut r, "250")?;
    smtp_say(r.get_mut(), "AUTH LOGIN")?;
    smtp_expect(&mut r, "334")?;
    smtp_say(r.get_mut(), &base64_encode(user.as_bytes()))?;
    smtp_expect(&mut r, "334")?;
    smtp_say(r.get_mut(), &base64_encode(password.as_bytes()))?;
    smtp_expect(&mut r, "235")?;
    smtp_say(r.get_mut(), &format!("MAIL FROM:<{user}>"))?;
    smtp_expect(&mut r, "250")?;
    smtp_say(
        r.get_mut(),
        &format!("RCPT TO:<{}>", if to.trim().is_empty() { user } else { to }),
    )?;
    smtp_expect(&mut r, "250")?;
    smtp_say(r.get_mut(), "DATA")?;
    smtp_expect(&mut r, "354")?;

    let message = build_message(user, to, subject, body);
    // Dot-stuffing: a line consisting of a single '.' would end DATA early.
    for line in message.split("\r\n") {
        if line.starts_with('.') {
            smtp_say(r.get_mut(), &format!(".{line}"))?;
        } else {
            smtp_say(r.get_mut(), line)?;
        }
    }
    smtp_say(r.get_mut(), ".")?;
    let final_line = smtp_expect(&mut r, "250")?;
    let _ = smtp_say(r.get_mut(), "QUIT");
    Ok(final_line)
}

/// Read one (possibly multi-line) SMTP reply and assert its status code.
/// Free functions instead of two closures: both need `&mut BufReader`, and two
/// simultaneously-live closures cannot each hold that borrow.
fn smtp_expect<R: BufRead>(r: &mut R, code: &str) -> Result<String, String> {
    loop {
        let mut line = String::new();
        r.read_line(&mut line)
            .map_err(|e| format!("读取 SMTP 响应失败：{e}"))?;
        if line.is_empty() {
            return Err("SMTP 连接被关闭".to_string());
        }
        // Multi-line replies continue while the 4th char is '-'.
        let continued = line.as_bytes().get(3) == Some(&b'-');
        if !continued {
            if !line.starts_with(code) {
                return Err(format!("SMTP 返回：{}", line.trim()));
            }
            return Ok(line.trim().to_string());
        }
    }
}

fn smtp_say<W: Write>(w: &mut W, line: &str) -> Result<(), String> {
    w.write_all(line.as_bytes())
        .and_then(|_| w.write_all(b"\r\n"))
        .and_then(|_| w.flush())
        .map_err(|e| format!("发送 SMTP 命令失败：{e}"))
}

pub fn build_message(from: &str, to: &str, subject: &str, body: &str) -> String {
    let to = if to.trim().is_empty() { from } else { to };
    let encoded_subject = format!("=?utf-8?B?{}?=", base64_encode(subject.as_bytes()));
    let encoded_body = wrap_base64(&base64_encode(body.as_bytes()));
    format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Subject: {encoded_subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {encoded_body}"
    )
}

/// Base64 lines are limited to 76 characters by RFC 2045.
fn wrap_base64(data: &str) -> String {
    let mut out = String::with_capacity(data.len() + data.len() / 76 * 2);
    for chunk in data.as_bytes().chunks(76) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for text in ["", "a", "ab", "abc", "桌面信息岛", &"x".repeat(300)] {
            let enc = base64_encode(text.as_bytes());
            let dec = base64_decode(enc.as_bytes()).unwrap();
            assert_eq!(String::from_utf8(dec).unwrap(), text);
        }
    }

    #[test]
    fn utf8_subjects_are_decoded() {
        let raw = format!("=?utf-8?B?{}?=", base64_encode("周报提醒".as_bytes()));
        assert_eq!(decode_words(&raw), "周报提醒");
    }

    #[test]
    fn q_encoding_is_decoded() {
        // "caf=C3=A9" in Q encoding.
        assert_eq!(decode_words("=?utf-8?Q?caf=C3=A9?="), "café");
        assert_eq!(decode_words("=?utf-8?Q?a_b?="), "a b");
    }

    #[test]
    fn plain_text_survives_untouched() {
        assert_eq!(decode_words("普通主题"), "普通主题");
        assert_eq!(decode_words("Hello world"), "Hello world");
        assert_eq!(decode_words("=?bogus"), "=?bogus");
    }

    #[test]
    fn headers_unfold_and_match_case_insensitively() {
        let headers = "From: Alice <a@example.com>\r\nSubject: Hello\r\n World\r\nDate: now\r\n";
        assert_eq!(header_value(headers, "from").unwrap(), "Alice <a@example.com>");
        assert_eq!(header_value(headers, "Subject").unwrap(), "Hello World");
        assert!(header_value(headers, "Cc").is_none());
    }

    #[test]
    fn uid_and_literal_length_are_parsed() {
        let line = "* 1 FETCH (UID 142 BODY[HEADER.FIELDS (FROM SUBJECT DATE)] {245}";
        assert_eq!(extract_uid(line).as_deref(), Some("142"));
        assert_eq!(literal_len(line), Some(245));
        assert_eq!(literal_len("* 1 FETCH (UID 1)"), None);
    }

    #[test]
    fn a_message_has_the_required_headers_and_wrapping() {
        let msg = build_message("me@example.com", "", "提醒", &"内容".repeat(40));
        assert!(msg.contains("From: me@example.com"));
        assert!(msg.contains("To: me@example.com"));
        assert!(msg.contains("Content-Type: text/plain; charset=utf-8"));
        for line in msg.split("\r\n") {
            assert!(line.len() <= 78, "line too long: {line}");
        }
    }

    #[test]
    fn password_quotes_are_escaped_for_imap() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
