//! Thin Win32 layer for everything the island samples or stores.
//!
//! Same policy as the rest of the workspace: the handful of functions needed
//! are declared by hand instead of pulling in the `windows` crate for the whole
//! platform surface. `windows` is used only where WinRT makes hand-written
//! bindings uneconomical (see `notify::qq`).
//!
//! Structures that Win32 fills in (`MEMORYSTATUSEX`, `MIB_IF_ROW2`,
//! `CREDENTIALW`) are `#[repr(C)]` with every field in documented order. Their
//! sizes are asserted at runtime by [`sizes_are_sane`] — a wrong offset in
//! `MIB_IF_ROW2` would otherwise produce a plausible-looking but meaningless
//! network counter.

use core::ffi::c_void;

pub type Handle = isize;
pub type Bool = i32;

// ---------------------------------------------------------------------------
// CPU / memory / disk
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FileTime {
    pub low: u32,
    pub high: u32,
}

impl FileTime {
    pub fn as_u64(&self) -> u64 {
        ((self.high as u64) << 32) | self.low as u64
    }
}

#[repr(C)]
#[derive(Default)]
pub struct MemoryStatusEx {
    pub length: u32,
    pub memory_load: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page_file: u64,
    pub avail_page_file: u64,
    pub total_virtual: u64,
    pub avail_virtual: u64,
    pub avail_extended_virtual: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> Bool;
    fn GlobalMemoryStatusEx(out: *mut MemoryStatusEx) -> Bool;
    fn GetDiskFreeSpaceExW(
        dir: *const u16,
        free_bytes_available: *mut u64,
        total_bytes: *mut u64,
        total_free_bytes: *mut u64,
    ) -> Bool;
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(mem: Handle) -> *mut c_void;
    fn GlobalUnlock(mem: Handle) -> Bool;
    fn GlobalSize(mem: Handle) -> usize;
    fn GlobalFree(mem: Handle) -> Handle;
    fn GetTickCount64() -> u64;
    fn MessageBeep(kind: u32) -> Bool;
}

/// A raw CPU sample: idle and busy counters in 100 ns units.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuSample {
    pub idle: u64,
    pub busy: u64,
}

pub fn cpu_sample() -> Option<CpuSample> {
    let mut idle = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    // `kernel` includes idle time, so busy = kernel + user - idle.
    let k = kernel.as_u64();
    let u = user.as_u64();
    let i = idle.as_u64();
    Some(CpuSample {
        idle: i,
        busy: k.saturating_add(u).saturating_sub(i),
    })
}

/// Percentage from two samples. 0 when the counters did not move (a very short
/// interval, or an idle machine that rounded to the same value).
pub fn cpu_percent(prev: &CpuSample, now: &CpuSample) -> f32 {
    let idle = now.idle.saturating_sub(prev.idle) as f64;
    let busy = now.busy.saturating_sub(prev.busy) as f64;
    let total = idle + busy;
    if total <= 0.0 {
        0.0
    } else {
        ((busy / total) * 100.0).clamp(0.0, 100.0) as f32
    }
}

pub fn memory() -> Option<(u64, u64, u32)> {
    let mut st = MemoryStatusEx {
        length: core::mem::size_of::<MemoryStatusEx>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut st) } == 0 {
        return None;
    }
    Some((st.total_phys, st.avail_phys, st.memory_load))
}

pub fn disk(system_drive: &str) -> Option<(u64, u64)> {
    let dir = dcx_win32::wide(system_drive);
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    if unsafe { GetDiskFreeSpaceExW(dir.as_ptr(), &mut free, &mut total, &mut total_free) } == 0 {
        return None;
    }
    Some((total, total_free))
}

pub fn uptime_secs() -> u64 {
    (unsafe { GetTickCount64() }) / 1000
}

pub fn beep() {
    const MB_ICONASTERISK: u32 = 0x0000_0040;
    unsafe {
        MessageBeep(MB_ICONASTERISK);
    }
}

// ---------------------------------------------------------------------------
// Network — GetIfTable2 / MIB_IF_ROW2
// ---------------------------------------------------------------------------

const IF_MAX_STRING_SIZE: usize = 256;
const IF_MAX_PHYS_ADDRESS_LENGTH: usize = 32;

/// Full `MIB_IF_ROW2`. Every field is present, including the ones never read,
/// because the table is an **array**: a truncated struct would stride wrongly
/// and read counters from the middle of the next row.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MibIfRow2 {
    pub interface_luid: u64,
    pub interface_index: u32,
    pub interface_guid: [u8; 16],
    pub alias: [u16; IF_MAX_STRING_SIZE + 1],
    pub description: [u16; IF_MAX_STRING_SIZE + 1],
    pub physical_address_length: u32,
    pub physical_address: [u8; IF_MAX_PHYS_ADDRESS_LENGTH],
    pub permanent_physical_address: [u8; IF_MAX_PHYS_ADDRESS_LENGTH],
    pub mtu: u32,
    pub if_type: u32,
    pub tunnel_type: u32,
    pub media_type: u32,
    pub physical_medium_type: u32,
    pub access_type: u32,
    pub direction_type: u32,
    pub flags: u8,
    pub oper_status: u32,
    pub admin_status: u32,
    pub media_connect_state: u32,
    pub network_guid: [u8; 16],
    pub connection_type: u32,
    pub transmit_link_speed: u64,
    pub receive_link_speed: u64,
    pub in_octets: u64,
    pub out_octets: u64,
    pub in_ucast_pkts: u64,
    pub out_ucast_pkts: u64,
    pub in_nucast_pkts: u64,
    pub out_nucast_pkts: u64,
    pub in_discards: u64,
    pub out_discards: u64,
    pub in_errors: u64,
    pub out_errors: u64,
    pub in_unknown_protos: u64,
    pub in_ucast_octets: u64,
    pub out_ucast_octets: u64,
    pub in_multicast_octets: u64,
    pub out_multicast_octets: u64,
    pub in_broadcast_octets: u64,
    pub out_broadcast_octets: u64,
    pub out_qlen: u64,
}

#[repr(C)]
pub struct MibIfTable2 {
    pub num_entries: u32,
    /// Padding the compiler inserts before the array (the real struct has the
    /// array inline; keeping it as a pointer would read the wrong address).
    _pad: u32,
    pub table: [MibIfRow2; 1],
}

#[link(name = "iphlpapi")]
extern "system" {
    fn GetIfTable2(table: *mut *mut MibIfTable2) -> u32;
    fn FreeMibTable(mem: *mut c_void);
}

/// Interface type for Ethernet; 71 is IEEE 802.11 (Wi-Fi). Both are physical.
const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_OPER_STATUS_UP: u32 = 1;

/// Sum of received / sent octets over physical interfaces that are up.
///
/// Loopback, tunnels and pseudo-interfaces are excluded: they double-count
/// local traffic and make the readout meaningless. A "physical" check on
/// `if_type` is more reliable than the filter flags, which report false for
/// some Wi-Fi drivers.
pub fn net_octets() -> Option<(u64, u64)> {
    let mut table: *mut MibIfTable2 = core::ptr::null_mut();
    let rc = unsafe { GetIfTable2(&mut table) };
    if rc != 0 || table.is_null() {
        return None;
    }
    let mut in_sum = 0u64;
    let mut out_sum = 0u64;
    unsafe {
        let count = (*table).num_entries as usize;
        let base = core::ptr::addr_of!((*table).table) as *const MibIfRow2;
        for i in 0..count.min(512) {
            let row = &*base.add(i);
            if row.oper_status != IF_OPER_STATUS_UP {
                continue;
            }
            if !matches!(row.if_type, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211) {
                continue;
            }
            in_sum = in_sum.saturating_add(row.in_octets);
            out_sum = out_sum.saturating_add(row.out_octets);
        }
        FreeMibTable(table as *mut c_void);
    }
    Some((in_sum, out_sum))
}

/// Names of the interfaces the sums above cover, for the diagnostics card.
pub fn net_interface_names() -> Vec<String> {
    let mut table: *mut MibIfTable2 = core::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    unsafe {
        let count = (*table).num_entries as usize;
        let base = core::ptr::addr_of!((*table).table) as *const MibIfRow2;
        for i in 0..count.min(512) {
            let row = &*base.add(i);
            if row.oper_status != IF_OPER_STATUS_UP
                || !matches!(row.if_type, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211)
            {
                continue;
            }
            let end = row.alias.iter().position(|c| *c == 0).unwrap_or(0);
            out.push(String::from_utf16_lossy(&row.alias[..end]));
        }
        FreeMibTable(table as *mut c_void);
    }
    out
}

/// Struct sizes that Win32 writes into. A mismatch means a wrong layout, and
/// every derived number would be garbage rather than an error — so it is
/// checked once at startup and reported instead of silently trusted.
pub fn sizes_are_sane() -> Vec<String> {
    let mut problems = Vec::new();
    let mib = core::mem::size_of::<MibIfRow2>();
    if mib != 1352 {
        problems.push(format!("MIB_IF_ROW2 = {mib} bytes, expected 1352"));
    }
    let mem = core::mem::size_of::<MemoryStatusEx>();
    if mem != 64 {
        problems.push(format!("MEMORYSTATUSEX = {mem} bytes, expected 64"));
    }
    problems
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_DIB: u32 = 8;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_HDROP: u32 = 15;

const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(owner: isize) -> Bool;
    fn CloseClipboard() -> Bool;
    fn EmptyClipboard() -> Bool;
    fn GetClipboardData(format: u32) -> Handle;
    fn SetClipboardData(format: u32, mem: Handle) -> Handle;
    fn IsClipboardFormatAvailable(format: u32) -> Bool;
    fn EnumClipboardFormats(format: u32) -> u32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
    fn GetClipboardSequenceNumber() -> u32;
    fn AddClipboardFormatListener(hwnd: isize) -> Bool;
    fn RemoveClipboardFormatListener(hwnd: isize) -> Bool;
    fn GetClipboardOwner() -> isize;
}

pub fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn clipboard_listener(hwnd: isize, on: bool) -> bool {
    unsafe {
        if on {
            AddClipboardFormatListener(hwnd) != 0
        } else {
            RemoveClipboardFormatListener(hwnd) != 0
        }
    }
}

pub fn clipboard_owner() -> isize {
    unsafe { GetClipboardOwner() }
}

pub fn clipboard_has(format: u32) -> bool {
    (unsafe { IsClipboardFormatAvailable(format) }) != 0
}

pub fn clipboard_formats() -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        if OpenClipboard(0) == 0 {
            return out;
        }
        let mut fmt = 0u32;
        loop {
            fmt = EnumClipboardFormats(fmt);
            if fmt == 0 {
                break;
            }
            out.push(fmt);
            if out.len() > 128 {
                break;
            }
        }
        CloseClipboard();
    }
    out
}

/// Copy one clipboard format into a byte vector. `None` when the clipboard is
/// held by another process (common while a browser is copying a large image) —
/// callers retry instead of treating it as "no content".
pub fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    for attempt in 0..8 {
        unsafe {
            if OpenClipboard(0) == 0 {
                std::thread::sleep(std::time::Duration::from_millis(8 * (attempt + 1)));
                continue;
            }
            let handle = GetClipboardData(format);
            if handle == 0 {
                CloseClipboard();
                return None;
            }
            let size = GlobalSize(handle);
            if size == 0 {
                CloseClipboard();
                return None;
            }
            let ptr = GlobalLock(handle) as *const u8;
            if ptr.is_null() {
                CloseClipboard();
                return None;
            }
            let bytes = core::slice::from_raw_parts(ptr, size).to_vec();
            GlobalUnlock(handle);
            CloseClipboard();
            return Some(bytes);
        }
    }
    None
}

pub fn clipboard_text() -> Option<String> {
    let bytes = clipboard_bytes(CF_UNICODETEXT)?;
    if bytes.len() < 2 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Publish text as `CF_UNICODETEXT`. `EmptyClipboard` is required by the API
/// and also clears the previous owner's data, so it is done in one open/close
/// cycle.
pub fn set_clipboard_text(text: &str) -> bool {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    unsafe {
        if OpenClipboard(0) == 0 {
            return false;
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return false;
        }
        let bytes = units.len() * 2;
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem == 0 {
            CloseClipboard();
            return false;
        }
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return false;
        }
        core::ptr::copy_nonoverlapping(units.as_ptr() as *const u8, dst, bytes);
        GlobalUnlock(mem);
        if SetClipboardData(CF_UNICODETEXT, mem) == 0 {
            GlobalFree(mem);
            CloseClipboard();
            return false;
        }
        // Ownership transferred to the system; do not free.
        CloseClipboard();
        true
    }
}

/// Publish a DIB so the image can be pasted into any application. The DIB
/// header is passed through unchanged — it already came from the clipboard.
pub fn set_clipboard_dib(dib: &[u8]) -> bool {
    if dib.len() < 40 {
        return false;
    }
    unsafe {
        if OpenClipboard(0) == 0 {
            return false;
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return false;
        }
        let mem = GlobalAlloc(GMEM_MOVEABLE, dib.len());
        if mem == 0 {
            CloseClipboard();
            return false;
        }
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return false;
        }
        core::ptr::copy_nonoverlapping(dib.as_ptr(), dst, dib.len());
        GlobalUnlock(mem);
        if SetClipboardData(CF_DIB, mem) == 0 {
            GlobalFree(mem);
            CloseClipboard();
            return false;
        }
        CloseClipboard();
        true
    }
}

pub fn png_format() -> u32 {
    let name = dcx_win32::wide("PNG");
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

/// Register a named clipboard format from an already wide string.
pub fn register_clipboard_format(name: &[u16]) -> u32 {
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

// ---------------------------------------------------------------------------
// Credentials — IMAP password and WeCom webhook key never touch a TOML
// ---------------------------------------------------------------------------

// The FFI lives in `dcx-island-config::credential`: the host's settings
// window writes these values and this process reads them. Two independent
// copies of the target-name format would silently diverge.
pub use dcx_island_config::credential::read as cred_read;

// ---------------------------------------------------------------------------
// WinHTTP — WeCom group-robot webhook
// ---------------------------------------------------------------------------

type HInternet = *mut c_void;

const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

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
    ) -> Bool;
    fn WinHttpReceiveResponse(request: HInternet, reserved: *mut c_void) -> Bool;
    fn WinHttpQueryHeaders(
        request: HInternet,
        info_level: u32,
        name: *const u16,
        buffer: *mut c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> Bool;
    fn WinHttpReadData(
        request: HInternet,
        buffer: *mut c_void,
        to_read: u32,
        read: *mut u32,
    ) -> Bool;
    fn WinHttpSetTimeouts(
        handle: HInternet,
        resolve: i32,
        connect: i32,
        send: i32,
        receive: i32,
    ) -> Bool;
    fn WinHttpCloseHandle(handle: HInternet) -> Bool;
}

/// Minimal HTTPS POST. Returns the HTTP status and up to 4 KB of the body.
///
/// WinHTTP is used instead of an HTTP crate because it shares the system proxy
/// configuration and certificate store: an offline machine, a corporate proxy
/// or a TLS-inspecting antivirus all behave exactly as they do in a browser.
pub fn https_post(url: &str, content_type: &str, body: &str) -> Result<(u32, String), String> {
    let (secure, host, port, path) = split_url(url)?;
    unsafe {
        let agent = dcx_win32::wide("dcx-island/1.0");
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            core::ptr::null(),
            core::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHttpOpen 失败".to_string());
        }
        WinHttpSetTimeouts(session, 5000, 5000, 8000, 8000);
        let host_w = dcx_win32::wide(&host);
        let connect = WinHttpConnect(session, host_w.as_ptr(), port, 0);
        if connect.is_null() {
            WinHttpCloseHandle(session);
            return Err(format!("无法连接 {host}:{port}"));
        }
        let verb = dcx_win32::wide("POST");
        let object = dcx_win32::wide(&path);
        let flags = if secure { WINHTTP_FLAG_SECURE } else { 0 };
        let request = WinHttpOpenRequest(
            connect,
            verb.as_ptr(),
            object.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            flags,
        );
        if request.is_null() {
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err("WinHttpOpenRequest 失败".to_string());
        }
        let headers = dcx_win32::wide(&format!(
            "Content-Type: {content_type}\r\nAccept: application/json\r\n"
        ));
        let mut body_bytes = body.as_bytes().to_vec();
        let sent = WinHttpSendRequest(
            request,
            headers.as_ptr(),
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
            return Err(format!("发送失败：{err}"));
        }
        if WinHttpReceiveResponse(request, core::ptr::null_mut()) == 0 {
            let err = std::io::Error::last_os_error();
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err(format!("等待响应失败：{err}"));
        }
        let mut code: u32 = 0;
        let mut len = core::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            core::ptr::null(),
            &mut code as *mut u32 as *mut c_void,
            &mut len,
            core::ptr::null_mut(),
        );
        let mut text = String::new();
        let mut buf = vec![0u8; 4096];
        let mut read: u32 = 0;
        while WinHttpReadData(
            request,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut read,
        ) != 0
            && read > 0
            && text.len() < 8192
        {
            text.push_str(&String::from_utf8_lossy(&buf[..read as usize]));
        }
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connect);
        WinHttpCloseHandle(session);
        Ok((code, text))
    }
}

fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("不支持的地址：{url}"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>().map_err(|_| format!("端口无效：{p}"))?,
        ),
        None => (authority.to_string(), if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err("地址缺少主机名".to_string());
    }
    Ok((secure, host, port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_win32_abi() {
        assert_eq!(core::mem::size_of::<MibIfRow2>(), 1352);
        assert_eq!(core::mem::size_of::<MemoryStatusEx>(), 64);
        assert!(sizes_are_sane().is_empty());
    }

    #[test]
    fn cpu_percent_handles_an_unchanged_counter() {
        let a = CpuSample {
            idle: 10,
            busy: 10,
        };
        assert_eq!(cpu_percent(&a, &a), 0.0);
        // `CpuSample.busy` is already the non-idle part of kernel+user, so the
        // denominator is Δidle + Δbusy, not Δbusy alone.
        let all_busy = CpuSample {
            idle: 10,
            busy: 30,
        };
        assert!((cpu_percent(&a, &all_busy) - 100.0).abs() < 0.01);
        let mixed = CpuSample {
            idle: 20,
            busy: 30,
        };
        assert!((cpu_percent(&a, &mixed) - 66.666).abs() < 0.01);
    }

    #[test]
    fn urls_split_into_host_port_and_path() {
        assert_eq!(
            split_url("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=x").unwrap(),
            (
                true,
                "qyapi.weixin.qq.com".to_string(),
                443,
                "/cgi-bin/webhook/send?key=x".to_string()
            )
        );
        assert_eq!(
            split_url("http://127.0.0.1:8080/a").unwrap(),
            (false, "127.0.0.1".to_string(), 8080, "/a".to_string())
        );
        assert!(split_url("ftp://x").is_err());
    }

    #[test]
    fn live_samples_are_in_range() {
        // These read the real machine; they must not panic or return nonsense.
        if let Some(mem) = memory() {
            assert!(mem.0 > 0);
            assert!(mem.2 <= 100);
        }
        if let Some((total, _free)) = disk("C:\\") {
            assert!(total > 0);
        }
        let a = cpu_sample().expect("GetSystemTimes");
        std::thread::sleep(std::time::Duration::from_millis(60));
        let b = cpu_sample().expect("GetSystemTimes");
        let pct = cpu_percent(&a, &b);
        assert!((0.0..=100.0).contains(&pct));
    }
}
