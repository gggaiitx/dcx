//! Toast mirror through the WinRT `UserNotificationListener` — QQ-only by
//! default, every app when `[inbox] system` is on.
//!
//! This is the only part of the island that uses the `windows` crate: the WinRT
//! ABI for `IAsyncOperation`, `IVectorView` and the notification objects is not
//! something to hand-write. Everything else stays on the hand-declared FFI.
//!
//! # Why it may not work, and what the UI says
//!
//! `UserNotificationListener::RequestAccessAsync` returns one of:
//!
//! * `Allowed` — polling works;
//! * `Denied` — the user turned off notification access in Settings;
//! * `Unspecified` — **the common case for an unpackaged desktop binary**.
//!   Windows requires a registered AppUserModelId before it can attribute the
//!   permission, and a plain `.exe` has none until it appears in the Start menu
//!   or is launched through a shortcut with an AUMID.
//!
//! The channel therefore reports the status it actually got instead of retrying
//! forever or claiming to work. Nothing here is on the critical path of the
//! island: no permission, no rows, no error dialog.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};
use windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
};

use crate::inbox::ChannelState;

#[derive(Clone, Debug)]
pub struct QqHit {
    pub id: u32,
    pub title: String,
    pub body: String,
    /// Display name of the raising app (`AppInfo.DisplayInfo.DisplayName`,
    /// falling back to the AUMID's last meaningful segment).
    pub app: String,
    /// The AUMID classified as QQ. Routing decision is the caller's: QQ hits
    /// go to the QQ channel only while that switch is on, everything else is
    /// a `System` row.
    pub is_qq: bool,
}

pub struct QqProbe {
    pub rx: Receiver<QqHit>,
    pub state: Arc<Mutex<ChannelState>>,
    /// Which switches were on when the thread started; the notifier is
    /// rebuilt whenever either changes, so these stay accurate.
    pub qq_enabled: bool,
    pub system_enabled: bool,
    stop: Arc<AtomicBool>,
}

impl Drop for QqProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

static SINK: Mutex<Option<Sender<QqHit>>> = Mutex::new(None);
static STATE: Mutex<Option<Arc<Mutex<ChannelState>>>> = Mutex::new(None);

/// Un-initialise WinRT on the same thread that initialised it, on every exit
/// path of the channel thread (including early returns).
struct RoApartment;
impl Drop for RoApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

pub fn start(qq_enabled: bool, system: bool) -> Option<QqProbe> {
    let (tx, rx) = channel();
    if let Ok(mut slot) = SINK.lock() {
        *slot = Some(tx);
    }
    let state = Arc::new(Mutex::new(ChannelState::Idle));
    if let Ok(mut slot) = STATE.lock() {
        *slot = Some(Arc::clone(&state));
    }
    let state2 = Arc::clone(&state);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);

    std::thread::Builder::new()
        .name("dcx-island-toast".to_string())
        .spawn(move || {
            // S_FALSE / RPC_E_CHANGED_MODE are both non-fatal: COM may already
            // be initialised on this thread, or in STA. WinRT activation still
            // works from a desktop process; report the real failure later via
            // the listener calls instead of aborting here.
            let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            let _apartment = RoApartment;

            let listener = match UserNotificationListener::Current() {
                Ok(l) => l,
                Err(e) => {
                    set(&state2, ChannelState::Unavailable(format!("无法访问通知服务：{e}")));
                    return;
                }
            };
            // `GetAccessStatus()` is a synchronous read. `RequestAccessAsync()`
            // may pop a consent dialog and block this thread indefinitely on an
            // unattended desktop; fly-island uses the read-only form too. An
            // unpackaged exe normally gets `Unspecified` and the channel simply
            // reports "not available" instead of hanging a thread forever.
            let status = match listener.GetAccessStatus() {
                Ok(status) => status,
                Err(e) => {
                    set(&state2, ChannelState::Error(format!("读取通知权限失败：{e}")));
                    return;
                }
            };
            match status {
                UserNotificationListenerAccessStatus::Allowed => {
                    set(&state2, ChannelState::Ok);
                }
                UserNotificationListenerAccessStatus::Denied => {
                    set(
                        &state2,
                        ChannelState::Denied("通知访问被拒绝，请在系统设置中允许".into()),
                    );
                    return;
                }
                _ => {
                    set(
                        &state2,
                        ChannelState::Unavailable(
                            if system {
                                "系统未授权通知访问（桌面程序常见），通知渠道不可用".into()
                            } else {
                                "系统未授权通知访问（桌面程序常见），QQ 渠道不可用".into()
                            },
                        ),
                    );
                    return;
                }
            }

            let mut seen: HashSet<u32> = HashSet::new();
            while !stop2.load(Ordering::SeqCst) {
                // 15 s: the notification store is cheap to read but this is a
                // cross-process WinRT call, not something to do per frame.
                for _ in 0..30 {
                    if stop2.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                match listener.GetNotificationsAsync(NotificationKinds::Toast) {
                    Ok(op) => match op.join() {
                        Ok(list) => {
                            let count = list.Size().unwrap_or(0);
                            for i in 0..count.min(64) {
                                let Ok(n) = list.GetAt(i) else { continue };
                                let Ok(app) = n.AppInfo() else { continue };
                                let app_id = app
                                    .AppUserModelId()
                                    .map(|s| s.to_string())
                                    .unwrap_or_default();
                                let is_qq = classify(&app_id);
                                // System mode widens the filter to every app;
                                // with only QQ on, the old behaviour is exact.
                                if !(system || (qq_enabled && is_qq)) {
                                    continue;
                                }
                                let Ok(id) = n.Id() else { continue };
                                if !seen.insert(id) {
                                    continue;
                                }
                                if seen.len() > 512 {
                                    seen.clear();
                                    seen.insert(id);
                                }
                                let (title, body) = texts(&n);
                                let app_name = display_name(&app, &app_id);
                                if let Ok(slot) = SINK.lock() {
                                    if let Some(tx) = slot.as_ref() {
                                        let _ = tx.send(QqHit {
                                            id,
                                            title,
                                            body,
                                            app: app_name,
                                            is_qq,
                                        });
                                    }
                                }
                            }
                        }
                        Err(e) => set(&state2, ChannelState::Error(format!("读取通知失败：{e}"))),
                    },
                    Err(e) => set(&state2, ChannelState::Error(format!("读取通知失败：{e}"))),
                }
            }
        })
        .ok()?;

    Some(QqProbe {
        rx,
        state,
        qq_enabled,
        system_enabled: system,
        stop,
    })
}

fn set(state: &Arc<Mutex<ChannelState>>, next: ChannelState) {
    if let Ok(mut slot) = state.lock() {
        *slot = next;
    }
}

fn texts(n: &windows::UI::Notifications::UserNotification) -> (String, String) {
    let mut title = String::new();
    let mut body = String::new();
    let visual = match n.Notification().and_then(|x| x.Visual()) {
        Ok(v) => v,
        Err(_) => return (title, body),
    };
    let Ok(binding_name) = KnownNotificationBindings::ToastGeneric() else {
        return (title, body);
    };
    let Ok(binding) = visual.GetBinding(&binding_name) else {
        return (title, body);
    };
    let Ok(elements) = binding.GetTextElements() else {
        return (title, body);
    };
    let count = elements.Size().unwrap_or(0);
    for i in 0..count.min(6) {
        if let Ok(text) = elements.GetAt(i) {
            // `AdaptiveNotificationText` is a WinRT object, not a string; its
            // payload is the `Text` property.
            let Ok(h) = text.Text() else { continue };
            let s = h.to_string();
            if i == 0 {
                title = s;
            } else if body.is_empty() {
                body = s;
            } else {
                body.push(' ');
                body.push_str(&s);
            }
        }
    }
    (title, body)
}

/// The name a `System` row prints. `DisplayName` is the app's own localised
/// name and works for packaged apps; unpackaged ones often have none, so the
/// AUMID is stripped to something readable: `Microsoft.WindowsCalculator_8we…!App`
/// → `WindowsCalculator`, plain `QQ` stays `QQ`.
fn display_name(
    app: &windows::ApplicationModel::AppInfo,
    app_id: &str,
) -> String {
    if let Ok(info) = app.DisplayInfo() {
        if let Ok(name) = info.DisplayName() {
            let name = name.to_string();
            if !name.trim().is_empty() {
                return name;
            }
        }
    }
    fallback_name(app_id)
}

/// Pure half of [`display_name`], testable without a WinRT object.
fn fallback_name(app_id: &str) -> String {
    let base = app_id.split('!').next().unwrap_or(app_id);
    let base = base.split('_').next().unwrap_or(base);
    // An unpackaged app's AUMID is often its exe path; without this the
    // "readable" last segment of `...\tool.exe` is `exe`.
    let file = base.rsplit(['\\', '/']).next().unwrap_or(base);
    let stem = match file.get(file.len().saturating_sub(4)..) {
        Some(ext) if ext.eq_ignore_ascii_case(".exe") && file.len() > 4 => {
            &file[..file.len() - 4]
        }
        _ => file,
    };
    let last = stem.rsplit('.').next().unwrap_or(stem).trim();
    if last.is_empty() {
        "未知应用".to_string()
    } else {
        last.to_string()
    }
}

/// Is this AppUserModelId QQ's? QQ registers plain `QQ`; some builds use
/// `Tencent.QQ`. `QQBrowser` and `QQMusic` must not match.
pub fn classify(app_id: &str) -> bool {
    let id = app_id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return false;
    }
    if id == "qq" || id == "tencent.qq" {
        return true;
    }
    let last = id.rsplit(['.', '\\', '!']).next().unwrap_or("");
    last == "qq"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_qq_aumids_match() {
        assert!(classify("QQ"));
        assert!(classify("Tencent.QQ"));
        assert!(classify("com.tencent.qq"));
        assert!(!classify("QQBrowser"));
        assert!(!classify("QQMusic"));
        assert!(!classify(""));
        assert!(!classify("WeChat"));
    }

    #[test]
    fn aumid_fallback_names_are_readable() {
        assert_eq!(fallback_name("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), "WindowsCalculator");
        assert_eq!(fallback_name("QQ"), "QQ");
        assert_eq!(fallback_name(r"C:\Apps\SomeTool\tool.exe"), "tool");
        assert_eq!(fallback_name(r"C:\Apps\SomeTool\tool.Exe"), "tool");
        assert_eq!(fallback_name(""), "未知应用");
    }
}
