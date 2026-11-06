//! Query execution: name matching over the in-memory index, and content
//! matching over a filtered candidate set with ripgrep's grep-searcher.
//!
//! Cancellation is cooperative: every worker compares `generation` before each
//! file and the UI drops events carrying a stale `gen`. No thread is killed,
//! so the `panic = "abort"` release profile stays safe.

use crate::index::{ext_hash, Index};
use crate::model::{now_ms, Ev, Hit, HitKind, Mode, Query, Stats, BATCH_MS, BATCH_SIZE};
use crossbeam_channel::{bounded, Sender};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkFinish, SinkMatch};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

struct Parsed {
    /// Literal / glob terms. A `regex:` token is *not* one of these: treating
    /// its source as a literal substring (the old behaviour) made every query
    /// that also contained `regex:` miss lines that did not contain the pattern
    /// text verbatim.
    terms: Vec<String>,
    regexes: Vec<regex::Regex>,
    ext: Option<String>,
    path_part: Option<String>,
}

fn parse(raw: &str, smart_case: bool) -> Parsed {
    let mut terms = Vec::new();
    let mut regexes = Vec::new();
    let mut ext = None;
    let mut path_part = None;
    for token in raw.split_whitespace() {
        if let Some(v) = token.strip_prefix("ext:") {
            if !v.is_empty() {
                ext = Some(v.trim_start_matches('.').to_ascii_lowercase());
            }
        } else if let Some(v) = token.strip_prefix("path:") {
            if !v.is_empty() {
                path_part = Some(v.to_ascii_lowercase());
            }
        } else if let Some(v) = token.strip_prefix("regex:") {
            match regex::RegexBuilder::new(v)
                .case_insensitive(!smart_case)
                .build()
            {
                Ok(re) => regexes.push(re),
                // A broken regex used to disappear entirely, which turned
                // "regex:[" into an empty query and matched every file. Search
                // for the source as a literal instead, so the result is empty
                // rather than the whole disk.
                Err(_) => terms.push(v.to_string()),
            }
        } else {
            terms.push(token.to_string());
        }
    }
    Parsed { terms, regexes, ext, path_part }
}

/// Classic `*`/`?` glob over bytes. Two-pointer with a single backtrack point:
/// the old rolling-row DP allocated two `bool` vectors per candidate name,
/// which is millions of allocations on a full-disk walk.
fn wildcard(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let s = text.as_bytes();
    let (mut pi, mut si) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&s[si])) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            mark = si;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    let hay = haystack.as_bytes();
    let needle = needle.as_bytes();
    !needle.is_empty()
        && hay.len() >= needle.len()
        && hay
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn term_matches(term: &str, haystack: &str, case_sensitive: bool) -> bool {
    if term.contains('*') || term.contains('?') {
        return wildcard(term, haystack);
    }
    if case_sensitive {
        haystack.contains(term)
    } else {
        // No lowercase allocation: this runs once per term per indexed file.
        contains_ci(haystack, term)
    }
}

fn name_matches(parsed: &Parsed, haystack: &str, smart_case: bool) -> bool {
    parsed
        .terms
        .iter()
        .all(|t| term_matches(t, haystack, smart_case))
        && parsed.regexes.iter().all(|re| re.is_match(haystack))
}

/// Resolved once per query — reading the Shell Folders registry key once per
/// candidate file would dominate a million-entry scan.
fn desktop_prefix() -> String {
    dcx_organizer::desktop_dir().to_string_lossy().into_owned()
}

/// Case-insensitive "path is root or below root" without allocating a
/// lowercase copy of every candidate path. A plain `starts_with` treats
/// `D:\work2` as inside `D:\work`, which leaks results across sibling folders.
fn under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches(['\\', '/']);
    if root.is_empty() {
        return true;
    }
    let p = path.as_bytes();
    let r = root.as_bytes();
    if p.len() < r.len() || !p[..r.len()].eq_ignore_ascii_case(r) {
        return false;
    }
    p.len() == r.len() || matches!(p.get(r.len()), Some(b'\\') | Some(b'/'))
}

fn scope_accepts(path: &str, q: &Query, desk: &str) -> bool {
    match &q.scope {
        crate::model::ScopeSel::Computer => true,
        crate::model::ScopeSel::Desktop => under(path, desk),
        crate::model::ScopeSel::Path(p) => under(path, p),
    }
}

fn category_hashes(key: &str) -> Option<HashSet<u32>> {
    if key == "all" || key.is_empty() {
        return None;
    }
    let cats = crate::model::categories();
    if key == "other" {
        let known: HashSet<u32> = cats
            .iter()
            .filter(|c| c.key != "all" && c.key != "other")
            .flat_map(|c| c.exts.iter().map(|e| ext_hash(e)))
            .collect();
        // "Other" is implemented in `run_*` as "not in any known set".
        return Some(known);
    }
    cats.iter()
        .find(|c| c.key == key)
        .map(|c| c.exts.iter().map(|e| ext_hash(e)).collect())
}

fn other_matches(ext: u32, known: &HashSet<u32>) -> bool {
    !known.contains(&ext)
}

/// Entry point: spawn one short-lived query thread. Long-lived generation
/// checks let an old query wind down instead of being force-killed.
pub fn run(
    q: Query,
    index: Arc<RwLock<Index>>,
    generation: Arc<AtomicU64>,
    ev_tx: Sender<Ev>,
) {
    std::thread::spawn(move || {
        let started = Instant::now();
        let gen = q.gen;
        if q.raw.trim().is_empty() {
            let _ = ev_tx.send(Ev::Done {
                gen,
                partial: false,
                stats: Stats::default(),
            });
            return;
        }
        let smart_case = q.case_sensitive || q.raw.chars().any(|c| c.is_uppercase());
        let parsed = parse(&q.raw, smart_case);
        let mut stats = Stats::default();

        let result = match q.mode {
            Mode::Name => run_name(&q, &parsed, smart_case, &index, &generation, &ev_tx, &mut stats),
            Mode::Content => run_content(&q, &parsed, smart_case, &index, &generation, &ev_tx, &mut stats),
        };

        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        // `run_*` returns true when the scan completed without truncation.
        let complete = result.unwrap_or(false);
        let _ = ev_tx.send(Ev::Done { gen, partial: !complete, stats });
    });
}

fn alive(gen: u64, generation: &Arc<AtomicU64>, started: &Instant, q: &Query) -> bool {
    generation.load(Ordering::SeqCst) == gen
        && started.elapsed().as_millis() < q.time_budget_ms as u128
}

/// Batch sender fd-style: collect hits up to BATCH_SIZE or BATCH_MS, then one
/// channel send instead of one per hit.
fn push_batch(gen: u64, hits: &mut Vec<Hit>, ev_tx: &Sender<Ev>, last: &mut u64) {
    let now = now_ms();
    if hits.len() >= BATCH_SIZE || (now.saturating_sub(*last) >= BATCH_MS && !hits.is_empty()) {
        let batch = std::mem::take(hits);
        let _ = ev_tx.send(Ev::Batch { gen, hits: batch });
        *last = now;
    }
}

fn run_name(
    q: &Query,
    p: &Parsed,
    smart_case: bool,
    index: &Arc<RwLock<Index>>,
    generation: &Arc<AtomicU64>,
    ev_tx: &Sender<Ev>,
    stats: &mut Stats,
) -> Result<bool, ()> {
    let cat = category_hashes(&q.category);
    let desk = desktop_prefix();
    let first_term = p.terms.first().map(|s| s.as_str()).unwrap_or("");
    let started = Instant::now();
    let mut hits = Vec::with_capacity(BATCH_SIZE);
    let mut last = now_ms();
    let mut partial = false;

    // A read lock held for the whole scan keeps the snapshot consistent. The
    // walk writer only needs brief locks per 512-entry batch.
    let Ok(guard) = index.read() else { return Ok(false) };
    for i in 0..guard.entries.len() {
        if (i & 0x0fff) == 0 && !alive(q.gen, generation, &started, q) {
            partial = true;
            break;
        }
        let e = guard.entries[i];
        if e.dead {
            continue;
        }
        if let Some(ref set) = cat {
            let ok = if q.category == "other" {
                other_matches(e.ext, set)
            } else {
                set.contains(&e.ext)
            };
            if !ok {
                continue;
            }
        }
        let path = guard.path(i);
        if !scope_accepts(path, q, &desk) {
            continue;
        }
        stats.scanned += 1;
        let name = guard.name(i);
        if let Some(ref want) = p.ext {
            let got = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase());
            if got.as_deref() != Some(want.as_str()) {
                continue;
            }
        }
        if let Some(ref part) = p.path_part {
            if !path.to_ascii_lowercase().contains(part) {
                continue;
            }
        }
        let haystack = if p.path_part.is_some() { path } else { name };
        if !name_matches(p, haystack, smart_case) {
            continue;
        }
        let score = 100i32
            + if name.to_ascii_lowercase().starts_with(&first_term.to_ascii_lowercase()) {
                30
            } else {
                0
            }
            - (path.matches(['\\', '/']).count() as i32 * 2)
            - (name.len() as i32 / 8);
        let (size, mtime) = file_meta(path);
        hits.push(Hit {
            kind: HitKind::Name,
            path: path.to_string(),
            line: None,
            col: None,
            preview: String::new(),
            highlight: None,
            score,
            size,
            mtime,
            name_lower: name.to_ascii_lowercase(),
            dir_lower: std::path::Path::new(path)
                .parent()
                .map(|d| d.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
        });
        stats.matched += 1;
        push_batch(q.gen, &mut hits, ev_tx, &mut last);
        if stats.matched >= q.max_results as u64 {
            partial = true;
            break;
        }
    }
    if !hits.is_empty() {
        let _ = ev_tx.send(Ev::Batch { gen: q.gen, hits });
    }
    Ok(!partial)
}

/// Extensions that are never useful to line-search. Files with other
/// extensions are still guarded by max_file_size and NUL-byte detection.
const BINARY_EXTS: &[&str] = &[
    "exe", "dll", "sys", "so", "dylib", "pdb", "obj", "lib", "bin", "zip", "rar", "7z",
    "gz", "bz2", "xz", "zst", "iso", "cab", "png", "jpg", "jpeg", "gif", "bmp", "webp",
    "ico", "tif", "tiff", "psd", "heic", "pdf", "doc", "docx", "xls", "xlsx", "ppt",
    "pptx", "mp3", "wav", "flac", "mp4", "mkv", "avi", "mov", "wmv", "sqlite", "db",
    "ttf", "otf", "woff", "woff2", "class", "jar", "pyc", "wasm",
];

struct HitSink {
    out: Arc<Mutex<Vec<Hit>>>,
    path: String,
    terms: Vec<String>,
    regexes: Vec<regex::Regex>,
    smart_case: bool,
    max: usize,
    size: Option<u64>,
    mtime: Option<u64>,
}

fn line_matches(
    line: &str,
    terms: &[String],
    regexes: &[regex::Regex],
    smart_case: bool,
) -> bool {
    let lower = if smart_case {
        String::new()
    } else {
        line.to_ascii_lowercase()
    };
    terms.iter().all(|t| {
        if smart_case {
            line.contains(t)
        } else {
            lower.contains(&t.to_ascii_lowercase())
        }
    }) && regexes.iter().all(|re| re.is_match(line))
}

fn column_of(line: &str, byte: usize) -> u64 {
    line.get(..byte)
        .map(|s| s.chars().count() as u64 + 1)
        .unwrap_or(1)
}

fn file_meta(path: &str) -> (Option<u64>, Option<u64>) {
    match std::fs::metadata(path) {
        Ok(meta) => (
            Some(meta.len()),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        ),
        Err(_) => (None, None),
    }
}

impl Sink for HitSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if !line_matches(&line, &self.terms, &self.regexes, self.smart_case) {
            return Ok(true);
        }
        let lower_line = line.to_ascii_lowercase();
        let (highlight, col) = if let Some(term) = self.terms.first().filter(|t| !t.is_empty()) {
            let needle = if self.smart_case {
                term.clone()
            } else {
                term.to_ascii_lowercase()
            };
            match lower_line.find(&needle) {
                Some(s) => (Some((s, s + needle.len())), column_of(&line, s)),
                None => (None, 1),
            }
        } else if let Some(m) = self.regexes.first().and_then(|re| re.find(&line)) {
            (
                Some((m.start(), m.end())),
                column_of(&line, m.start()),
            )
        } else {
            (None, 1)
        };
        let mut guard = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.max {
            return Ok(false);
        }
        let file = std::path::Path::new(&self.path);
        guard.push(Hit {
            kind: HitKind::Content,
            path: self.path.clone(),
            line: mat.line_number(),
            col: Some(col),
            preview: line.chars().take(240).collect(),
            highlight,
            score: 100,
            size: self.size,
            mtime: self.mtime,
            name_lower: file
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            dir_lower: file
                .parent()
                .map(|d| d.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
        });
        Ok(true)
    }

    fn finish(&mut self, _searcher: &Searcher, finish: &SinkFinish) -> Result<(), Self::Error> {
        // A NUL byte means binary: discard whatever partial lines we collected.
        if finish.binary_byte_offset().is_some() {
            self.out.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
        Ok(())
    }
}

/// A compiled matcher plus its reusable searcher. Building these used to happen
/// once **per candidate file**; for a 100k-file content query that dominated the
/// run time. One per worker is enough — `Searcher` is stateful but reusable.
struct Prepared {
    matcher: grep_regex::RegexMatcher,
    searcher: Searcher,
}

fn prepare(pattern: &str, smart_case: bool, encoding: Option<&str>) -> Option<Prepared> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!smart_case)
        .build(pattern)
        .ok()?;
    let mut builder = SearcherBuilder::new();
    builder
        .line_number(true)
        // Library default is None (unlike the rg CLI) — enable it explicitly.
        .binary_detection(BinaryDetection::quit(b'\0'));
    // Default `memory_map` is Never, which is also the right choice for a large
    // directory walk (mmap can be slower and risks a fault on truncate).
    if let Some(label) = encoding {
        if let Ok(enc) = grep_searcher::Encoding::new(label) {
            builder.encoding(Some(enc));
        }
    }
    Some(Prepared {
        matcher,
        searcher: builder.build(),
    })
}

fn search_file(
    prepared: &mut Prepared,
    file: &str,
    out: &Arc<Mutex<Vec<Hit>>>,
    terms: &[String],
    regexes: &[regex::Regex],
    smart_case: bool,
    size: Option<u64>,
    mtime: Option<u64>,
) -> bool {
    let sink = HitSink {
        out: out.clone(),
        path: file.to_string(),
        terms: terms.to_vec(),
        regexes: regexes.to_vec(),
        smart_case,
        max: 50,
        size,
        mtime,
    };
    // The return type of `search_path` varies between grep-searcher releases;
    // results are collected through the shared `Arc<Mutex<_>>`.
    prepared
        .searcher
        .search_path(&prepared.matcher, Path::new(file), sink)
        .is_ok()
}

fn run_content(
    q: &Query,
    p: &Parsed,
    smart_case: bool,
    index: &Arc<RwLock<Index>>,
    generation: &Arc<AtomicU64>,
    ev_tx: &Sender<Ev>,
    stats: &mut Stats,
) -> Result<bool, ()> {
    if p.terms.is_empty() && p.regexes.is_empty() {
        return Ok(false);
    }
    let started = Instant::now();
    let desk = desktop_prefix();
    let binary_exts: HashSet<u32> = BINARY_EXTS.iter().map(|e| ext_hash(e)).collect();
    let cat = category_hashes(&q.category);

    // 1) Candidate set from the name index — the whole point of the two-layer
    //    architecture: never line-search the entire disk.
    let mut candidates: Vec<String> = Vec::new();
    let mut collect_partial = false;
    {
        let Ok(guard) = index.read() else { return Ok(false) };
        for i in 0..guard.entries.len() {
            if (i & 0x0fff) == 0
                && (generation.load(Ordering::SeqCst) != q.gen
                    || started.elapsed().as_millis() >= q.time_budget_ms as u128)
            {
                collect_partial = true;
                break;
            }
            let e = guard.entries[i];
            if e.dead || (e.ext != 0 && binary_exts.contains(&e.ext)) {
                continue;
            }
            if let Some(ref set) = cat {
                let ok = if q.category == "other" {
                    other_matches(e.ext, set)
                } else {
                    set.contains(&e.ext)
                };
                if !ok {
                    continue;
                }
            }
            let path = guard.path(i);
            if !scope_accepts(path, q, &desk) {
                continue;
            }
            if let Some(ref want) = p.ext {
                let got = guard.name(i).rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase());
                if got.as_deref() != Some(want.as_str()) {
                    continue;
                }
            }
            if let Some(ref part) = p.path_part {
                if !path.to_ascii_lowercase().contains(part) {
                    continue;
                }
            }
            // File size is checked per worker, not here: a serial `metadata`
            // pass over a million candidates could eat the whole time budget
            // before the first byte is read.
            candidates.push(path.to_string());
        }
    }

    // One grep pattern finds candidate lines; the sink then applies every
    // literal/regex term as an AND filter (a line matching only one of several
    // regexes must not be reported).
    let pattern = if !p.regexes.is_empty() {
        p.regexes
            .iter()
            .map(|r| format!("(?:{})", r.as_str()))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        regex::escape(&p.terms[0])
    };
    let primary_encoding = match q.encoding.as_str() {
        "utf8" => Some("utf-8"),
        "gb18030" => Some("gb18030"),
        _ => None,
    };
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 4);
    let (job_tx, job_rx) = bounded::<String>(64);
    let (hit_tx, hit_rx) = bounded::<Hit>(4096);
    let scanned = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let emitted = Arc::new(AtomicU64::new(0));
    let truncated = Arc::new(AtomicBool::new(false));
    let max_results = q.max_results.max(1) as u64;

    // 2) Batcher: 256 hits / 32 ms, mirroring fd's BatchSender.
    let batcher_tx = ev_tx.clone();
    let batch_gen = q.gen;
    let batcher = std::thread::spawn(move || {
        let mut buf: Vec<Hit> = Vec::with_capacity(BATCH_SIZE);
        let mut last = now_ms();
        loop {
            match hit_rx.recv_timeout(Duration::from_millis(BATCH_MS)) {
                Ok(hit) => {
                    buf.push(hit);
                    push_batch(batch_gen, &mut buf, &batcher_tx, &mut last);
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if !buf.is_empty() {
                        push_batch(batch_gen, &mut buf, &batcher_tx, &mut last);
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !buf.is_empty() {
            let _ = batcher_tx.send(Ev::Batch { gen: batch_gen, hits: buf });
        }
    });

    // 3) Workers line-search one file each with a matcher and searcher built
    //    once per worker, not once per file.
    let mut handles = Vec::new();
    for _ in 0..workers {
        let job_rx = job_rx.clone();
        let hit_tx = hit_tx.clone();
        let generation = generation.clone();
        let scanned = scanned.clone();
        let errors = errors.clone();
        let stop = stop.clone();
        let emitted = emitted.clone();
        let truncated = truncated.clone();
        let pattern = pattern.clone();
        let terms = p.terms.clone();
        let regexes = p.regexes.clone();
        let encoding = q.encoding.clone();
        let gen = q.gen;
        let budget = q.time_budget_ms;
        let start = started;
        let auto = encoding == "auto";
        let max_size = q.max_file_size;
        handles.push(std::thread::spawn(move || {
            let Some(mut primary) = prepare(&pattern, smart_case, primary_encoding) else {
                return;
            };
            let mut fallback = if auto {
                prepare(&pattern, smart_case, Some("gb18030"))
            } else {
                None
            };
            while let Ok(file) = job_rx.recv() {
                if generation.load(Ordering::SeqCst) != gen
                    || stop.load(Ordering::SeqCst)
                    || start.elapsed().as_millis() >= budget as u128
                {
                    stop.store(true, Ordering::SeqCst);
                    break;
                }
                let meta = std::fs::metadata(&file).ok();
                let too_big = meta
                    .as_ref()
                    .map(|m| m.len() > max_size)
                    .unwrap_or(true);
                let size = meta.as_ref().map(|m| m.len());
                let mtime = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());
                let out = Arc::new(Mutex::new(Vec::new()));
                if !too_big
                    && !search_file(
                        &mut primary,
                        &file,
                        &out,
                        &terms,
                        &regexes,
                        smart_case,
                        size,
                        mtime,
                    )
                {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
                if !too_big && auto {
                    // BOM sniffing only covers UTF-8/UTF-16. A Chinese keyword
                    // in a legacy GB18030 file would otherwise be invisible.
                    let needs_second = terms.iter().any(|t| !t.is_ascii())
                        && out.lock().map(|g| g.is_empty()).unwrap_or(true);
                    if needs_second {
                        if let Some(fb) = fallback.as_mut() {
                            let _ = search_file(
                                fb, &file, &out, &terms, &regexes, smart_case, size, mtime,
                            );
                        }
                    }
                }
                if let Ok(mut guard) = out.lock() {
                    for hit in guard.drain(..) {
                        if emitted.fetch_add(1, Ordering::Relaxed) >= max_results {
                            truncated.store(true, Ordering::SeqCst);
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                        if hit_tx.send(hit).is_err() {
                            return;
                        }
                    }
                }
                let n = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 64 == 0 {
                    crate::win::trace(&format!("content scanned {n} files"));
                }
            }
        }));
    }
    drop(hit_tx);

    let mut partial = collect_partial;
    for file in candidates {
        if stop.load(Ordering::SeqCst)
            || generation.load(Ordering::SeqCst) != q.gen
            || started.elapsed().as_millis() >= q.time_budget_ms as u128
        {
            partial = true;
            break;
        }
        if job_tx.send(file).is_err() {
            partial = true;
            break;
        }
    }
    drop(job_tx);
    for h in handles {
        let _ = h.join();
    }
    let _ = batcher.join();

    stats.scanned = scanned.load(Ordering::Relaxed);
    stats.errors = errors.load(Ordering::Relaxed);
    stats.matched = emitted.load(Ordering::Relaxed);
    Ok(!partial && !truncated.load(Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_names() {
        assert!(wildcard("*.exe", "setup.exe"));
        assert!(wildcard("read?e.txt", "readme.txt"));
        assert!(!wildcard("*.exe", "setup.txt"));
        assert!(wildcard("*计划*", "我的工作计划.docx"));
    }

    #[test]
    fn scope_prefix_does_not_leak_siblings() {
        assert!(under(r"d:\work\a.txt", r"d:\work"));
        assert!(under(r"d:\work\a.txt", r"d:\"));
        assert!(under(r"c:\users\x\desktop\a.txt", r"c:\users\x\desktop"));
        assert!(!under(r"d:\work2\a.txt", r"d:\work"));
        assert!(!under(r"c:\users\x\desktop2\a.txt", r"c:\users\x\desktop"));
    }

    #[test]
    fn regex_token_is_regex_not_literal() {
        let p = parse("报告 regex:ba+r", false);
        assert_eq!(p.terms, vec!["报告"]);
        assert_eq!(p.regexes.len(), 1);
        assert!(name_matches(&p, "报告 baaar", false));
        assert!(!name_matches(&p, "报告 regex:ba+r", false));
    }

    #[test]
    fn smart_case_regex_is_case_sensitive() {
        let p = parse("regex:Report", true);
        assert!(name_matches(&p, "Report", true));
        assert!(!name_matches(&p, "report", true));
    }

    #[test]
    fn invalid_regex_does_not_match_everything() {
        let p = parse("regex:[", false);
        assert!(p.regexes.is_empty());
        assert_eq!(p.terms, vec!["["]);
        assert!(!name_matches(&p, "anything", false));
    }
}
