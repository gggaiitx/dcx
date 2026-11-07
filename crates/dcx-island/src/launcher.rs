//! Start-menu applications: one scan at launch, filtered in memory.
//!
//! Opening goes through `ShellExecuteW open`, which resolves the `.lnk`;
//! "locate" uses `explorer.exe /select,` so the user lands on the shortcut.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Debug)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
}

pub struct Launcher {
    rx: Receiver<Vec<AppEntry>>,
    apps: Vec<AppEntry>,
}

pub fn start() -> Launcher {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new()
        .name("dcx-island-apps".to_string())
        .spawn(move || {
            let _ = tx.send(scan());
        });
    Launcher {
        rx,
        apps: Vec::new(),
    }
}

impl Launcher {
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(apps) = self.rx.try_recv() {
            self.apps = apps;
            changed = true;
        }
        changed
    }

    pub fn all(&self) -> &[AppEntry] {
        &self.apps
    }
}

/// Filter + rank a slice of apps. A free function so the panel can filter the
/// slice it was handed without owning a [`Launcher`].
pub fn matches<'a>(apps: &'a [AppEntry], query: &str, limit: usize) -> Vec<&'a AppEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<&AppEntry> = apps
        .iter()
        .filter(|a| a.name.to_lowercase().contains(&q))
        .collect();
    hits.sort_by(|a, b| {
        let pa = a.name.to_lowercase().starts_with(&q);
        let pb = b.name.to_lowercase().starts_with(&q);
        pb.cmp(&pa)
            .then_with(|| a.name.chars().count().cmp(&b.name.chars().count()))
            .then_with(|| a.name.cmp(&b.name))
    });
    hits.truncate(limit);
    hits
}

fn scan() -> Vec<AppEntry> {
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(programdata) = std::env::var_os("ProgramData") {
        roots.push(PathBuf::from(programdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let mut out = Vec::new();
    for root in roots {
        walk(&root, 0, &mut out);
    }
    out.sort_by_cached_key(|a| a.name.to_lowercase());
    // The two start menus can carry the same shortcut; two identical rows in
    // a five-row list is a waste of a row.
    out.dedup_by(|a, b| a.name.to_lowercase() == b.name.to_lowercase());
    out
}

fn walk(dir: &Path, depth: u8, out: &mut Vec<AppEntry>) {
    // A start menu with thousands of shortcuts is possible on a shared
    // machine; the cap keeps launch time bounded and the list usable.
    if depth > 5 || out.len() >= 4000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            walk(&path, depth + 1, out);
            continue;
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
        {
            if let Some(stem) = path.file_stem() {
                let name = stem.to_string_lossy().trim().to_string();
                if !name.is_empty() && !name.starts_with('~') {
                    out.push(AppEntry { name, path });
                }
            }
        }
    }
}

pub fn open(path: &Path) -> Result<(), String> {
    let op = dcx_win32::wide("open");
    let file = dcx_win32::wide(&path.to_string_lossy());
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
        Err(format!("无法打开「{}」（ShellExecute {code}）", path.display()))
    }
}

pub fn reveal(path: &Path) -> Result<(), String> {
    // One argument: `explorer.exe` splits `/select,"path"` itself, and quoting
    // here keeps spaces inside the path intact.
    let arg = format!("/select,\"{}\"", path.display());
    std::process::Command::new("explorer.exe")
        .arg(arg)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("定位失败：{e}"))
}
