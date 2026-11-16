//! Minimal HTTPS JSON POST through WinHTTP.
//!
//! WinHTTP is used instead of a bundled TLS stack because it follows the
//! system proxy, certificate store and enterprise TLS inspection exactly like
//! the browser the user already configured. It also keeps the binary small.

use std::ffi::c_void;

type HInternet = *mut c_void;

const ACCESS_AUTOMATIC_PROXY: u32 = 4;
const ACCESS_NO_PROXY: u32 = 1;
const FLAG_SECURE: u32 = 0x0080_0000;
const QUERY_STATUS: u32 = 19;
const QUERY_NUMBER: u32 = 0x2000_0000;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

#[link(name = "winhttp")]
extern "system" {
    fn WinHttpOpen(
        agent: *const u16,
        access_type: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> HInternet;
    fn WinHttpConnect(session: HInternet, host: *const u16, port: u16, reserved: u32) -> HInternet;
    fn WinHttpOpenRequest(
        connect: HInternet,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept: *const *const u16,
        flags: u32,
    ) -> HInternet;
    fn WinHttpSendRequest(
        request: HInternet,
        headers: *const u16,
        headers_len: u32,
        optional: *mut c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(request: HInternet, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(
        request: HInternet,
        info_level: u32,
        name: *const u16,
        buffer: *mut c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpReadData(
        request: HInternet,
        buffer: *mut c_void,
        to_read: u32,
        read: *mut u32,
    ) -> i32;
    fn WinHttpSetTimeouts(
        handle: HInternet,
        resolve: i32,
        connect: i32,
        send: i32,
        receive: i32,
    ) -> i32;
    fn WinHttpCloseHandle(handle: HInternet) -> i32;
}

/// POST an already-serialized JSON body. `headers` are extra request headers
/// (`Authorization`, `x-api-key`, …); content type and accept are added here.
pub fn post_json(
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<(u16, String), String> {
    let (secure, host, port, path) = split_url(url)?;
    let mut header_text =
        String::from("Content-Type: application/json\r\nAccept: application/json\r\n");
    for (name, value) in headers.iter() {
        if value.is_empty() {
            continue;
        }
        header_text.push_str(name);
        header_text.push_str(": ");
        header_text.push_str(value);
        header_text.push_str("\r\n");
    }
    unsafe {
        let agent = dcx_win32::wide("dcx-snip/1.0");
        // A corporate proxy must not swallow a local Ollama / LM Studio /
        // LibreTranslate endpoint; loopback always goes direct.
        let access = if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
            ACCESS_NO_PROXY
        } else {
            ACCESS_AUTOMATIC_PROXY
        };
        let session = WinHttpOpen(
            agent.as_ptr(),
            access,
            core::ptr::null(),
            core::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHttpOpen 失败".to_string());
        }
        WinHttpSetTimeouts(session, 10_000, 10_000, 60_000, 180_000);
        let host_w = dcx_win32::wide(&host);
        let connect = WinHttpConnect(session, host_w.as_ptr(), port, 0);
        if connect.is_null() {
            WinHttpCloseHandle(session);
            return Err(format!("无法连接 {host}:{port}"));
        }
        let verb = dcx_win32::wide("POST");
        let object = dcx_win32::wide(&path);
        let request = WinHttpOpenRequest(
            connect,
            verb.as_ptr(),
            object.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            if secure { FLAG_SECURE } else { 0 },
        );
        if request.is_null() {
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err("创建 HTTP 请求失败".to_string());
        }
        let headers_w = dcx_win32::wide(&header_text);
        let mut body_bytes = body.as_bytes().to_vec();
        let sent = WinHttpSendRequest(
            request,
            headers_w.as_ptr(),
            u32::MAX,
            body_bytes.as_mut_ptr() as *mut c_void,
            body_bytes.len() as u32,
            body_bytes.len() as u32,
            0,
        );
        if sent == 0 {
            let err = std::io::Error::last_os_error();
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err(format!("发送请求失败：{err}"));
        }
        if WinHttpReceiveResponse(request, core::ptr::null_mut()) == 0 {
            let err = std::io::Error::last_os_error();
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err(format!("等待响应失败：{err}"));
        }
        let mut status: u32 = 0;
        let mut len = core::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request,
            QUERY_STATUS | QUERY_NUMBER,
            core::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            core::ptr::null_mut(),
        );
        let mut bytes = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        let mut read: u32 = 0;
        while WinHttpReadData(
            request,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut read,
        ) != 0
            && read > 0
        {
            bytes.extend_from_slice(&buf[..read as usize]);
            if bytes.len() >= MAX_RESPONSE {
                break;
            }
        }
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connect);
        WinHttpCloseHandle(session);
        // Decode once: per-chunk `from_utf8_lossy` can split a Chinese
        // character across the 16 KB boundary and produce a replacement glyph.
        Ok((status as u16, String::from_utf8_lossy(&bytes).to_string()))
    }
}

pub fn http_error(provider: &str, status: u16, body: &str) -> String {
    let snippet: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect();
    if snippet.is_empty() {
        format!("{provider} 返回 HTTP {status}")
    } else {
        format!("{provider} 返回 HTTP {status}：{snippet}")
    }
}

fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let url = url.trim();
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err("地址必须以 http:// 或 https:// 开头".to_string());
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let default_port = if secure { 443 } else { 80 };
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        // `[::1]` / `[::1]:5000`: the brackets belong to the URL, not to the
        // host name handed to WinHttpConnect.
        let (h, tail) = inner
            .split_once(']')
            .ok_or_else(|| "IPv6 地址缺少 ]".to_string())?;
        let port = if tail.is_empty() {
            default_port
        } else if let Some(p) = tail.strip_prefix(':') {
            p.parse::<u16>().map_err(|_| format!("端口无效：{p}"))?
        } else {
            return Err("IPv6 地址格式无效".to_string());
        };
        (h.to_string(), port)
    } else if authority.matches(':').count() > 1 {
        // A bare IPv6 literal is ambiguous with the port separator.
        return Err("IPv6 地址请写成 [::1] 或 [::1]:端口 的形式".to_string());
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        (
            h.to_string(),
            p.parse::<u16>().map_err(|_| format!("端口无效：{p}"))?,
        )
    } else {
        (authority.to_string(), default_port)
    };
    if host.is_empty() {
        return Err("地址缺少主机名".to_string());
    }
    // Bearer keys and screenshots must not cross a plaintext link to a remote
    // host; loopback and LAN endpoints (Ollama, LibreTranslate) stay allowed.
    if !secure && !local_network_host(&host) {
        return Err("明文 http:// 仅允许本机或局域网地址；远程服务请改用 https://".to_string());
    }
    Ok((secure, host, port, path.to_string()))
}

/// Loopback, RFC 1918 private, link-local and IPv6 unique-local literals.
fn local_network_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    if let Ok(v4) = h.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback() || v4.is_private() || v4.is_link_local();
    }
    if let Ok(v6) = h.parse::<std::net::Ipv6Addr>() {
        let head = v6.segments()[0];
        return v6.is_loopback()
            || (head & 0xfe00) == 0xfc00
            || (head & 0xffc0) == 0xfe80;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_https_with_port_and_query() {
        let (secure, host, port, path) = split_url("https://example.com:8443/v1/chat?q=1").unwrap();
        assert!(secure);
        assert_eq!(host, "example.com");
        assert_eq!(port, 8443);
        assert_eq!(path, "/v1/chat?q=1");
    }

    #[test]
    fn splits_plain_http_default_port() {
        let (secure, host, port, path) = split_url("http://127.0.0.1:5000/translate").unwrap();
        assert!(!secure);
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 5000);
        assert_eq!(path, "/translate");
    }

    #[test]
    fn rejects_missing_scheme() {
        assert!(split_url("api.example.com/v1").is_err());
    }

    #[test]
    fn splits_ipv6_literal_with_port() {
        let (secure, host, port, path) = split_url("http://[::1]:5000/translate").unwrap();
        assert!(!secure);
        assert_eq!(host, "::1");
        assert_eq!(port, 5000);
        assert_eq!(path, "/translate");
        let (_, host, port, _) = split_url("https://[fe80::1]/x").unwrap();
        assert_eq!(host, "fe80::1");
        assert_eq!(port, 443);
    }

    #[test]
    fn rejects_bare_ipv6_literal() {
        assert!(split_url("http://::1:5000/x").is_err());
    }

    #[test]
    fn plain_http_only_for_local_networks() {
        assert!(split_url("http://127.0.0.1:5000/x").is_ok());
        assert!(split_url("http://192.168.1.9/x").is_ok());
        assert!(split_url("http://localhost:8080/x").is_ok());
        assert!(split_url("http://[::1]:5000/x").is_ok());
        assert!(split_url("http://api.example.com/v1").is_err());
        assert!(split_url("https://api.example.com/v1").is_ok());
    }
}
