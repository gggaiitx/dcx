//! Headless file search: runs `dcx-search.exe --query … --json` from the
//! island's own directory. One worker per query; the UI debounces.
//!
//! The child prints one JSON object (see `dcx-search/src/headless.rs`); exit
//! code 2 means the index snapshot is missing or stale and its stderr is shown
//! verbatim — an honest "index not built yet" beats an empty list.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct FileHit {
    pub path: String,
    pub name: String,
    pub dir: String,
    #[serde(default)]
    pub size: Option<u64>,
    /// Last write time, seconds since the Unix epoch.
    #[serde(default)]
    pub mtime: Option<u64>,
}

#[derive(Debug)]
pub struct Outcome {
    pub query: String,
    pub hits: Vec<FileHit>,
    pub error: Option<String>,
    /// How old the snapshot behind the answer is, in seconds.
    pub index_age_secs: u64,
}

#[derive(Deserialize)]
struct Reply {
    hits: Vec<FileHit>,
    #[serde(default)]
    index_age_secs: u64,
}

pub fn start(query: String, limit: usize) -> Receiver<Outcome> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new()
        .name("dcx-island-search".to_string())
        .spawn(move || {
            let _ = tx.send(run(&query, limit));
        });
    rx
}

/// A child that hangs on a damaged snapshot must not wedge the tab: the
/// process is killed after this and reported as a timeout.
const CHILD_TIMEOUT: Duration = Duration::from_secs(5);

fn run(query: &str, limit: usize) -> Outcome {
    let error = |msg: String| Outcome {
        query: query.to_string(),
        hits: Vec::new(),
        error: Some(msg),
        index_age_secs: 0,
    };
    let Some(exe) = exe_path() else {
        return error("找不到 dcx-search.exe（应与信息岛放在同一目录）".to_string());
    };
    let child = Command::new(&exe)
        .args(["--query", query, "--limit", &limit.to_string(), "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return error(format!("启动文件检索失败：{e}")),
    };

    let deadline = Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return error(format!("等待检索进程失败：{e}"));
            }
        }
    };
    let Some(status) = status else {
        return error("文件检索超时（索引可能损坏，重新打开一次文件检索可重建）".to_string());
    };

    // Read after exit; the JSON is capped by `--limit`, so the pipe cannot
    // have blocked the child on a full buffer in practice.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_end(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr);
    }
    if !status.success() {
        let text = String::from_utf8_lossy(&stderr).trim().to_string();
        let msg = if text.is_empty() {
            format!("检索退出码 {:?}", status.code())
        } else {
            text
        };
        return error(msg);
    }
    match serde_json::from_slice::<Reply>(&stdout) {
        Ok(reply) => Outcome {
            query: query.to_string(),
            hits: reply.hits,
            error: None,
            index_age_secs: reply.index_age_secs,
        },
        Err(e) => error(format!("解析检索结果失败：{e}")),
    }
}

fn exe_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = dir.join("dcx-search.exe");
    exe.exists().then_some(exe)
}
