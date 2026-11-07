//! Global hotkeys for the desk-box, file-search and info-island apps.
//!
//! The hotkeys must be owned by the **resident platform**, not by the apps:
//! `apps.toml [apps.search].enabled` defaults to false, and even the box can be
//! started on demand. A dedicated thread here creates a `HWND_MESSAGE` window,
//! registers all three hotkeys against that window and runs its own message loop.
//!
//! `RegisterHotKey(NULL, ...)` would post `WM_HOTKEY` to the *thread queue*,
//! which the host's `Platform::run()` pump is not guaranteed to dispatch into
//! our code. Binding to a real window makes delivery deterministic.
//!
//! On `WM_HOTKEY` we (1) `launch_on_demand(app)` and (2) touch the app's show
//! marker (`%LOCALAPPDATA%\dcx\box_show` / `search_show` / `island_show`). The apps poll those
//! markers and raise/toggle their window — the platform's existing "file is
//! IPC" convention, with no winit window-procedure subclassing.
//!
//! Known limits: UIPI means a hotkey is not delivered while a higher
//! integrity-level window is foreground; if `dcx-host` is not running there is
//! no listener at all. Both are surfaced in the settings centre.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const WM_HOTKEY: u32 = 0x0312;
const WM_TIMER: u32 = 0x0113;
const TIMER_ID: usize = 1;
const BOX_HOTKEY_ID: i32 = 1;
const SEARCH_HOTKEY_ID: i32 = 2;
const ISLAND_HOTKEY_ID: i32 = 3;
const HWND_MESSAGE: isize = -3;
const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000;

const BOX_ERROR_FILE: &str = "box_hotkey_error.txt";
const SEARCH_ERROR_FILE: &str = "search_hotkey_error.txt";
const ISLAND_ERROR_FILE: &str = "island_hotkey_error.txt";

static BOUND_BOX: Mutex<Option<(String, bool)>> = Mutex::new(None);
static BOUND_SEARCH: Mutex<Option<(String, bool)>> = Mutex::new(None);
static BOUND_ISLAND: Mutex<Option<(String, bool)>> = Mutex::new(None);
/// Last message written per error file, so the retry loop does not rewrite an
/// unchanged error every second.
static LAST_ERROR: Mutex<Option<(String, String)>> = Mutex::new(None);

#[repr(C)]
struct WndClassW {
    style: u32,
    lpfn_wnd_proc: usize,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: isize,
    h_icon: isize,
    h_cursor: isize,
    hbr_background: isize,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[repr(C)]
#[derive(Default)]
struct Msg {
    hwnd: isize,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
    l_private: u32,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        param: *mut c_void,
    ) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
    fn SetTimer(hwnd: isize, id: usize, elapse: u32, proc: *const c_void) -> usize;
    fn KillTimer(hwnd: isize, id: usize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
}

fn local_dir() -> PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("dcx");
    let _ = std::fs::create_dir_all(&p);
    p
}

fn error_path(file: &str) -> PathBuf {
    local_dir().join(file)
}

fn read_error(file: &str) -> Option<String> {
    std::fs::read_to_string(error_path(file))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read by the settings centre to show "快捷键被占用，请改键".
pub fn last_error() -> Option<String> {
    read_error(SEARCH_ERROR_FILE)
}

pub fn last_box_error() -> Option<String> {
    read_error(BOX_ERROR_FILE)
}

pub fn last_island_error() -> Option<String> {
    read_error(ISLAND_ERROR_FILE)
}

fn note_error(file: &str, text: &str) {
    // The 1 s retry loop calls this every tick while a key is taken. Writing
    // the file (and the log line) only on a change keeps the disk quiet and
    // the log readable.
    if let Ok(mut slot) = LAST_ERROR.lock() {
        if slot
            .as_ref()
            .map(|(f, t)| f == file && t == text)
            .unwrap_or(false)
        {
            return;
        }
        *slot = Some((file.to_string(), text.to_string()));
    }
    let _ = std::fs::write(error_path(file), text);
    println!("[dcx-hotkey] {text}");
}

fn clear_error(file: &str) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        if slot.as_ref().map(|(f, _)| f == file).unwrap_or(false) {
            *slot = None;
        }
    }
    let _ = std::fs::remove_file(error_path(file));
}

/// Only the activation keys the host cares about; the rest of each TOML is left
/// untouched (the host edits a `toml::Value`, not a partial struct).
fn hotkey_setting(file: &str, table: Option<&str>, default_key: &str) -> (String, bool) {
    let text = std::fs::read_to_string(local_dir().join(file)).unwrap_or_default();
    let doc: toml::Value = text
        .parse()
        .unwrap_or_else(|_| toml::Value::Table(Default::default()));
    let node = match table {
        Some(t) => doc.get(t),
        None => Some(&doc),
    };
    let key = node
        .and_then(|n| n.get("hotkey"))
        .and_then(|v| v.as_str())
        .unwrap_or(default_key)
        .to_string();
    let on = node
        .and_then(|n| n.get("hotkey_enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    (key, on)
}

fn search_hotkey_setting() -> (String, bool) {
    hotkey_setting("search.toml", Some("activation"), "Alt+s")
}

fn box_hotkey_setting() -> (String, bool) {
    hotkey_setting("organizer.toml", None, "Alt+d")
}

fn island_hotkey_setting() -> (String, bool) {
    hotkey_setting("island.toml", None, "Alt+i")
}

/// The island's master switch also lives in `island.toml` (the settings centre
/// writes it together with `apps.toml`). A hotkey must not resurrect a
/// permanently topmost window the user explicitly turned off.
fn island_enabled() -> bool {
    let text = std::fs::read_to_string(local_dir().join("island.toml")).unwrap_or_default();
    let doc: toml::Value = text
        .parse()
        .unwrap_or_else(|_| toml::Value::Table(Default::default()));
    doc.get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// `"Alt+s"` / `"Ctrl+Shift+F"` → (modifiers, virtual key).
///
/// A global hotkey **must** carry a modifier and exactly one base key. The old
/// parser accepted `"s"` and bound `RegisterHotKey(hwnd, id, 0x4000, 'S')`,
/// which takes the S key away from every program on the machine until the
/// setting is changed — a system-wide keyboard hijack triggered by a typo.
fn parse_hotkey(s: &str) -> Result<(u32, u32), String> {
    let mut mods = 0u32;
    let mut vk: Option<u32> = None;
    let mut parts = 0usize;
    for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        parts += 1;
        match part.to_ascii_lowercase().as_str() {
            "alt" => mods |= MOD_ALT,
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" => mods |= MOD_WIN,
            "space" => set_key(&mut vk, 0x20, part, s)?,
            "enter" => set_key(&mut vk, 0x0D, part, s)?,
            "tab" => set_key(&mut vk, 0x09, part, s)?,
            "esc" | "escape" => set_key(&mut vk, 0x1B, part, s)?,
            other => {
                let upper = other.to_ascii_uppercase();
                let code = if upper.len() == 1 {
                    let c = upper.as_bytes()[0];
                    if c.is_ascii_alphanumeric() {
                        Some(c as u32)
                    } else {
                        None
                    }
                } else if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                    if (1..=24).contains(&n) {
                        Some(0x70 + n - 1)
                    } else {
                        None
                    }
                } else {
                    None
                };
                match code {
                    Some(code) => set_key(&mut vk, code, part, s)?,
                    // Silently ignoring an unknown token turned `"Alt+Ctrl"` or
                    // a typo into a bare-key registration.
                    None => return Err(format!("快捷键 {s} 中的「{part}」无法识别")),
                }
            }
        }
    }
    if parts == 0 {
        return Err("快捷键为空，示例：Alt+d".to_string());
    }
    let vk = vk.ok_or_else(|| format!("快捷键 {s} 缺少主键，示例：Alt+d"))?;
    if mods & (MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN) == 0 {
        return Err(format!(
            "快捷键 {s} 必须包含 Alt / Ctrl / Shift / Win 之一，否则会独占该按键"
        ));
    }
    Ok((mods | MOD_NOREPEAT, vk))
}

fn set_key(slot: &mut Option<u32>, code: u32, part: &str, whole: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("快捷键 {whole} 包含多个主键（重复的是「{part}」）"));
    }
    *slot = Some(code);
    Ok(())
}

/// Check a user-entered hotkey without registering it. Used by the settings
/// window so an invalid combination is explained at the field, not only after
/// it silently failed to register.
pub fn validate(key: &str) -> Result<(), String> {
    parse_hotkey(key).map(|_| ())
}

fn touch_show_marker(name: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();
    let _ = std::fs::write(local_dir().join(name), now);
}

fn bind(
    hwnd: isize,
    id: i32,
    bound: &Mutex<Option<(String, bool)>>,
    key: String,
    enabled: bool,
    error_file: &str,
) {
    let mut state = bound.lock().unwrap_or_else(|e| e.into_inner());
    let already = state
        .as_ref()
        .map(|(k, on)| *on == enabled && (k == &key || !enabled))
        .unwrap_or(false);
    if already {
        return;
    }
    unsafe { UnregisterHotKey(hwnd, id) };
    if !enabled {
        clear_error(error_file);
        *state = Some((key, false));
        return;
    }
    match parse_hotkey(&key) {
        Ok((mods, vk)) => {
            if unsafe { RegisterHotKey(hwnd, id, mods, vk) } != 0 {
                clear_error(error_file);
                println!("[dcx-hotkey] registered {key}");
                *state = Some((key, true));
            } else {
                // Do **not** record the key as bound. The 1 s timer retries,
                // so a hotkey that was taken at startup starts working as soon
                // as the other program releases it — the old code stored the
                // key unconditionally and the retry then short-circuited,
                // leaving the hotkey dead for the rest of the session.
                *state = None;
                note_error(
                    error_file,
                    &format!("快捷键 {key} 已被其他程序占用，请在设置中心改键"),
                );
            }
        }
        Err(why) => {
            *state = None;
            note_error(error_file, &why);
        }
    }
}

fn rebind(hwnd: isize) {
    let (search_key, search_on) = search_hotkey_setting();
    bind(
        hwnd,
        SEARCH_HOTKEY_ID,
        &BOUND_SEARCH,
        search_key,
        search_on,
        SEARCH_ERROR_FILE,
    );
    let (box_key, box_on) = box_hotkey_setting();
    bind(
        hwnd,
        BOX_HOTKEY_ID,
        &BOUND_BOX,
        box_key,
        box_on,
        BOX_ERROR_FILE,
    );
    let (island_key, island_on) = island_hotkey_setting();
    bind(
        hwnd,
        ISLAND_HOTKEY_ID,
        &BOUND_ISLAND,
        island_key,
        island_on,
        ISLAND_ERROR_FILE,
    );
}

fn fire(id: i32) {
    match id {
        BOX_HOTKEY_ID => {
            let (_, enabled) = box_hotkey_setting();
            // The master switch really means "no box": a disabled app must not
            // be resurrected from the keyboard.
            if enabled && crate::apps::is_enabled("box") {
                if let Err(e) = crate::apps::launch_on_demand("box") {
                    note_error(BOX_ERROR_FILE, &e);
                }
                touch_show_marker("box_show");
            }
        }
        SEARCH_HOTKEY_ID => {
            let (_, enabled) = search_hotkey_setting();
            if enabled {
                // Start first, then signal. A running instance sees the marker
                // change and raises its window; a fresh instance shows itself
                // on its first frame anyway.
                if let Err(e) = crate::apps::launch_on_demand("search") {
                    note_error(SEARCH_ERROR_FILE, &e);
                }
                touch_show_marker("search_show");
            }
        }
        ISLAND_HOTKEY_ID => {
            let (_, hk_on) = island_hotkey_setting();
            // Two independent switches must agree: the island's own master
            // switch and the host's registry flag. Unlike file search, this app
            // is an always-visible topmost window — an "off" state must really
            // mean no window, even if a stale apps.toml says otherwise.
            if hk_on && island_enabled() && crate::apps::is_enabled("island") {
                if let Err(e) = crate::apps::launch_on_demand("island") {
                    note_error(ISLAND_ERROR_FILE, &e);
                }
                touch_show_marker("island_show");
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_HOTKEY => {
            // Never do IO or `CreateProcess` here. This window procedure is the
            // only message pump for the hotkey thread: a slow spawn would delay
            // the 1 s rebind timer and every later hotkey press.
            let id = wparam as i32;
            std::thread::spawn(move || fire(id));
            0
        }
        WM_TIMER if wparam == TIMER_ID => {
            // 1 s poll of all documents so editing a hotkey in the settings
            // centre takes effect without restarting the platform.
            rebind(hwnd);
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Start the hotkey thread. Never blocks the platform; registration failures
/// are reported to the settings centre instead of aborting startup.
pub fn start() {
    std::thread::spawn(|| {
        let class_name = dcx_win32::wide("DCX.Hotkey.Window");
        let window_name = dcx_win32::wide("DCX hotkey sink");
        let cls = WndClassW {
            style: 0,
            lpfn_wnd_proc: wnd_proc as *const () as usize,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: unsafe { GetModuleHandleW(core::ptr::null()) },
            h_icon: 0,
            h_cursor: 0,
            hbr_background: 0,
            lpsz_menu_name: core::ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
        };
        let atom = unsafe { RegisterClassW(&cls) };
        if atom == 0 {
            note_error(SEARCH_ERROR_FILE, "无法注册热键窗口类，全局快捷键不可用");
            return;
        }
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                window_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                0,
                0,
                core::ptr::null_mut(),
            )
        };
        if hwnd == 0 {
            note_error(SEARCH_ERROR_FILE, "无法创建热键隐藏窗口，全局快捷键不可用");
            return;
        }
        unsafe {
            SetTimer(hwnd, TIMER_ID, 1000, core::ptr::null());
        }
        rebind(hwnd);

        let mut msg = Msg::default();
        loop {
            let r = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
            if r <= 0 {
                break;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        unsafe {
            UnregisterHotKey(hwnd, BOX_HOTKEY_ID);
            UnregisterHotKey(hwnd, SEARCH_HOTKEY_ID);
            KillTimer(hwnd, TIMER_ID);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(s: &str) -> (u32, u32) {
        parse_hotkey(s).unwrap_or_else(|e| panic!("{s} should parse: {e}"))
    }

    #[test]
    fn defaults_parse() {
        assert_eq!(parsed("Alt+d").1, 'D' as u32);
        assert_eq!(parsed("ctrl+shift+f").1, 'F' as u32);
        assert_eq!(parsed("win+F12").1, 0x7B);
        assert_eq!(parsed("Alt+space").1, 0x20);
    }

    #[test]
    fn a_modifier_is_mandatory() {
        // `RegisterHotKey(hwnd, id, MOD_NOREPEAT, 'S')` takes the S key away
        // from every program on the machine; a typo must not do that.
        for bare in ["s", "S", "F5", "space", "5"] {
            assert!(parse_hotkey(bare).is_err(), "{bare} must be refused");
        }
    }

    #[test]
    fn exactly_one_base_key() {
        assert!(parse_hotkey("Alt+s+t").is_err());
        assert!(parse_hotkey("Alt+Ctrl").is_err());
        assert!(parse_hotkey("").is_err());
    }

    #[test]
    fn unknown_tokens_are_refused_instead_of_ignored() {
        assert!(parse_hotkey("Alt+Hyper+s").is_err());
        assert!(parse_hotkey("Alt+F25").is_err());
    }

    #[test]
    fn norepeat_is_always_set() {
        assert_ne!(parsed("Alt+d").0 & MOD_NOREPEAT, 0);
    }
}
