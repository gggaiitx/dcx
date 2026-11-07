//! `--query <词> [--limit N] --json`: one-shot search against the on-disk
//! index snapshot, for the island's search tab.
//!
//! Runs before the single-instance mutex: the caller wants an answer, not a
//! second window. Exit codes: 0 success, 1 usage, 2 snapshot missing or stale
//! (the honest "index not built yet" signal the island shows verbatim).

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use crossbeam_channel::unbounded;
use serde::Serialize;

use crate::config::Config;
use crate::index;
use crate::model::{Ev, Mode, Query, ScopeSel};

#[derive(Serialize)]
struct Out {
    query: String,
    partial: bool,
    elapsed_ms: u64,
    hits: Vec<HitOut>,
}

#[derive(Serialize)]
struct HitOut {
    path: String,
    name: String,
    dir: String,
    size: Option<u64>,
    mtime: Option<u64>,
}

pub fn run(args: &[String]) -> i32 {
    let mut query = None;
    let mut limit = 12usize;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--query" => {
                i += 1;
                query = args.get(i).cloned();
            }
            "--limit" => {
                i += 1;
                limit = args
                    .get(i)
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(12)
                    .clamp(1, 50);
            }
            _ => {}
        }
        i += 1;
    }
    let Some(raw) = query.filter(|q| !q.trim().is_empty()) else {
        eprintln!("用法：dcx-search --query <关键词> [--limit N] --json");
        return 1;
    };

    let cfg = Config::load();
    let vols = index::volumes();
    let signature = index::snapshot_signature(&cfg, &vols);
    let path = index::snapshot_path();
    let Some((idx, _, _)) = index::load_snapshot(&path, signature) else {
        eprintln!(
            "索引快照不存在或已过期（{}）；先打开一次文件检索建立索引",
            path.display()
        );
        return 2;
    };

    let (tx, rx) = unbounded();
    let gen = 1u64;
    let text = raw.clone();
    let q = Query {
        gen,
        mode: Mode::Name,
        raw,
        // Whole machine: the island's field is a launcher-first quick search,
        // and the index already honours the configured excludes and volumes.
        scope: ScopeSel::Computer,
        category: "all".to_string(),
        case_sensitive: false,
        max_results: limit,
        max_file_size: cfg.max_file_size(),
        encoding: cfg.content.encoding.clone(),
        time_budget_ms: cfg.content.time_budget_ms,
    };
    crate::query::run(
        q,
        Arc::new(RwLock::new(idx)),
        Arc::new(AtomicU64::new(gen)),
        tx,
    );

    let mut hits = Vec::new();
    let mut partial = false;
    let mut elapsed_ms = 0;
    for ev in rx {
        match ev {
            Ev::Batch { gen: g, hits: mut b } if g == gen => {
                hits.append(&mut b);
                hits.truncate(limit);
            }
            Ev::Done {
                gen: g,
                partial: p,
                stats,
            } if g == gen => {
                partial = p;
                elapsed_ms = stats.elapsed_ms;
                break;
            }
            _ => {}
        }
    }

    let out = Out {
        query: text,
        partial,
        elapsed_ms,
        hits: hits
            .into_iter()
            .map(|h| {
                let (dir, name) = split_path(&h.path);
                HitOut {
                    path: h.path,
                    name,
                    dir,
                    size: h.size,
                    mtime: h.mtime,
                }
            })
            .collect(),
    };
    match serde_json::to_string(&out) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(e) => {
            eprintln!("序列化结果失败：{e}");
            1
        }
    }
}

fn split_path(path: &str) -> (String, String) {
    match path.rfind(['\\', '/']) {
        Some(at) => (path[..at].to_string(), path[at + 1..].to_string()),
        None => (String::new(), path.to_string()),
    }
}
