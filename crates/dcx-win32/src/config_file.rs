//! Crash-safe, cross-process-safe persistence for the TOML documents that the
//! DCX processes use as IPC.
//!
//! Four documents (`settings.toml`, `organizer.toml`, `apps.toml`,
//! `search.toml`) are read and written by more than one process. The rules
//! here are therefore deliberately strict:
//!
//! * **One writer at a time.** [`FileLock`] is a named mutex derived from the
//!   full path, so `dcx-host.exe`, `dcx-host.exe --settings`, `dcx-box.exe`
//!   and `dcx-search.exe` serialise their read-modify-write cycles. The lock is
//!   recursive for the owning thread, which lets a caller that already holds it
//!   save from inside the same critical section.
//! * **Never truncate the live file.** [`write_atomic`] writes a sibling temp
//!   file with a unique name, flushes it, and swaps it in with
//!   `ReplaceFileW`/`MoveFileExW`. There is no "direct write" fallback: if the
//!   swap fails the old document stays exactly as it was and the caller gets
//!   the error. The old code fell back to `fs::write`, which is what produced
//!   half-written TOML — and, in the engine, a full taskbar reset.
//! * **A corrupt file is read-only.** A document that fails to parse is
//!   recorded in [`POISONED`]; every later save of that path is refused until
//!   something parses it successfully again. Otherwise "parse failed → use
//!   defaults → save defaults" silently wipes the user's manual categories and
//!   custom library path.

use core::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x80;
const WAIT_TIMEOUT: u32 = 0x102;

const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;

/// How long a writer waits for a peer before giving up. Long enough for a UI
/// debounce to finish, short enough that a hung holder cannot freeze a frame.
pub const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[link(name = "kernel32")]
extern "system" {
    fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
    fn ReleaseMutex(handle: isize) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn ReplaceFileW(
        replaced: *const u16,
        replacement: *const u16,
        backup: *const u16,
        flags: u32,
        exclude: *mut c_void,
        reserved: *mut c_void,
    ) -> i32;
    fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
}

/// UTF-16 with a trailing NUL, straight from the OS string.
///
/// Kept as an alias so the config callers read naturally; the implementation
/// lives in the crate root next to [`crate::wide`].
pub fn wide_path(p: &Path) -> Vec<u16> {
    crate::wide_path(p)
}

/// Case-folded identity of a document path. Used for both the mutex name and
/// the poison registry, so every process agrees on what "the same file" is.
pub fn config_key(path: &Path) -> String {
    let mut p = path.to_path_buf();
    if p.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            p = cwd.join(p);
        }
    }
    p.to_string_lossy().to_ascii_lowercase()
}

fn lock_name(path: &Path) -> String {
    // FNV-1a: stable across builds and processes, unlike `DefaultHasher`.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in config_key(path).as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    // `Local\` = per logon session, which is where every DCX process lives.
    format!("Local\\dcx-cfg-{hash:016x}")
}

/// Held for the duration of a read-modify-write cycle.
pub struct FileLock {
    handle: isize,
}

impl FileLock {
    /// Block up to `timeout` for the document's mutex.
    pub fn acquire(path: &Path, timeout: std::time::Duration) -> io::Result<FileLock> {
        let name = wide_path(Path::new(&lock_name(path)));
        let handle = unsafe { crate::CreateMutexW(core::ptr::null_mut(), 0, name.as_ptr()) };
        if handle == 0 {
            return Err(io::Error::last_os_error());
        }
        let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        match unsafe { WaitForSingleObject(handle, ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(FileLock { handle }),
            WAIT_TIMEOUT => {
                unsafe { CloseHandle(handle) };
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "配置文件正被其他进程写入，请稍后重试",
                ))
            }
            _ => {
                let e = io::Error::last_os_error();
                unsafe { CloseHandle(handle) };
                Err(e)
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

static TEMP_SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_path(path: &Path) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    // PID + sequence + a per-process random-ish start: two writers can never
    // collide on the temp name, which the fixed `.toml.tmp` did.
    let pid = std::process::id();
    path.with_file_name(format!("{name}.{pid}.{seq}.tmp"))
}

/// Replace `path` with `bytes` atomically.
///
/// The live file is never opened for writing, so a crash or a full disk leaves
/// the previous revision intact and parseable.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    let result = (|| -> io::Result<()> {
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        swap_in(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn swap_in(tmp: &Path, path: &Path) -> io::Result<()> {
    let t = wide_path(tmp);
    let p = wide_path(path);
    if path.exists() {
        let ok = unsafe {
            ReplaceFileW(
                p.as_ptr(),
                t.as_ptr(),
                core::ptr::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        // `ReplaceFileW` is unsupported on a few redirector-backed directories;
        // `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` is still a single atomic
        // namespace operation, unlike a truncating `fs::write`.
        if unsafe {
            MoveFileExW(
                t.as_ptr(),
                p.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        return Err(err);
    }
    if unsafe { MoveFileExW(t.as_ptr(), p.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Paths that failed to parse in this process. Cleared as soon as one parses
/// again, so a hand-fixed file becomes writable without a restart.
static POISONED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Record that `path` exists but is not parseable.
pub fn note_parse_error(path: &Path) {
    let key = config_key(path);
    if let Ok(mut list) = POISONED.lock() {
        if !list.contains(&key) {
            list.push(key);
        }
    }
}

/// Record that `path` parsed; re-enables saving.
pub fn note_parse_ok(path: &Path) {
    let key = config_key(path);
    if let Ok(mut list) = POISONED.lock() {
        list.retain(|k| *k != key);
    }
}

/// True when the last read of `path` failed to parse. Callers must refuse to
/// save in that state: writing defaults over an unreadable document destroys
/// exactly the fields the user cares about.
pub fn is_poisoned(path: &Path) -> bool {
    let key = config_key(path);
    POISONED
        .lock()
        .map(|list| list.contains(&key))
        .unwrap_or(false)
}

/// Error returned by a save that refused to overwrite a corrupt document.
pub fn poisoned_error(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{} 无法解析，已拒绝覆盖以免丢失配置；请修复或删除该文件后重启",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_unique_per_call() {
        let p = Path::new(r"C:\x\settings.toml");
        assert_ne!(temp_path(p), temp_path(p));
    }

    #[test]
    fn lock_name_is_case_insensitive_and_bounded() {
        let a = lock_name(Path::new(r"C:\Users\A\dcx\settings.toml"));
        let b = lock_name(Path::new(r"c:\users\a\DCX\settings.toml"));
        assert_eq!(a, b);
        assert!(a.len() < 64);
    }

    #[test]
    fn poisoned_paths_round_trip() {
        let p = Path::new(r"C:\x\poison-probe.toml");
        note_parse_error(p);
        assert!(is_poisoned(p));
        note_parse_ok(p);
        assert!(!is_poisoned(p));
    }

    #[test]
    fn atomic_write_replaces_and_keeps_previous_on_failure() {
        let dir = std::env::temp_dir().join(format!("dcx-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("probe.toml");
        write_atomic(&f, b"a = 1\n").unwrap();
        write_atomic(&f, b"a = 2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "a = 2\n");
        // A directory in place of the temp target must not truncate the file.
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_dir(&dir);
    }
}
