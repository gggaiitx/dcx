//! Headless file search: runs `dcx-search.exe --query … --json` from the
//! island's own directory. One worker per query; the UI debounces.
//!
//! The child prints one JSON object (see `dcx-search/src/headless.rs`); exit
//! code 2 means the index snapshot is missing or stale and its stderr is shown
//! verbatim — an honest "index not built yet" beats an empty list.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct FileHit {
    pub path: String,
    pub name: String,
    pub dir: String,
}

#[derive(Debug)]
pub struct Outcome {
    pub query: String,
    pub hits: Vec<FileHit>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Reply {
    hits: Vec<FileHit>,
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

fn run(query: &str, limit: usize) -> Outcome {
    let error = |msg: String| Outcome {
        query: query.to_string(),
        hits: Vec::new(),
        error: Some(msg),
    };
    let Some(exe) = exe_path() else {
        return error("找不到 dcx-search.exe（应与信息岛放在同一目录）".to_string());
    };
    let out = match std::process::Command::new(&exe)
        .args(["--query", query, "--limit", &limit.to_string(), "--json"])
        .output()
    {
        Ok(o) => o,
        Err(e) => return error(format!("启动文件检索失败：{e}")),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            format!("检索退出码 {:?}", out.status.code())
        } else {
            stderr
        };
        return error(msg);
    }
    match serde_json::from_str::<Reply>(&String::from_utf8_lossy(&out.stdout)) {
        Ok(reply) => Outcome {
            query: query.to_string(),
            hits: reply.hits,
            error: None,
        },
        Err(e) => error(format!("解析检索结果失败：{e}")),
    }
}

fn exe_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = dir.join("dcx-search.exe");
    exe.exists().then_some(exe)
}
