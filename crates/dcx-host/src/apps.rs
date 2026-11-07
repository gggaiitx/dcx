//! 应用注册表与进程生命周期 —— 平台宿主统一拉起独立应用。
//!
//! 拆分后 `dcx-host.exe` 不再包含任何应用的 UI 代码，只负责：
//!
//! * 任务栏注入（见 `injector`）；
//! * 在**一个**设置窗口里集中编辑平台与各应用的设置（见 `settings_app`）；
//! * 按 `%LOCALAPPDATA%\dcx\apps.toml` 启动／停止独立应用 exe，并做看门狗。
//!
//! 应用清单是编译期静态表：新增一个独立应用 = 加一条 [`AppEntry`] + 一个
//! 设置页，不需要改设置窗口的导航或状态字段。每个应用是独立进程、独立
//! 图标、独立崩溃域；宿主退出时统一停止。
//!
//! 配置同样是「文件即 IPC」：设置窗口写 `apps.toml`，本模块读；应用本身
//! 读自己的配置文件（收纳盒读 `organizer.toml`），两者不互相调用。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use dcx_win32::config_file;

/// 一个独立应用在平台里的登记信息。
pub struct AppEntry {
    /// 稳定标识，也是 `apps.toml` 里 `[apps.<id>]` 的键。
    pub id: &'static str,
    /// 设置中心显示名。
    pub name: &'static str,
    /// 与 `dcx-host.exe` 同目录下的可执行文件名。
    pub exe: &'static str,
    /// 主窗口标题，用于发现「用户自己双击启动」的实例。
    pub window_title: &'static str,
    /// 设置中心的副标题。
    pub description: &'static str,
    /// `apps.toml` 里没有 `[apps.<id>]` 时的缺省开关值。
    ///
    /// 收纳盒继承拆分前的行为（缺省 true）；文件检索缺省 **false** —— 它
    /// 默认不随平台自启、不常驻、不预建全盘索引，只由 host 的全局热键按需
    /// 拉起（见 `hotkey.rs`）。用户在设置中心开启常驻后仍会写 `enabled=true`，
    /// 此时 `spawn_enabled()` 与看门狗照常保活。
    pub default_on: bool,
}

static REGISTRY: &[AppEntry] = &[
    AppEntry {
        id: "box",
        name: "桌面收纳盒",
        exe: "dcx-box.exe",
        window_title: "桌面收纳盒",
        description: "把桌面文件收进独立收纳库，按扩展名分类；拖出即放回桌面",
        default_on: true,
    },
    AppEntry {
        id: "search",
        name: "文件检索",
        exe: "dcx-search.exe",
        // 必须与 dcx-search/src/app.rs::WIN_TITLE 逐字一致。
        window_title: "文件检索",
        description: "全盘文件名与内容检索，默认关闭，由全局热键唤起",
        default_on: false,
    },
    AppEntry {
        id: "island",
        name: "桌面信息岛",
        exe: "dcx-island.exe",
        // 必须与 dcx-island/src/app.rs::WIN_TITLE 逐字一致。
        window_title: "桌面信息岛",
        description: "时间、系统性能、任务提醒、剪贴板与消息聚合，默认关闭",
        // 常驻 topmost 悬浮窗会改变现有桌面；与文件检索一样由设置中心显式开启。
        default_on: false,
    },
];

/// 所有已登记的独立应用（设置中心与看门狗共用这一张表）。
pub fn entries() -> &'static [AppEntry] {
    REGISTRY
}

/// 宿主亲自启动的进程。`try_wait()` 需要可变借用，所以整表放在一个锁里。
struct ChildEntry {
    id: String,
    child: Child,
}
static CHILDREN: Mutex<Vec<ChildEntry>> = Mutex::new(Vec::new());

#[derive(Serialize, Deserialize, Default)]
pub struct AppsConfig {
    #[serde(default)]
    pub apps: BTreeMap<String, AppState>,
}

#[derive(Serialize, Deserialize)]
pub struct AppState {
    /// `None` means "the document has no opinion". Resolving it to `true` in
    /// serde (the old `default_enabled`) contradicted `AppEntry::default_on`,
    /// so hand-editing `apps.toml` without `enabled` silently started the
    /// search app and its full-disk index even though it defaults to off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

fn local_dir() -> PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("dcx");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn config_path() -> PathBuf {
    local_dir().join("apps.toml")
}

pub fn load() -> AppsConfig {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(text) => match toml::from_str(&text) {
            Ok(c) => {
                config_file::note_parse_ok(&p);
                c
            }
            Err(e) => {
                eprintln!("[dcx-apps] parse {p:?} failed ({e}); using defaults (read-only)");
                config_file::note_parse_error(&p);
                AppsConfig::default()
            }
        },
        Err(_) => AppsConfig::default(),
    }
}

/// Atomic, locked replacement. Errors are returned instead of silently
/// truncating the document: the old direct write could leave half a TOML for
/// the watchdog to read.
fn save(cfg: &AppsConfig) -> Result<(), String> {
    let path = config_path();
    let _guard = config_file::FileLock::acquire(&path, config_file::LOCK_TIMEOUT)
        .map_err(|e| format!("配置文件被占用：{e}"))?;
    if config_file::is_poisoned(&path) {
        return Err(config_file::poisoned_error(&path).to_string());
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    config_file::write_atomic(&path, text.as_bytes()).map_err(|e| format!("写入失败：{e}"))
}

pub fn is_enabled(id: &str) -> bool {
    load()
        .apps
        .get(id)
        .and_then(|s| s.enabled)
        .unwrap_or_else(|| find(id).map(|e| e.default_on).unwrap_or(false))
}

/// 设置中心的开关：写文件 + 立刻起／停进程，不要求重启平台。
pub fn set_enabled(id: &str, on: bool) -> Result<(), String> {
    let mut cfg = load();
    cfg.apps.insert(
        id.to_owned(),
        AppState {
            enabled: Some(on),
        },
    );
    save(&cfg)?;
    if on {
        launch(id).map_err(|e| format!("启动应用失败：{e}"))?;
    } else {
        stop(id);
    }
    Ok(())
}

fn find(id: &str) -> Option<&'static AppEntry> {
    REGISTRY.iter().find(|e| e.id == id)
}

fn window(title: &str) -> isize {
    let t = dcx_win32::wide(title);
    unsafe { dcx_win32::FindWindowW(core::ptr::null(), t.as_ptr()) }
}

#[link(name = "user32")]
extern "system" {
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

/// 让窗口正常关闭（而不是强杀进程），这样应用能走完自己的退出路径。
fn close_window(title: &str) {
    const WM_CLOSE: u32 = 0x0010;
    let hwnd = window(title);
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

/// 进程是否还在：先看我们启动的 Child，再看用户手工启动的实例。
pub fn is_running(id: &str) -> bool {
    if let Ok(mut children) = CHILDREN.lock() {
        // `try_wait` needs &mut Child, so `retain_mut` (not `retain`).
        children.retain_mut(|c| {
            c.id != id || matches!(c.child.try_wait(), Ok(None) | Err(_))
        });
        if children.iter().any(|c| c.id == id) {
            return true;
        }
    }
    find(id).map(|e| window(e.window_title) != 0).unwrap_or(false)
}

fn exe_path(entry: &AppEntry) -> Result<PathBuf, String> {
    let mut path = std::env::current_exe().map_err(|e| e.to_string())?;
    path.set_file_name(entry.exe);
    if !path.exists() {
        return Err(format!("找不到 {}", path.display()));
    }
    Ok(path)
}

pub fn launch(id: &str) -> Result<(), String> {
    // `launch` is reachable from the tray, the hotkey thread, the settings
    // window and the watchdog. Two of them can see "not running" before either
    // records its child, which starts the app twice; the lock makes the
    // check-and-spawn atomic.
    static LAUNCH_LOCK: Mutex<()> = Mutex::new(());
    let _guard = LAUNCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let entry = find(id).ok_or_else(|| format!("未登记的应用：{id}"))?;
    if is_running(id) {
        return Ok(());
    }
    let child = Command::new(exe_path(entry)?)
        .spawn()
        .map_err(|e| format!("无法启动 {}：{e}", entry.exe))?;
    if let Ok(mut children) = CHILDREN.lock() {
        children.push(ChildEntry {
            id: id.to_owned(),
            child,
        });
    }
    println!("[dcx-apps] started {} ({})", entry.name, entry.exe);
    Ok(())
}

/// 全局热键用：不管 `apps.toml` 里是不是 enabled 都拉起应用一次。
///
/// `launch()` 本身并不检查 enabled，这里单独留一个语义明确的入口，是为了
/// 让「按需启动」与「看门狗保活」在调用点上不会混淆：按需启动的实例在
/// `enabled=false` 时退出后不会被 5 秒看门狗重新拉起。
pub fn launch_on_demand(id: &str) -> Result<(), String> {
    launch(id)
}

pub fn stop(id: &str) {
    // Take the child out of the table first, so the watchdog cannot observe
    // "still running" while we wait for it to exit.
    let mut ours: Option<ChildEntry> = None;
    if let Ok(mut children) = CHILDREN.lock() {
        let mut i = 0;
        while i < children.len() {
            if children[i].id == id {
                ours = Some(children.remove(i));
            } else {
                i += 1;
            }
        }
    }
    let Some(mut entry) = ours else {
        // 用户自己双击启动的实例不在 Child 表里，用 WM_CLOSE 请它退出。
        if let Some(e) = find(id) {
            close_window(e.window_title);
        }
        return;
    };

    // 先请它自己退出：`WM_CLOSE` → eframe 关闭窗口 → `on_exit` 落盘任务与
    // 剪贴板。直接 `kill()` 会跳过这一步，最多丢掉 900 ms 内的改动。
    if let Some(e) = find(id) {
        close_window(e.window_title);
    }
    if !wait_exit(&mut entry.child, GRACEFUL_STOP) {
        println!(
            "[dcx-apps] {} did not exit in {} ms; terminating",
            id,
            GRACEFUL_STOP.as_millis()
        );
        let _ = entry.child.kill();
        let _ = entry.child.wait();
    }
}

/// 等待子进程退出，超时返回 `false`。
fn wait_exit(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            // A child we cannot query is treated as gone: killing it is a no-op.
            Err(_) => return true,
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// 关闭前给应用多长时间自己退出（落盘 + 释放单实例互斥体）。
const GRACEFUL_STOP: Duration = Duration::from_millis(1200);

/// Set by [`stop_all`]. The watchdog checks it every 100 ms, so shutdown never
/// has to wait a full tick and cannot start one more app after the stop.
static WATCHDOG_STOP: AtomicBool = AtomicBool::new(false);
static WATCHDOG: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

const WATCHDOG_TICK: Duration = Duration::from_secs(5);
/// After this many consecutive failures the app is retried slowly instead of
/// being respawned every 5 s for the rest of the session.
const MAX_FAILURES: u32 = 6;
/// A process that lives longer than this counts as healthy. The watchdog only
/// sees a spawn's outcome on the next tick, so this must exceed `WATCHDOG_TICK`.
const STABLE_RUN: Duration = Duration::from_secs(20);

fn backoff(failures: u32) -> Duration {
    if failures >= MAX_FAILURES {
        Duration::from_secs(600)
    } else {
        Duration::from_secs(5u64 << failures.min(5))
    }
}

/// Per-app watchdog state.
struct Watch {
    /// Earliest time of the next launch attempt.
    due: Instant,
    /// Consecutive launch errors *and* short-lived starts.
    failures: u32,
    /// When we last spawned it, until the next tick tells us whether it lived.
    spawned: Option<Instant>,
}

/// 平台启动时调用：拉起所有启用的应用，并开一条看门狗。
pub fn spawn_enabled() {
    WATCHDOG_STOP.store(false, Ordering::SeqCst);
    for entry in REGISTRY {
        if is_enabled(entry.id) {
            if let Err(e) = launch(entry.id) {
                println!("[dcx-apps] cannot start {}: {e}", entry.name);
            }
        }
    }
    let handle = std::thread::spawn(|| {
        // A crash loop used to restart every 5 s forever, appending to
        // `island.log`/`host.log` and fighting the user's task manager.
        let mut watch: BTreeMap<&'static str, Watch> = BTreeMap::new();
        loop {
            for _ in 0..(WATCHDOG_TICK.as_millis() / 100) as u32 {
                if WATCHDOG_STOP.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if WATCHDOG_STOP.load(Ordering::SeqCst) {
                return;
            }
            for entry in REGISTRY {
                if !is_enabled(entry.id) {
                    watch.remove(entry.id);
                    continue;
                }
                if is_running(entry.id) {
                    // Alive at a tick means it survived the spawn; forget the
                    // backoff so a later crash starts counting from zero.
                    watch.remove(entry.id);
                    continue;
                }
                let now = Instant::now();
                let w = watch.entry(entry.id).or_insert(Watch {
                    due: now,
                    failures: 0,
                    spawned: None,
                });
                // Our last spawn is gone. `launch` returned `Ok`, so this is
                // not a launch error — it is an app that started, read its
                // config and exited (e.g. `island.toml` disabled while
                // `apps.toml` still says enabled). Without counting it, the
                // watchdog respawned it every 5 s forever and `MAX_FAILURES`
                // never applied.
                if let Some(at) = w.spawned.take() {
                    let lived = now.duration_since(at);
                    if lived < STABLE_RUN {
                        w.failures = w.failures.saturating_add(1);
                        let delay = backoff(w.failures);
                        w.due = now + delay;
                        println!(
                            "[dcx-apps] {} exited after {} ms (failure {}); retry in {} s",
                            entry.name,
                            lived.as_millis(),
                            w.failures,
                            delay.as_secs()
                        );
                    } else {
                        w.failures = 0;
                        w.due = now;
                    }
                }
                if now < w.due {
                    continue;
                }
                match launch(entry.id) {
                    Ok(()) => {
                        // Keep `failures`: a successful spawn says nothing
                        // about the process staying alive.
                        w.spawned = Some(Instant::now());
                        w.due = Instant::now() + WATCHDOG_TICK;
                    }
                    Err(e) => {
                        w.failures = w.failures.saturating_add(1);
                        let delay = backoff(w.failures);
                        println!(
                            "[dcx-apps] cannot start {} (failure {}): {e}; retry in {} s",
                            entry.name,
                            w.failures,
                            delay.as_secs()
                        );
                        w.due = now + delay;
                    }
                }
            }
        }
    });
    if let Ok(mut slot) = WATCHDOG.lock() {
        *slot = Some(handle);
    }
}

/// 平台退出前调用：所有独立应用随宿主一起停止，不留孤进程。
pub fn stop_all() {
    // Signal first: the watchdog must not launch anything between the stop
    // loop and `process::exit`, which is how orphan processes appeared.
    WATCHDOG_STOP.store(true, Ordering::SeqCst);
    if let Ok(mut slot) = WATCHDOG.lock() {
        if let Some(handle) = slot.take() {
            let _ = handle.join();
        }
    }
    // Ask every app to close first, then wait **once**. Waiting per app would
    // make the host take N × GRACEFUL_STOP to exit.
    for entry in REGISTRY {
        close_window(entry.window_title);
    }
    let mut pending: Vec<ChildEntry> = match CHILDREN.lock() {
        Ok(mut children) => core::mem::take(&mut *children),
        Err(_) => Vec::new(),
    };
    let deadline = Instant::now() + GRACEFUL_STOP;
    while !pending.is_empty() && Instant::now() < deadline {
        pending.retain_mut(|c| !matches!(c.child.try_wait(), Ok(Some(_))));
        if pending.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    for mut c in pending {
        let _ = c.child.kill();
        let _ = c.child.wait();
    }
}
