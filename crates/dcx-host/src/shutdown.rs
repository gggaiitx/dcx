//! Quitting paths.
//!
//! * the tray's 「退出」 ends the process; Explorer keeps the injected engine and
//!   the taskbar keeps the look it was given;
//! * [`restore_taskbar_and_quit`] hands the taskbar back first: it stops the
//!   watchdog, restarts Explorer, waits for the shell to come back and only then
//!   exits.
//!
//! Why the shell restart and not a `FreeLibrary`: XAML holds the TAP object we
//! registered in `InitializeXamlDiagnosticsEx`, and the element handles cached in
//! `dcx-engine` point into that tree. Unloading the image on its own leaves
//! Explorer calling into freed pages (0xc000027b stowed exception). Letting the
//! process die takes the whole registrations with it.
//!
//! It is also the only way the *shell's own* state comes back: the engine wrote
//! XAML properties into the taskbar tree (`Height`, `Margin`, `CornerRadius`,
//! `ScaleTransform`, …) and cleared `Shell_TrayWnd`'s DWM border colour. Those
//! values live in Explorer, so they outlive this process and are only reset when
//! the shell rebuilds its taskbar.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Nothing in flight.
const IDLE: u8 = 0;
/// A restart is running on the worker thread.
const BUSY: u8 = 1;
/// The last attempt failed; the reason is in [`ERROR`].
const FAILED: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(IDLE);
static ERROR: Mutex<Option<String>> = Mutex::new(None);

/// How long Explorer is given to disappear after `taskkill`, and then to come
/// back, before we start it ourselves.
const SHELL_TIMEOUT: Duration = Duration::from_secs(8);

/// True while the shell is being restarted. The settings window keeps painting
/// and disables the button on this.
pub fn is_busy() -> bool {
    STATE.load(Ordering::SeqCst) == BUSY
}

/// Reason the previous attempt did not get as far as exiting, if any. Taken, so
/// the settings window shows it exactly once.
pub fn take_error() -> Option<String> {
    ERROR.lock().ok().and_then(|mut slot| slot.take())
}

/// Restart Explorer, then exit the process.
///
/// Returns immediately: the restart takes a second or two and the window has to
/// keep repainting its "restarting" line while it happens. On success this
/// process is gone, so nothing after the call matters; on failure the reason is
/// left in [`take_error`] and injection is resumed, because a half-done restore
/// must not also leave the taskbar unmanaged.
pub fn restore_taskbar_and_quit() {
    if STATE.swap(BUSY, Ordering::SeqCst) == BUSY {
        return;
    }
    std::thread::spawn(|| match restore() {
        Ok(()) => {
            // The platform is going away: stop every standalone app with it so
            // no tile is left painting into a taskbar that is no longer managed.
            crate::apps::stop_all();
            // `exit` would otherwise leave the spawned settings window running.
            crate::settings_app::close_settings_window();
            std::process::exit(0);
        }
        Err(e) => {
            if let Ok(mut slot) = ERROR.lock() {
                *slot = Some(format!("恢复任务栏失败：{e}"));
            }
            STATE.store(FAILED, Ordering::SeqCst);
        }
    });
}

fn restore() -> Result<(), String> {
    // 1. Stop re-injection before the shell goes away. The watchdog polls every
    //    5 s and would otherwise put the engine straight back into the fresh
    //    Explorer while this process is still alive, which makes the restart
    //    look like it did nothing.
    crate::injector::pause();

    // 2. Kill Explorer. `/F` because the shell restarts itself on a normal close
    //    request; killing the process is what unloads `dcx_engine.dll`.
    if let Err(e) = kill_shell() {
        crate::injector::resume();
        return Err(e);
    }
    // Wait for the image to actually go away, otherwise the "is it back?" test
    // below sees the old process and returns at once.
    if !wait_for_shell(false, SHELL_TIMEOUT) {
        crate::injector::resume();
        return Err("资源管理器没有退出，请检查是否有程序阻止它关闭".to_string());
    }

    // 3. Wait for Windows' own shell restart (AutoRestartShell). When that is
    //    off the desktop stays black, so start it ourselves rather than leave
    //    the user with no taskbar at all.
    if !wait_for_shell(true, SHELL_TIMEOUT) {
        Command::new("explorer.exe")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                crate::injector::resume();
                format!("启动资源管理器失败：{e}")
            })?;
        if !wait_for_shell(true, SHELL_TIMEOUT) {
            crate::injector::resume();
            return Err(
                "资源管理器没有起来；请在任务管理器 → 运行新任务里执行 explorer.exe".to_string(),
            );
        }
    }

    // 4. Explorer rebuilt the taskbar and the work area from its own defaults,
    //    and the engine went away with the old process. Let the caller exit.
    Ok(())
}

fn kill_shell() -> Result<(), String> {
    let status = Command::new("taskkill")
        .args(["/F", "/IM", "explorer.exe"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        // 128 = "process not found": there was no shell to kill, and step 3
        // still produces a clean one.
        Ok(_) if !crate::injector::shell_present() => Ok(()),
        Ok(s) => Err(format!(
            "结束资源管理器失败（taskkill 退出码 {}）",
            s.code().unwrap_or(-1)
        )),
        Err(e) => Err(format!("无法运行 taskkill：{e}")),
    }
}

/// Poll until an `explorer.exe` is present (`want = true`) or absent.
fn wait_for_shell(want: bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if crate::injector::shell_present() == want {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}
