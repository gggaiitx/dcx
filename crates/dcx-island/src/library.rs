//! Desktop/library counts for the Perf tab, plus the staging helpers the two
//! "send it to the box" paths share (panel button and drag-to-capsule).
//!
//! The island is a **reader** of the organizer document. Counts come from a
//! 5 s scan (or immediately after `organizer.toml` changes); absorbing runs on
//! a worker thread and persists with `save_keep_gone`, which patches only the
//! three manifest keys instead of rewriting a document the box UI may be
//! editing at the same moment.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dcx_organizer::{Config as OrgConfig, Organizer};

const SCAN: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    /// Top-level desktop items not collected yet.
    pub desktop: usize,
    /// Top-level items in the collection.
    pub library: usize,
}

pub struct Library {
    pub rx: Receiver<Snapshot>,
    stop: Arc<AtomicBool>,
}

impl Drop for Library {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start() -> Library {
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let _ = std::thread::Builder::new()
        .name("dcx-island-library".to_string())
        .spawn(move || run(tx, stop2));
    Library { rx, stop }
}

fn run(tx: Sender<Snapshot>, stop: Arc<AtomicBool>) {
    let mut last_scan: Option<Instant> = None;
    let mut last_stamp = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let stamp = std::fs::metadata(OrgConfig::path())
            .and_then(|m| m.modified())
            .ok();
        let due = last_scan.map(|t| t.elapsed() >= SCAN).unwrap_or(true);
        if due || stamp != last_stamp {
            last_scan = Some(Instant::now());
            last_stamp = stamp;
            // `desktop_items` + `counts` are directory scans of the desktop and
            // the collection's top level; at this cadence that is cheaper than
            // the box panel's own 1.2 s refresh.
            let org = Organizer::new(OrgConfig::load());
            let desktop = org.desktop_items().len();
            let library: usize = org.counts().iter().map(|(_, n, _)| *n).sum();
            if tx.send(Snapshot { desktop, library }).is_err() {
                return;
            }
        }
        std::thread::sleep(TICK);
    }
}

// ---------------------------------------------------------------------------
// Absorbing
// ---------------------------------------------------------------------------

/// Move `paths` into the collection and persist the manifest. Runs on a worker
/// thread — callers never block a frame on a large file.
///
/// Returns `(adopted, saved)`: `saved == false` means the files moved but the
/// manifest write failed, which the toast should say rather than claim success.
pub fn absorb(paths: Vec<PathBuf>) -> (usize, bool) {
    let mut org = Organizer::new(OrgConfig::load());
    let (adopted, _moved) = org.absorb_auto(&paths);
    if adopted == 0 {
        return (0, true);
    }
    (adopted, org.save_keep_gone().is_ok())
}

fn staging_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("dcx-island-staging");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;
    Ok(dir)
}

/// Stamp used in staged names. Local time, second resolution; the staging dir
/// is reused across runs, and `unique` avoids clobbering a leftover from the
/// previous second.
fn stage_name(ext: &str, now: &crate::clock::Clock) -> Result<(PathBuf, String), String> {
    let dir = staging_dir()?;
    let base = format!(
        "剪辑 {}-{:02}-{:02} {:02}-{:02}-{:02}",
        now.year, now.month, now.day, now.hour, now.minute, now.second
    );
    let mut name = format!("{base}.{ext}");
    let mut path = dir.join(&name);
    let mut n = 1;
    while path.exists() {
        name = format!("{base} ({n}).{ext}");
        path = dir.join(&name);
        n += 1;
    }
    Ok((path, name))
}

/// Stage a text clip as `剪辑 <时间戳>.md`. The clipboard store keeps text
/// inline, so a fresh file is the only thing there is to hand to `absorb` —
/// and the entry stays in the history.
pub fn stage_text(text: &str, now: &crate::clock::Clock) -> Result<PathBuf, String> {
    let (path, _) = stage_name("md", now)?;
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("写入暂存文件失败：{e}"))?;
    Ok(path)
}

/// Copy a clipboard image into staging. **Copy**, never absorb the original:
/// the clipboard store owns that file and would show a broken thumbnail from
/// then on.
pub fn stage_image(src: &Path, now: &crate::clock::Clock) -> Result<PathBuf, String> {
    let (path, _) = stage_name("png", now)?;
    std::fs::copy(src, &path).map_err(|e| format!("复制图片到暂存失败：{e}"))?;
    Ok(path)
}
