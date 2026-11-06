//! Volume enumeration, cold-start parallel walk and the in-memory name index.
//!
//! The index is a compact byte pool plus fixed-size entries — no `Vec<String>`
//! per million files. It is grown in place under an `RwLock`, so a query can
//! run while the initial walk is still filling it ("边建边出" in the PRD).

use crate::config::Config;
use crate::model::{Ev, ScopeSel};
use crossbeam_channel::Sender;
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        label: *mut u16,
        label_len: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs: *mut u16,
        fs_len: u32,
    ) -> i32;
}

const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOVABLE: u32 = 2;
#[cfg(windows)]
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
#[cfg(windows)]
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;

#[derive(Clone, Debug)]
pub struct Volume {
    pub root: String,
    pub label: String,
    pub removable: bool,
}

/// All mounted fixed volumes plus removable ones (network / CD-ROM excluded).
pub fn volumes() -> Vec<Volume> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide_root = dcx_win32::wide(&root);
        let kind = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let mut label = [0u16; 128];
        let ok = unsafe {
            GetVolumeInformationW(
                wide_root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                0,
            )
        };
        let name = if ok != 0 {
            let end = label.iter().position(|&c| c == 0).unwrap_or(label.len());
            String::from_utf16_lossy(&label[..end])
        } else {
            String::new()
        };
        out.push(Volume {
            root,
            label: if name.is_empty() {
                format!("本地磁盘 ({letter}:)")
            } else {
                format!("{name} ({letter}:)")
            },
            removable: kind == DRIVE_REMOVABLE,
        });
    }
    out
}

/// Resolve a UI scope to concrete walk roots. `Computer` also takes any
/// custom folder that is not already inside a local volume (UNC shares, mounted
/// folders) — otherwise adding such a folder would silently index nothing.
pub fn roots_for(scope: &ScopeSel, cfg: &Config, vols: &[Volume]) -> Vec<String> {
    match scope {
        ScopeSel::Desktop => vec![dcx_organizer::desktop_dir().to_string_lossy().into_owned()],
        ScopeSel::Computer => {
            let mut roots: Vec<String> = vols.iter().map(|v| v.root.clone()).collect();
            for custom in &cfg.scope.custom_roots {
                let lower = custom.to_ascii_lowercase();
                let covered = roots
                    .iter()
                    .any(|r| lower.starts_with(&r.to_ascii_lowercase()));
                if !covered && !roots.iter().any(|r| r.eq_ignore_ascii_case(custom)) {
                    roots.push(custom.clone());
                }
            }
            roots
        }
        ScopeSel::Path(p) => vec![p.clone()],
    }
}

pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn ext_hash(ext: &str) -> u32 {
    fnv1a(ext) as u32
}

// --- on-disk snapshot ------------------------------------------------------
//
// Rebuilding 1.4 M names takes well over a minute, so the same index is written
// next to `search.toml` and loaded on the next launch. It is a plain
// length-prefixed path list: no mmap tricks, no platform-specific layout, and a
// corrupt/old file simply falls back to a full walk.

const SNAPSHOT_MAGIC: &[u8; 8] = b"DCXIDX01";
const SNAPSHOT_VERSION: u32 = 1;
const MAX_PATH_BYTES: u32 = 64 * 1024;
const MAX_ENTRIES: u64 = 5_000_000;

pub fn snapshot_path() -> PathBuf {
    crate::win::local_dir().join("search_index.bin")
}

/// The snapshot is only reusable when the volumes and the walk-affecting
/// options are identical. A different exclude list or drive set means a walk;
/// UI and content settings do not affect the index and are not included.
pub fn snapshot_signature(cfg: &Config, vols: &[Volume]) -> u64 {
    let mut text = String::new();
    let mut roots: Vec<&str> = vols.iter().map(|v| v.root.as_str()).collect();
    roots.sort();
    for root in roots {
        text.push_str(root);
        text.push('|');
    }
    let mut custom = cfg.scope.custom_roots.clone();
    custom.sort();
    for p in custom {
        text.push_str(&p);
        text.push('|');
    }
    let mut excludes = cfg.scope.exclude.clone();
    excludes.sort();
    for e in excludes {
        text.push_str(&e);
        text.push('|');
    }
    text.push(if cfg.scope.include_hidden { '1' } else { '0' });
    text.push(if cfg.scope.follow_links { '1' } else { '0' });
    fnv1a(&text)
}

pub fn save_snapshot(index: &Index, path: &Path, signature: u64) -> std::io::Result<()> {
    let saved_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let tmp = path.with_extension("bin.tmp");
    let file = std::fs::File::create(&tmp)?;
    let mut out = BufWriter::new(file);
    out.write_all(SNAPSHOT_MAGIC)?;
    out.write_all(&SNAPSHOT_VERSION.to_le_bytes())?;
    out.write_all(&saved_at.to_le_bytes())?;
    out.write_all(&signature.to_le_bytes())?;
    let live = index.entries.iter().filter(|e| !e.dead).count() as u64;
    out.write_all(&live.to_le_bytes())?;
    for (i, entry) in index.entries.iter().enumerate() {
        if entry.dead {
            continue;
        }
        let bytes = index.path(i).as_bytes();
        if bytes.len() as u64 > MAX_PATH_BYTES as u64 {
            continue;
        }
        out.write_all(&(bytes.len() as u32).to_le_bytes())?;
        out.write_all(bytes)?;
    }
    out.flush()?;
    drop(out);
    std::fs::rename(&tmp, path)
}

/// `None` for missing, truncated, version-mismatched or scope-mismatched files.
pub fn load_snapshot(path: &Path, expect_signature: u64) -> Option<(Index, u64, usize)> {
    let file = std::fs::File::open(path).ok()?;
    let mut input = BufReader::new(file);
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic).ok()?;
    if &magic != SNAPSHOT_MAGIC {
        return None;
    }
    let version = read_u32(&mut input)?;
    if version != SNAPSHOT_VERSION {
        return None;
    }
    let saved_at = read_u64(&mut input)?;
    let signature = read_u64(&mut input)?;
    if signature != expect_signature {
        return None;
    }
    let count = read_u64(&mut input)?;
    if count == 0 || count > MAX_ENTRIES {
        return None;
    }
    let mut index = Index::new();
    for _ in 0..count {
        let len = read_u32(&mut input)?;
        if len == 0 || len > MAX_PATH_BYTES {
            return None;
        }
        let mut bytes = vec![0u8; len as usize];
        input.read_exact(&mut bytes).ok()?;
        let path = String::from_utf8(bytes).ok()?;
        index.add(&path);
    }
    let entries = index.len();
    if entries == 0 {
        return None;
    }
    Some((index, saved_at, entries))
}

fn read_u32(input: &mut impl Read) -> Option<u32> {
    let mut buf = [0u8; 4];
    input.read_exact(&mut buf).ok()?;
    Some(u32::from_le_bytes(buf))
}

fn read_u64(input: &mut impl Read) -> Option<u64> {
    let mut buf = [0u8; 8];
    input.read_exact(&mut buf).ok()?;
    Some(u64::from_le_bytes(buf))
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub off: u32,
    pub len: u32,
    pub name_off: u32,
    pub name_len: u32,
    pub ext: u32,
    pub dead: bool,
}

pub struct Index {
    pub pool: Vec<u8>,
    pub entries: Vec<Entry>,
    /// Path hash → entry indices, for O(1) incremental delete / dedupe.
    by_path: HashMap<u64, Vec<usize>>,
    live: usize,
}

impl Index {
    pub fn new() -> Self {
        Index {
            pool: Vec::with_capacity(1 << 20),
            entries: Vec::with_capacity(1 << 16),
            by_path: HashMap::new(),
            live: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn path(&self, i: usize) -> &str {
        let e = self.entries[i];
        let a = e.off as usize;
        let b = a + e.len as usize;
        std::str::from_utf8(&self.pool[a..b]).unwrap_or("")
    }

    pub fn name(&self, i: usize) -> &str {
        let e = self.entries[i];
        let a = e.name_off as usize;
        let b = a + e.name_len as usize;
        std::str::from_utf8(&self.pool[a..b]).unwrap_or("")
    }

    pub fn contains(&self, path: &str) -> bool {
        let key = fnv1a(path);
        self.by_path
            .get(&key)
            .map(|v| v.iter().any(|&i| !self.entries[i].dead && self.path(i) == path))
            .unwrap_or(false)
    }

    pub fn add(&mut self, path: &str) {
        if path.is_empty() || self.contains(path) {
            return;
        }
        let bytes = path.as_bytes();
        if self.pool.len() + bytes.len() > u32::MAX as usize {
            return;
        }
        let off = self.pool.len() as u32;
        self.pool.extend_from_slice(bytes);
        let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
        let name_off = off + (bytes.len() - name.len()) as u32;
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| ext_hash(&e.to_ascii_lowercase()))
            .unwrap_or(0);
        let idx = self.entries.len();
        self.entries.push(Entry {
            off,
            len: bytes.len() as u32,
            name_off,
            name_len: name.len() as u32,
            ext,
            dead: false,
        });
        self.by_path.entry(fnv1a(path)).or_default().push(idx);
        self.live += 1;
    }

    pub fn remove(&mut self, path: &str) -> bool {
        let key = fnv1a(path);
        // Copy the candidate indices out first: `self.path(i)` needs an
        // immutable borrow while `by_path` was mutably borrowed.
        let Some(list) = self.by_path.get(&key).cloned() else {
            return false;
        };
        for i in list {
            if !self.entries[i].dead && self.path(i) == path {
                self.entries[i].dead = true;
                self.live = self.live.saturating_sub(1);
                return true;
            }
        }
        false
    }

    /// Drop every entry strictly below one of `prefixes`. Windows reports a
    /// removal as `RemoveKind::Any` without saying whether the path was a file
    /// or a folder, so the caller first tries an exact `remove`; anything not
    /// found there may have been a directory. One scan covers the whole burst.
    pub fn remove_any_prefix(&mut self, prefixes: &[String]) {
        if prefixes.is_empty() {
            return;
        }
        let mut needles: Vec<(String, String)> = Vec::with_capacity(prefixes.len());
        for p in prefixes {
            let needle = if p.ends_with(['\\', '/']) {
                p.clone()
            } else {
                format!("{p}\\")
            };
            needles.push((needle.clone(), needle.replace('\\', "/")));
        }
        for i in 0..self.entries.len() {
            if self.entries[i].dead {
                continue;
            }
            let e = self.entries[i];
            let a = e.off as usize;
            let b = a + e.len as usize;
            let path = std::str::from_utf8(&self.pool[a..b]).unwrap_or("");
            let hit = needles.iter().any(|(back, slash)| {
                path.len() > back.len()
                    && (path.as_bytes()[..back.len()].eq_ignore_ascii_case(back.as_bytes())
                        || path.as_bytes()[..slash.len()]
                            .eq_ignore_ascii_case(slash.as_bytes()))
            });
            if hit {
                self.entries[i].dead = true;
                self.live = self.live.saturating_sub(1);
            }
        }
    }

    /// Drop every entry below a removed directory. `ReadDirectoryChangesW`
    /// reports a directory removal as one event, not one per child, so the
    /// prefix scan is the only way to keep stale paths out of the index.
    pub fn remove_prefix(&mut self, prefix: &str) {
        let needle = if prefix.ends_with(['\\', '/']) {
            prefix.to_string()
        } else {
            format!("{prefix}\\")
        };
        let needle_slash = needle.replace('\\', "/");
        for i in 0..self.entries.len() {
            if self.entries[i].dead {
                continue;
            }
            let e = self.entries[i];
            let a = e.off as usize;
            let b = a + e.len as usize;
            let path = std::str::from_utf8(&self.pool[a..b]).unwrap_or("");
            let hit = path.len() > needle.len() && {
                let bytes = path.as_bytes();
                bytes[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
                    || bytes[..needle_slash.len()].eq_ignore_ascii_case(needle_slash.as_bytes())
            };
            if hit {
                self.entries[i].dead = true;
                self.live = self.live.saturating_sub(1);
            }
        }
    }

    /// Re-file every entry below `old` under `new` after a directory rename
    /// (drag-move in Explorer sends one event for the folder, not per child).
    pub fn rename_prefix(&mut self, old: &str, new: &str) {
        if old.eq_ignore_ascii_case(new) {
            return;
        }
        let mut moved: Vec<String> = Vec::new();
        for i in 0..self.entries.len() {
            if self.entries[i].dead {
                continue;
            }
            let e = self.entries[i];
            let a = e.off as usize;
            let b = a + e.len as usize;
            let path = std::str::from_utf8(&self.pool[a..b]).unwrap_or("");
            let under = path.len() >= old.len()
                && path.as_bytes()[..old.len()].eq_ignore_ascii_case(old.as_bytes())
                && (path.len() == old.len()
                    || path.as_bytes().get(old.len()) == Some(&b'\\')
                    || path.as_bytes().get(old.len()) == Some(&b'/'));
            if under {
                let mut next = String::with_capacity(new.len() + path.len() - old.len());
                next.push_str(new);
                next.push_str(&path[old.len()..]);
                moved.push(next);
                self.entries[i].dead = true;
                self.live = self.live.saturating_sub(1);
            }
        }
        for path in moved {
            self.add(&path);
        }
    }
}

enum WalkItem {
    Entry(String),
    Error(String),
}

/// Case-insensitive substring match over the path. `excludes` must already be
/// lowercase (the caller lowercases the config once per walk): doing it here
/// meant one allocation per rule per file — tens of millions on a full disk.
fn excluded(path: &Path, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }
    let text = path.to_string_lossy();
    let hay = text.as_bytes();
    excludes.iter().any(|rule| {
        let needle = rule.as_bytes();
        !needle.is_empty()
            && hay.len() >= needle.len()
            && hay
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle))
    })
}

#[cfg(windows)]
fn hidden_by_attribute(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path)
        .map(|m| m.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn hidden_by_attribute(_path: &Path) -> bool {
    false
}

/// Kick off the cold-start walk on a background thread. Results are appended
/// to `index` as they arrive; `IndexReady` is emitted when the walk finishes.
pub fn spawn_build(
    index: Arc<RwLock<Index>>,
    cfg: Config,
    scope: ScopeSel,
    volumes: Vec<Volume>,
    ev_tx: Sender<Ev>,
    indexing: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    epoch: u64,
) {
    std::thread::spawn(move || {
        let started = Instant::now();
        indexing.store(true, Ordering::SeqCst);
        let roots = roots_for(&scope, &cfg, &volumes);
        if roots.is_empty() {
            indexing.store(false, Ordering::SeqCst);
            return;
        }

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        // fd's backpressure rule: bounded to 2x worker count so a fast walker
        // cannot outrun the index and grow memory without limit.
        let (tx, rx) = crossbeam_channel::bounded::<WalkItem>(threads * 2);

        let index_write = index.clone();
        let scanned_write = scanned.clone();
        let ev_progress = ev_tx.clone();
        let builder = std::thread::spawn(move || {
            let mut pending: Vec<String> = Vec::with_capacity(512);
            let mut errors: u64 = 0;
            let mut first_error: Option<String> = None;
            let mut last_progress = Instant::now();
            for item in rx {
                match item {
                    WalkItem::Entry(p) => pending.push(p),
                    WalkItem::Error(e) => {
                        errors += 1;
                        if first_error.is_none() {
                            first_error = Some(e);
                        }
                    }
                }
                if pending.len() >= 512 {
                    if let Ok(mut guard) = index_write.write() {
                        for p in pending.drain(..) {
                            guard.add(&p);
                        }
                    }
                    let n = scanned_write.fetch_add(512, Ordering::Relaxed) + 512;
                    // A full-disk walk can take a minute; without periodic
                    // progress the UI sits at "已扫描 0 项" the whole time.
                    if last_progress.elapsed() >= Duration::from_millis(500) {
                        last_progress = Instant::now();
                        let _ = ev_progress.send(Ev::Progress {
                            gen: 0,
                            epoch,
                            scanned: n,
                            indexing: true,
                        });
                    }
                }
            }
            if !pending.is_empty() {
                let n = pending.len() as u64;
                if let Ok(mut guard) = index_write.write() {
                    for p in pending.drain(..) {
                        guard.add(&p);
                    }
                }
                scanned_write.fetch_add(n, Ordering::Relaxed);
            }
            let _ = ev_progress.send(Ev::Progress {
                gen: 0,
                epoch,
                scanned: scanned_write.load(Ordering::Relaxed),
                indexing: false,
            });
            if let Some(e) = first_error {
                crate::win::trace(&format!("first walk error: {e}"));
            }
            errors
        });

        let mut wb = ignore::WalkBuilder::new(&roots[0]);
        for r in &roots[1..] {
            wb.add(r);
        }
        // Full-disk search must NOT inherit ripgrep's defaults: hidden files,
        // .gitignore and global ignore files would silently drop user files.
        wb.hidden(false)
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false)
            .follow_links(cfg.scope.follow_links)
            .same_file_system(true)
            .threads(threads);

        let excludes: Vec<String> = cfg
            .scope
            .exclude
            .iter()
            .map(|e| e.to_ascii_lowercase())
            .collect();
        let include_hidden = cfg.scope.include_hidden;
        wb.build_parallel().run(|| {
            let tx = tx.clone();
            let excludes = excludes.clone();
            // `run` takes an FnMut builder called once per worker thread, so
            // the Arc must be cloned here rather than moved into the visitor.
            let stop = stop.clone();
            Box::new(move |res| {
                if stop.load(Ordering::Relaxed) {
                    return ignore::WalkState::Quit;
                }
                match res {
                    Ok(entry) => {
                        let p = entry.path();
                        let is_dir = entry
                            .file_type()
                            .map(|t| t.is_dir())
                            .unwrap_or_else(|| p.is_dir());
                        if excluded(p, &excludes) {
                            return ignore::WalkState::Skip;
                        }
                        if is_dir {
                            return ignore::WalkState::Continue;
                        }
                        if !include_hidden {
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy())
                                .unwrap_or_default();
                            if name.starts_with('.') || hidden_by_attribute(p) {
                                return ignore::WalkState::Continue;
                            }
                        }
                        let _ = tx.send(WalkItem::Entry(p.to_string_lossy().into_owned()));
                    }
                    Err(e) => {
                        let _ = tx.send(WalkItem::Error(e.to_string()));
                    }
                }
                ignore::WalkState::Continue
            })
        });
        drop(tx);
        let errors = builder.join().unwrap_or(0);
        indexing.store(false, Ordering::SeqCst);
        let entries = index.read().map(|g| g.len()).unwrap_or(0);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        crate::win::trace(&format!(
            "index ready: {entries} entries, {errors} walk errors, {elapsed_ms} ms"
        ));
        let _ = ev_tx.send(Ev::IndexReady {
            epoch,
            entries,
            errors,
            elapsed_ms,
            cached: false,
        });
    });
}

/// Incremental maintenance with `notify` (Windows: ReadDirectoryChangesW).
/// The watcher must stay alive, so this thread parks itself for the process
/// lifetime; dropping the watcher would silently stop all updates.
///
/// `watch_epoch` is shared by every watcher this process ever starts. A rebuild
/// bumps it, and the older thread exits on its next event instead of writing
/// into an index that is no longer displayed.
pub fn spawn_watch(
    index: Arc<RwLock<Index>>,
    scope: ScopeSel,
    volumes: Vec<Volume>,
    cfg: Config,
    ev_tx: Sender<Ev>,
    watch_epoch: Arc<AtomicU64>,
    epoch: u64,
    dirty: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        use notify::{EventKind, RecursiveMode, Watcher};
        let (event_tx, event_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = event_tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                crate::win::trace(&format!("watcher unavailable, index stays cold: {e}"));
                return;
            }
        };

        let roots = roots_for(&scope, &cfg, &volumes);
        for r in &roots {
            if let Err(e) = watcher.watch(Path::new(r), RecursiveMode::Recursive) {
                crate::win::trace(&format!("cannot watch {r}: {e}"));
            }
        }
        let excludes: Vec<String> = cfg
            .scope
            .exclude
            .iter()
            .map(|e| e.to_ascii_lowercase())
            .collect();
        let include_hidden = cfg.scope.include_hidden;
        let mut pending_name: Vec<PathBuf> = Vec::new();
        let mut pending_remove: Vec<PathBuf> = Vec::new();

        loop {
            if watch_epoch.load(Ordering::SeqCst) != epoch {
                break;
            }
            let event = match event_rx.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(Ok(ev)) => ev,
                Ok(Err(_)) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    flush_removes(&index, &mut pending_remove);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if watch_epoch.load(Ordering::SeqCst) != epoch {
                break;
            }
            if event.need_rescan() {
                // Buffer overflow: ReadDirectoryChangesW dropped events. The
                // honest answer is a fresh walk, not a silently stale index.
                crate::win::trace("watcher overflow — requesting index rebuild");
                dirty.store(true, Ordering::Relaxed);
                let _ = ev_tx.send(Ev::Rebuild {
                    epoch,
                    reason: "文件变更过多".into(),
                });
                continue;
            }
            // Any name-level change makes the on-disk snapshot stale; the
            // periodic save turns this flag into a new file.
            if matches!(
                &event.kind,
                EventKind::Create(_)
                    | EventKind::Remove(_)
                    | EventKind::Modify(notify::event::ModifyKind::Name(_))
            ) {
                dirty.store(true, Ordering::Relaxed);
            }
            match event.kind {
                EventKind::Create(_) => {
                    let mut add: Vec<String> = Vec::new();
                    for p in &event.paths {
                        if p.is_dir() || excluded(p, &excludes) {
                            continue;
                        }
                        if !include_hidden && is_hidden_path(p) {
                            continue;
                        }
                        add.push(p.to_string_lossy().into_owned());
                    }
                    if !add.is_empty() {
                        if let Ok(mut guard) = index.write() {
                            for p in add {
                                guard.add(&p);
                            }
                        }
                    }
                }
                EventKind::Remove(notify::event::RemoveKind::Folder) => {
                    if let Ok(mut guard) = index.write() {
                        for p in &event.paths {
                            guard.remove_prefix(&p.to_string_lossy());
                        }
                    }
                }
                EventKind::Remove(_) => {
                    // File deletes arrive in bursts (a folder delete on Windows
                    // can produce thousands). Buffer them and pay one write lock
                    // per 250 ms instead of one per file. Windows also reports
                    // folder removal as `Any`, so `flush_removes` falls back to
                    // a prefix scan for paths that were not indexed as files.
                    pending_remove.extend(event.paths.iter().cloned());
                    if pending_remove.len() >= 256 {
                        flush_removes(&index, &mut pending_remove);
                    }
                }
                EventKind::Modify(notify::event::ModifyKind::Name(mode)) => {
                    // Windows sends the halves as two separate events (From,
                    // then To); macOS/Linux send both paths in one. Handle both
                    // shapes explicitly instead of assuming paths[0] is old.
                    use notify::event::RenameMode;
                    match mode {
                        RenameMode::From => {
                            pending_name = event.paths.clone();
                        }
                        RenameMode::To => {
                            if let (Some(old), Some(new)) =
                                (pending_name.pop(), event.paths.first())
                            {
                                apply_rename(
                                    &index,
                                    &old,
                                    new,
                                    &excludes,
                                    include_hidden,
                                );
                            } else if let Some(new) = event.paths.first() {
                                // A move-in from outside the watched roots only
                                // produces the destination half.
                                add_if_indexable(&index, new, &excludes, include_hidden);
                            }
                        }
                        RenameMode::Both => {
                            if event.paths.len() >= 2 {
                                apply_rename(
                                    &index,
                                    &event.paths[0],
                                    &event.paths[1],
                                    &excludes,
                                    include_hidden,
                                );
                            }
                        }
                        _ => {
                            if event.paths.len() >= 2 {
                                apply_rename(
                                    &index,
                                    &event.paths[0],
                                    &event.paths[1],
                                    &excludes,
                                    include_hidden,
                                );
                            } else {
                                pending_name = event.paths.clone();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // Keep `watcher` alive until the thread is torn down at process exit.
        drop(watcher);
    });
}

fn is_hidden_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    name.starts_with('.') || hidden_by_attribute(path)
}

fn add_if_indexable(
    index: &Arc<RwLock<Index>>,
    path: &Path,
    excludes: &[String],
    include_hidden: bool,
) {
    if path.is_dir() || excluded(path, excludes) {
        return;
    }
    if !include_hidden && is_hidden_path(path) {
        return;
    }
    if let Ok(mut guard) = index.write() {
        guard.add(&path.to_string_lossy());
    }
}

/// A rename is one entry when the watched path is a file, but a whole subtree
/// when it is a directory.
fn apply_rename(
    index: &Arc<RwLock<Index>>,
    old: &Path,
    new: &Path,
    excludes: &[String],
    include_hidden: bool,
) {
    if new.is_dir() {
        let drop_subtree = excluded(new, excludes) || (!include_hidden && is_hidden_path(new));
        if let Ok(mut guard) = index.write() {
            if drop_subtree {
                guard.remove_prefix(&old.to_string_lossy());
            } else {
                guard.rename_prefix(&old.to_string_lossy(), &new.to_string_lossy());
            }
        }
    } else {
        if let Ok(mut guard) = index.write() {
            guard.remove(&old.to_string_lossy());
        }
        add_if_indexable(index, new, excludes, include_hidden);
    }
}

fn flush_removes(index: &Arc<RwLock<Index>>, pending: &mut Vec<PathBuf>) {
    if pending.is_empty() {
        return;
    }
    let mut unknown: Vec<String> = Vec::new();
    if let Ok(mut guard) = index.write() {
        for p in pending.drain(..) {
            let s = p.to_string_lossy().into_owned();
            if !guard.remove(&s) {
                unknown.push(s);
            }
        }
        guard.remove_any_prefix(&unknown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_is_case_insensitive_and_rule_bounded() {
        let rules = vec!["node_modules".to_string(), "appdata\\local\\temp".to_string()];
        assert!(excluded(
            Path::new(r"D:\work\node_modules\pkg\a.js"),
            &rules
        ));
        assert!(excluded(
            Path::new(r"C:\Users\X\AppData\Local\Temp\a.txt"),
            &rules
        ));
        assert!(!excluded(Path::new(r"D:\work\src\a.js"), &rules));
    }

    #[cfg(windows)]
    #[test]
    fn prefix_removal_drops_children_only() {
        let mut index = Index::new();
        index.add(r"D:\work\a.txt");
        index.add(r"D:\work\sub\b.txt");
        index.add(r"D:\work2\c.txt");
        assert_eq!(index.len(), 3);
        index.remove_prefix(r"D:\work");
        assert_eq!(index.len(), 1);
        assert!(index.contains(r"D:\work2\c.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn directory_rename_moves_children() {
        let mut index = Index::new();
        index.add(r"D:\work\a.txt");
        index.add(r"D:\work\sub\b.txt");
        index.rename_prefix(r"D:\work", r"D:\archive");
        assert_eq!(index.len(), 2);
        assert!(index.contains(r"D:\archive\a.txt"));
        assert!(index.contains(r"D:\archive\sub\b.txt"));
        assert!(!index.contains(r"D:\work\a.txt"));
    }

    #[test]
    fn snapshot_round_trip_and_scope_signature() {
        let dir = std::env::temp_dir().join(format!("dcx-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.bin");
        let mut index = Index::new();
        index.add(r"D:\work\a.txt");
        index.add(r"D:\work\sub\b.txt");
        index.add(r"D:\work\a.txt");
        save_snapshot(&index, &path, 42).unwrap();
        let (loaded, saved_at, entries) = load_snapshot(&path, 42).expect("round trip");
        assert_eq!(entries, 2);
        assert!(loaded.contains(r"D:\work\a.txt"));
        assert!(loaded.contains(r"D:\work\sub\b.txt"));
        assert!(saved_at > 0);
        assert!(load_snapshot(&path, 43).is_none());

        let vols = vec![Volume {
            root: r"C:\".into(),
            label: "C".into(),
            removable: false,
        }];
        let mut cfg = Config::default();
        let first = snapshot_signature(&cfg, &vols);
        cfg.scope.include_hidden = true;
        assert_ne!(first, snapshot_signature(&cfg, &vols));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
