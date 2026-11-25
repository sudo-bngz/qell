//! Workspace index: every text file of the project in memory, for instant
//! search across files, plus the definitions found in them.
//!
//! The [`Indexer`] builds the index on a background thread and keeps it
//! current. A full refresh walks the workspace again (respecting
//! `.gitignore`) and re-reads only the files whose size or modification
//! time changed; it happens when the workspace opens and when files are
//! created, renamed or deleted from the editor. Everything else (a save,
//! git noticing changes) names the paths that changed, and only those
//! are looked at. Nothing runs on a timer.
//!
//! Search uses a trigram filter: each file keeps the sorted set of the
//! three-byte sequences it contains, and a plain-text query only scans the
//! files that contain all of the query's trigrams.

pub mod symbols;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

pub use symbols::{terraform_candidates, Symbol};

/// Files above this size are not indexed (minified bundles, data dumps).
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Stop after this many files: a home directory is not a project.
const MAX_FILES: usize = 50_000;
/// Results stop after this many matches.
const MAX_MATCHES: usize = 20_000;

/// One indexed file.
#[derive(Debug)]
pub struct IndexedFile {
    pub path: PathBuf,
    /// Relative to the workspace root.
    pub rel: PathBuf,
    pub text: Arc<str>,
    /// Language id (for definitions), if known.
    pub language: Option<String>,
    pub symbols: Vec<Symbol>,
    /// Whether `symbols` has been computed yet (the first index reads
    /// files first, then finds definitions, so search is ready sooner).
    symbols_done: bool,
    trigrams: Arc<[u32]>,
    modified: Option<SystemTime>,
    len: u64,
}

fn trigram(b: &[u8]) -> u32 {
    (b[0].to_ascii_lowercase() as u32) << 16 | (b[1].to_ascii_lowercase() as u32) << 8 | b[2].to_ascii_lowercase() as u32
}

fn trigrams(text: &[u8]) -> Vec<u32> {
    let mut t: Vec<u32> = text.windows(3).map(trigram).collect();
    t.sort_unstable();
    t.dedup();
    t
}

impl IndexedFile {
    fn new(path: PathBuf, rel: PathBuf, text: String, modified: Option<SystemTime>, len: u64) -> Self {
        let language = qell_core::syntax::language_for_path(&path);
        let trigrams = trigrams(text.as_bytes()).into();
        Self { path, rel, text: text.into(), language, symbols: Vec::new(), symbols_done: false, trigrams, modified, len }
    }

    /// The same file with its definitions found.
    fn with_symbols(&self, extractor: &mut symbols::Extractor) -> Self {
        let symbols = match &self.language {
            Some(l) if extractor.supports(l) => extractor.extract(l, &self.text),
            _ => Vec::new(),
        };
        Self {
            path: self.path.clone(),
            rel: self.rel.clone(),
            text: self.text.clone(),
            language: self.language.clone(),
            symbols,
            symbols_done: true,
            trigrams: self.trigrams.clone(),
            modified: self.modified,
            len: self.len,
        }
    }

    fn may_contain(&self, query_trigrams: &[u32]) -> bool {
        query_trigrams.iter().all(|t| self.trigrams.binary_search(t).is_ok())
    }
}

/// A definition somewhere in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub line: usize,
    pub col: usize,
    pub end_col: usize,
    pub kind: String,
    /// The line's text, trimmed, for lists.
    pub preview: String,
}

/// The indexed workspace.
#[derive(Debug, Default)]
pub struct Index {
    pub root: PathBuf,
    files: Vec<Arc<IndexedFile>>,
    /// Symbol name → (file index, symbol index).
    by_name: HashMap<String, Vec<(usize, usize)>>,
    /// The latest change to any indexed file below each folder,
    /// workspace-relative (`""` is the workspace).
    newest: HashMap<PathBuf, SystemTime>,
}

impl Index {
    fn build(root: PathBuf, mut files: Vec<Arc<IndexedFile>>) -> Self {
        files.sort_by(|a, b| a.rel.cmp(&b.rel));
        let mut by_name: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (fi, f) in files.iter().enumerate() {
            for (si, s) in f.symbols.iter().enumerate() {
                by_name.entry(s.name.clone()).or_default().push((fi, si));
            }
        }
        let mut newest: HashMap<PathBuf, SystemTime> = HashMap::new();
        for f in &files {
            let Some(m) = f.modified else { continue };
            for dir in f.rel.ancestors().skip(1) {
                let at = newest.entry(dir.to_path_buf()).or_insert(m);
                if *at < m {
                    *at = m;
                }
            }
        }
        Self { root, files, by_name, newest }
    }

    /// When anything indexed below `dir` (workspace-relative) last changed.
    /// Ignored and binary files aren't indexed, so they don't count.
    pub fn newest_in(&self, dir: &Path) -> Option<SystemTime> {
        self.newest.get(dir).copied()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.by_name.values().map(Vec::len).sum()
    }

    /// Indexed text of a file.
    pub fn text(&self, path: &Path) -> Option<Arc<str>> {
        self.files.iter().find(|f| f.path == path).map(|f| f.text.clone())
    }

    /// Definitions named `name`.
    pub fn definitions(&self, name: &str) -> Vec<Location> {
        self.by_name
            .get(name)
            .into_iter()
            .flatten()
            .map(|&(fi, si)| {
                let f = &self.files[fi];
                let s = &f.symbols[si];
                let preview = f.text.lines().nth(s.line).unwrap_or("").trim().chars().take(200).collect();
                Location {
                    path: f.path.clone(),
                    rel: f.rel.clone(),
                    line: s.line,
                    col: s.col,
                    end_col: s.end_col,
                    kind: s.kind.clone(),
                    preview,
                }
            })
            .collect()
    }

    /// Search every file. `cancelled` is checked between files so a newer
    /// query can stop an older one.
    pub fn search(&self, query: &Query, cancelled: &dyn Fn() -> bool) -> Result<SearchResults, String> {
        let re = query.regex()?;
        // Trigram filter for plain text (a regex could match without any
        // of its literal characters, so it scans everything).
        let filter: Vec<u32> = if query.is_regex { Vec::new() } else { trigrams(query.pattern.as_bytes()) };
        let mut out = SearchResults::default();
        for f in &self.files {
            if cancelled() {
                return Err("cancelled".into());
            }
            if !f.may_contain(&filter) {
                continue;
            }
            let matches = find_in_text(&f.text, &re, MAX_MATCHES - out.total);
            if matches.is_empty() {
                continue;
            }
            out.total += matches.len();
            out.files.push(FileMatches { path: f.path.clone(), rel: f.rel.clone(), matches });
            if out.total >= MAX_MATCHES {
                out.truncated = true;
                break;
            }
        }
        Ok(out)
    }
}

/// What to search for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl Query {
    pub fn regex(&self) -> Result<regex::Regex, String> {
        let pat = if self.is_regex { self.pattern.clone() } else { regex::escape(&self.pattern) };
        let pat = if self.whole_word { format!(r"\b(?:{pat})\b") } else { pat };
        regex::RegexBuilder::new(&pat)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .size_limit(10 * (1 << 20))
            .build()
            .map_err(|e| e.to_string().lines().last().unwrap_or("invalid regular expression").trim().to_string())
    }
}

/// A match: 0-based line, char columns, and the line for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    /// The line (or a window of it, for long lines).
    pub preview: String,
    /// The match's char range within `preview`.
    pub preview_range: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub matches: Vec<LineMatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub files: Vec<FileMatches>,
    pub total: usize,
    pub truncated: bool,
}

fn find_in_text(text: &str, re: &regex::Regex, limit: usize) -> Vec<LineMatch> {
    let mut out = Vec::new();
    let (mut line, mut line_start, mut scanned) = (0usize, 0usize, 0usize);
    for m in re.find_iter(text) {
        if m.start() == m.end() {
            continue; // an empty match marks nothing
        }
        // Advance the line counter to the match.
        for (i, b) in text.as_bytes()[scanned..m.start()].iter().enumerate() {
            if *b == b'\n' {
                line += 1;
                line_start = scanned + i + 1;
            }
        }
        scanned = m.start();
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        let line_text = text[line_start..line_end].trim_end_matches('\r');
        let start = text[line_start..m.start()].chars().count();
        let end = start + text[m.start()..m.end().min(line_end)].chars().count();
        // Long lines: show a window around the match.
        const WINDOW: usize = 160;
        let chars: Vec<char> = line_text.chars().collect();
        let (from, to) = if chars.len() <= WINDOW {
            (0, chars.len())
        } else {
            let from = start.saturating_sub(40);
            (from, (from + WINDOW).min(chars.len()))
        };
        let lead = chars[from..to].iter().take_while(|c| c.is_whitespace()).count();
        let preview: String = chars[from + lead..to].iter().collect();
        let preview_range = ((start - from).saturating_sub(lead), (end.min(to) - from).saturating_sub(lead));
        out.push(LineMatch { line, start, end, preview, preview_range });
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Read a file for the index: `None` when it is too large, binary or
/// unreadable.
fn read_text(path: &Path, len: u64) -> Option<String> {
    if len > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    // A NUL byte early on means binary, like git and grep decide.
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

type Found = (PathBuf, u64, Option<SystemTime>);

/// Files of the workspace, honouring .gitignore and the ignore list, and
/// whether there were more than [`MAX_FILES`]. A home directory's caches
/// and app data are skipped (see [`qell_git::is_home_noise`]).
fn walk(root: &Path, ignore: &[String]) -> (Vec<Found>, bool) {
    walk_up_to(root, ignore, MAX_FILES)
}

fn walk_up_to(root: &Path, ignore: &[String], max: usize) -> (Vec<Found>, bool) {
    let ignore: Vec<String> = ignore.to_vec();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            name != ".git" && !ignore.iter().any(|i| *i == name) && !(e.depth() > 0 && qell_git::is_home_noise(e.path()))
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if out.len() >= max {
            return (out, true);
        }
        out.push((entry.into_path(), meta.len(), meta.modified().ok()));
    }
    (out, false)
}

fn workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8)
}

/// Run `f` over `items` on a few threads.
fn parallel<T: Sync, R: Send>(
    items: &[T],
    f: impl Fn(&T, &mut symbols::Extractor) -> Option<R> + Sync,
    with_extractor: bool,
) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers())
            .map(|_| {
                s.spawn(|| {
                    // Compiling the tags queries costs a few ms: only when needed.
                    let mut extractor = if with_extractor { symbols::Extractor::new() } else { symbols::Extractor::empty() };
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        if let Some(r) = f(item, &mut extractor) {
                            out.push(r);
                        }
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

/// Bring `old` up to date with the disk, text only: unchanged files are
/// kept as they are (with their definitions), changed and new ones are
/// (re)read. Run [`with_symbols`] after to find definitions in them.
fn reindex_text(root: &Path, ignore: &[String], old: &Index) -> (Index, bool) {
    let previous: HashMap<&Path, &Arc<IndexedFile>> = old.files.iter().map(|f| (f.path.as_path(), f)).collect();
    let mut keep = Vec::new();
    let mut todo = Vec::new();
    let (found, truncated) = walk(root, ignore);
    for (path, len, modified) in found {
        match previous.get(path.as_path()) {
            Some(f) if f.len == len && f.modified == modified && modified.is_some() => keep.push(Arc::clone(f)),
            _ => todo.push((path, len, modified)),
        }
    }
    let fresh = parallel(
        &todo,
        |(path, len, modified), _| {
            let text = read_text(path, *len)?;
            let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
            Some(Arc::new(IndexedFile::new(path.clone(), rel, text, *modified, *len)))
        },
        false,
    );
    keep.extend(fresh);
    (Index::build(root.to_path_buf(), keep), truncated)
}

/// Bring `old` up to date for these paths only, without walking the
/// workspace: a file is re-read if it changed, dropped if it is gone or
/// no longer readable as text, added if new; a folder (git names an
/// untracked one as a whole) is walked on its own. Paths outside the
/// workspace or under an ignored name are left alone.
fn update_text(root: &Path, ignore: &[String], old: &Index, paths: &[PathBuf]) -> Index {
    let mut files: HashMap<PathBuf, Arc<IndexedFile>> = old.files.iter().map(|f| (f.path.clone(), Arc::clone(f))).collect();
    let mut todo: Vec<Found> = Vec::new();
    let mut consider = |files: &mut HashMap<PathBuf, Arc<IndexedFile>>, (path, len, modified): Found| match files.get(&path) {
        Some(f) if f.len == len && f.modified == modified && modified.is_some() => {}
        _ => todo.push((path, len, modified)),
    };
    for path in paths {
        let Ok(rel) = path.strip_prefix(root) else { continue };
        if rel.components().any(|c| {
            let c = c.as_os_str().to_string_lossy();
            c == ".git" || ignore.iter().any(|i| *i == c)
        }) {
            continue;
        }
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                // As many as the whole index may hold, at most.
                let (found, _) = walk_up_to(path, ignore, MAX_FILES);
                let present: std::collections::HashSet<&PathBuf> = found.iter().map(|(p, _, _)| p).collect();
                files.retain(|p, _| !p.starts_with(path) || present.contains(p));
                for f in found {
                    consider(&mut files, f);
                }
            }
            Ok(meta) if meta.is_file() => consider(&mut files, (path.clone(), meta.len(), meta.modified().ok())),
            _ => files.retain(|p, _| !p.starts_with(path)),
        }
    }
    let fresh = parallel(
        &todo,
        |(path, len, modified), _| Some((path.clone(), read_text(path, *len).map(|text| (text, *len, *modified)))),
        false,
    );
    for (path, read) in fresh {
        match read {
            Some((text, len, modified)) => {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                files.insert(path.clone(), Arc::new(IndexedFile::new(path, rel, text, modified, len)));
            }
            // Binary or too big now: it no longer belongs.
            None => {
                files.remove(&path);
            }
        }
    }
    Index::build(root.to_path_buf(), files.into_values().collect())
}

/// Find definitions in the files that don't have them yet.
fn with_symbols(index: &Index) -> Index {
    let todo: Vec<&Arc<IndexedFile>> = index.files.iter().filter(|f| !f.symbols_done).collect();
    let mut done: HashMap<PathBuf, Arc<IndexedFile>> =
        parallel(&todo, |f, x| Some((f.path.clone(), Arc::new(f.with_symbols(x)))), true).into_iter().collect();
    let files = index.files.iter().map(|f| done.remove(&f.path).unwrap_or_else(|| Arc::clone(f))).collect();
    Index::build(index.root.clone(), files)
}

/// Text and definitions in one go (tests, small changes).
#[cfg(test)]
fn reindex(root: &Path, ignore: &[String], old: &Index) -> Index {
    with_symbols(&reindex_text(root, ignore, old).0)
}

/// Where the index is at, for the UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    pub indexing: bool,
    pub files: usize,
    pub symbols: usize,
    /// The workspace has more files than are indexed.
    pub truncated: bool,
}

/// The most files indexed, for saying so.
pub const FILE_LIMIT: usize = MAX_FILES;

enum Request {
    Refresh,
    /// Only these paths changed.
    Update(Vec<PathBuf>),
    SetRoot(PathBuf),
    SetIgnore(Vec<String>),
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Keeps an [`Index`] of a workspace on a background thread.
pub struct Indexer {
    index: Arc<RwLock<Index>>,
    status: Arc<Mutex<Status>>,
    tx: Sender<Request>,
}

impl Indexer {
    /// Start indexing `root` right away.
    pub fn new(root: PathBuf, ignore: Vec<String>, waker: Option<Waker>) -> Self {
        let index = Arc::new(RwLock::new(Index { root: root.clone(), ..Default::default() }));
        let status = Arc::new(Mutex::new(Status { indexing: true, ..Default::default() }));
        let (tx, rx) = mpsc::channel::<Request>();
        {
            let (index, status) = (index.clone(), status.clone());
            std::thread::Builder::new()
                .name("qell-index".into())
                .spawn(move || {
                    let (mut root, mut ignore) = (root, ignore);
                    let mut pending = Some(Request::Refresh);
                    let mut truncated = false;
                    // Said once per workspace, not on every refresh.
                    let mut warned = false;
                    loop {
                        let req = match pending.take() {
                            Some(r) => r,
                            None => match rx.recv() {
                                Ok(r) => r,
                                Err(_) => return,
                            },
                        };
                        // Coalesce a burst of requests into one pass: a
                        // whole walk if any asks for it, else the paths.
                        let (mut fresh_root, mut full) = (false, false);
                        let mut paths: Vec<PathBuf> = Vec::new();
                        for r in std::iter::once(req).chain(rx.try_iter()) {
                            match r {
                                Request::Refresh => full = true,
                                Request::Update(p) => paths.extend(p),
                                Request::SetRoot(r) => {
                                    root = r;
                                    fresh_root = true;
                                    full = true;
                                    warned = false;
                                }
                                Request::SetIgnore(i) => {
                                    ignore = i;
                                    full = true;
                                }
                            }
                        }
                        status.lock().unwrap().indexing = true;
                        if let Some(w) = &waker {
                            w();
                        }
                        let started = std::time::Instant::now();
                        // Text first: search works as soon as files are read.
                        let text = {
                            let empty = Index::default();
                            let old = index.read().unwrap();
                            if full {
                                let (text, more) = reindex_text(&root, &ignore, if fresh_root { &empty } else { &old });
                                truncated = more;
                                if more && !warned {
                                    log::warn!("more than {MAX_FILES} files under {}; indexing the first ones", root.display());
                                    warned = true;
                                }
                                text
                            } else {
                                paths.sort();
                                paths.dedup();
                                update_text(&root, &ignore, &old, &paths)
                            }
                        };
                        let pending = text.files.iter().any(|f| !f.symbols_done);
                        *status.lock().unwrap() =
                            Status { indexing: pending, files: text.file_count(), symbols: text.symbol_count(), truncated };
                        if pending {
                            *index.write().unwrap() = text;
                            if let Some(w) = &waker {
                                w();
                            }
                            // Then definitions.
                            let full = with_symbols(&index.read().unwrap());
                            *status.lock().unwrap() =
                                Status { indexing: false, files: full.file_count(), symbols: full.symbol_count(), truncated };
                            *index.write().unwrap() = full;
                        } else {
                            *index.write().unwrap() = text;
                        }
                        log::debug!("indexed in {:?}", started.elapsed());
                        if let Some(w) = &waker {
                            w();
                        }
                    }
                })
                .expect("spawn index thread");
        }
        Self { index, status, tx }
    }

    /// Re-read what changed on disk.
    pub fn refresh(&self) {
        let _ = self.tx.send(Request::Refresh);
    }

    /// Re-read only these paths (absolute): files, or folders to look
    /// through. Cheaper than [`refresh`](Self::refresh), which walks the
    /// whole workspace.
    pub fn update(&self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            let _ = self.tx.send(Request::Update(paths));
        }
    }

    pub fn set_root(&self, root: PathBuf) {
        let _ = self.tx.send(Request::SetRoot(root));
    }

    pub fn set_ignore(&self, ignore: Vec<String>) {
        let _ = self.tx.send(Request::SetIgnore(ignore));
        self.refresh();
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }

    /// The index as of now (shared; searches read it in place).
    pub fn index(&self) -> Arc<RwLock<Index>> {
        self.index.clone()
    }
}

/// Runs searches on a background thread; a new query cancels the one
/// still running.
pub struct Searcher {
    generation: Arc<AtomicU64>,
    tx: Sender<(u64, Query)>,
    rx: Receiver<(u64, Query, Result<SearchResults, String>)>,
}

impl Searcher {
    pub fn new(index: Arc<RwLock<Index>>, waker: Option<Waker>) -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, req_rx) = mpsc::channel::<(u64, Query)>();
        let (res_tx, rx) = mpsc::channel();
        let gen = generation.clone();
        std::thread::Builder::new()
            .name("qell-search".into())
            .spawn(move || {
                while let Ok(mut req) = req_rx.recv() {
                    // Only the latest query matters.
                    while let Ok(newer) = req_rx.try_recv() {
                        req = newer;
                    }
                    let (id, query) = req;
                    let result = {
                        let index = index.read().unwrap();
                        index.search(&query, &|| gen.load(Ordering::Relaxed) != id)
                    };
                    if matches!(&result, Err(e) if e == "cancelled") {
                        continue;
                    }
                    if res_tx.send((id, query, result)).is_err() {
                        return;
                    }
                    if let Some(w) = &waker {
                        w();
                    }
                }
            })
            .expect("spawn search thread");
        Self { generation, tx, rx }
    }

    /// Start a search; returns its id.
    pub fn search(&self, query: Query) -> u64 {
        let id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.tx.send((id, query));
        id
    }

    /// Finished searches since the last call: (id, query, result).
    pub fn poll(&self) -> Vec<(u64, Query, Result<SearchResults, String>)> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("infra")).unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {\n    let total = compute_total(3);\n}\n").unwrap();
        std::fs::write(d.join("src/lib.rs"), "/// Adds things.\npub fn compute_total(n: u32) -> u32 {\n    n * 2 // TOTAL\n}\n")
            .unwrap();
        std::fs::write(
            d.join("infra/main.tf"),
            "variable \"region\" {}\nresource \"aws_s3_bucket\" \"logs\" {\n  bucket = var.region\n}\n",
        )
        .unwrap();
        std::fs::write(d.join("generated.txt"), "compute_total in a generated file\n").unwrap();
        std::fs::write(d.join(".gitignore"), "generated.txt\n").unwrap();
        std::fs::write(d.join("target/out.rs"), "fn compute_total() {}\n").unwrap();
        std::fs::write(d.join("blob.bin"), b"compute_total\0\x01\x02").unwrap();
        dir
    }

    fn index(d: &Path) -> Index {
        reindex(d, &["target".to_string()], &Index::default())
    }

    fn q(p: &str) -> Query {
        Query { pattern: p.into(), ..Default::default() }
    }

    #[test]
    fn indexes_text_files_respecting_ignores() {
        let dir = workspace();
        let idx = index(dir.path());
        let rels: Vec<_> = idx.files.iter().map(|f| f.rel.to_string_lossy().into_owned()).collect();
        assert_eq!(rels, vec![".gitignore", "infra/main.tf", "src/lib.rs", "src/main.rs"]);
    }

    #[test]
    fn each_folder_knows_its_latest_change() {
        let dir = workspace();
        let d = dir.path();
        let set = |rel: &str, secs: u64| {
            let f = std::fs::File::options().write(true).open(d.join(rel)).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        };
        set(".gitignore", 100);
        set("infra/main.tf", 200);
        set("src/lib.rs", 300);
        set("src/main.rs", 400);
        let idx = index(d);
        let at = |rel: &str| {
            idx.newest_in(Path::new(rel)).and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs())
        };
        assert_eq!(at("src"), Some(400));
        assert_eq!(at("infra"), Some(200));
        assert_eq!(at(""), Some(400));
        // Ignored folders aren't indexed, so they have nothing to say.
        assert_eq!(at("target"), None);
    }

    #[test]
    fn updating_named_paths_touches_only_those() {
        let dir = workspace();
        let d = dir.path();
        let old = index(d);
        let ignore = ["target".to_string()];
        // An edit, a deletion, a new file, a new folder, an ignored path.
        std::fs::write(d.join("src/main.rs"), "fn main() { renamed_call(); }\n").unwrap();
        std::fs::remove_file(d.join("infra/main.tf")).unwrap();
        std::fs::write(d.join("src/new.rs"), "pub fn fresh() {}\n").unwrap();
        std::fs::create_dir_all(d.join("docs/guide")).unwrap();
        std::fs::write(d.join("docs/guide/intro.md"), "# Intro\n").unwrap();
        std::fs::write(d.join("target/later.rs"), "fn x() {}\n").unwrap();
        // A file nobody names isn't looked at, even though it changed.
        std::fs::write(d.join("src/lib.rs"), "changed but not named\n").unwrap();
        let paths: Vec<PathBuf> =
            ["src/main.rs", "infra/main.tf", "src/new.rs", "docs", "target/later.rs"].iter().map(|r| d.join(r)).collect();
        let idx = with_symbols(&update_text(d, &ignore, &old, &paths));
        let rels: Vec<_> = idx.files.iter().map(|f| f.rel.to_string_lossy().into_owned()).collect();
        assert_eq!(rels, vec![".gitignore", "docs/guide/intro.md", "src/lib.rs", "src/main.rs", "src/new.rs"]);
        assert!(idx.text(&d.join("src/main.rs")).unwrap().contains("renamed_call"));
        assert!(idx.text(&d.join("src/lib.rs")).unwrap().contains("compute_total"), "not named, not re-read");
        assert_eq!(idx.definitions("fresh").len(), 1);
        // Unchanged files are kept as they were, not read again.
        let before = old.files.iter().find(|f| f.rel == Path::new(".gitignore")).unwrap();
        let after = idx.files.iter().find(|f| f.rel == Path::new(".gitignore")).unwrap();
        assert!(Arc::ptr_eq(before, after));
    }

    #[test]
    fn a_walk_says_when_it_stopped_early() {
        let dir = workspace();
        let (all, more) = walk_up_to(dir.path(), &["target".to_string()], 100);
        assert!(!more);
        let (some, more) = walk_up_to(dir.path(), &["target".to_string()], 2);
        assert!(more);
        assert_eq!(some.len(), 2);
        assert!(all.len() > 2);
    }

    #[test]
    fn plain_case_and_word_search() {
        let dir = workspace();
        let idx = index(dir.path());
        let never = || false;
        let r = idx.search(&q("total"), &never).unwrap();
        assert_eq!(r.total, 4); // total, compute_total ×2, TOTAL
        assert_eq!(r.files.len(), 2);
        let r = idx.search(&Query { case_sensitive: true, ..q("TOTAL") }, &never).unwrap();
        assert_eq!(r.total, 1);
        let r = idx.search(&Query { whole_word: true, ..q("total") }, &never).unwrap();
        assert_eq!(r.total, 2, "compute_total is not the word total");
        let m = &idx.search(&q("compute_total(3)"), &never).unwrap().files[0].matches[0];
        assert_eq!((m.line, m.start, m.end), (1, 16, 32));
        assert_eq!(m.preview, "let total = compute_total(3);");
        assert_eq!(m.preview_range, (12, 28));
    }

    #[test]
    fn regex_search_and_errors() {
        let dir = workspace();
        let idx = index(dir.path());
        let r = idx.search(&Query { is_regex: true, ..q(r"fn \w+\(") }, &|| false).unwrap();
        assert_eq!(r.total, 2);
        assert!(idx.search(&Query { is_regex: true, ..q("(unclosed") }, &|| false).is_err());
    }

    #[test]
    fn cancelled_search_stops() {
        let dir = workspace();
        let idx = index(dir.path());
        assert!(idx.search(&q("total"), &|| true).is_err());
    }

    #[test]
    fn definitions_across_languages() {
        let dir = workspace();
        let idx = index(dir.path());
        let defs = idx.definitions("compute_total");
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!((defs[0].rel.to_str(), defs[0].line, defs[0].col), (Some("src/lib.rs"), 1, 7));
        assert_eq!(defs[0].preview, "pub fn compute_total(n: u32) -> u32 {");
        let var = idx.definitions("var.region");
        assert_eq!(var[0].rel.to_str(), Some("infra/main.tf"));
        assert_eq!(idx.definitions("aws_s3_bucket.logs")[0].line, 1);
    }

    #[test]
    fn refresh_rereads_only_changes() {
        let dir = workspace();
        let d = dir.path();
        let first = index(d);
        let lib_before = first.files.iter().find(|f| f.rel.ends_with("lib.rs")).unwrap().clone();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(d.join("src/main.rs"), "fn main() { renamed_total(); }\n").unwrap();
        std::fs::write(d.join("src/new.rs"), "fn brand_new() {}\n").unwrap();
        let second = reindex(d, &["target".to_string()], &first);
        let lib_after = second.files.iter().find(|f| f.rel.ends_with("lib.rs")).unwrap();
        assert!(Arc::ptr_eq(&lib_before, lib_after), "unchanged file reused");
        assert_eq!(second.search(&q("renamed_total"), &|| false).unwrap().total, 1);
        assert_eq!(second.definitions("brand_new").len(), 1);
    }

    #[test]
    fn indexer_and_searcher_threads() {
        let dir = workspace();
        let indexer = Indexer::new(dir.path().to_path_buf(), vec!["target".into()], None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while indexer.status().indexing || indexer.status().files == 0 {
            assert!(std::time::Instant::now() < deadline, "indexing never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(indexer.status().files, 4);
        let searcher = Searcher::new(indexer.index(), None);
        let id = searcher.search(q("compute"));
        loop {
            if let Some((rid, _, res)) = searcher.poll().into_iter().next() {
                assert_eq!(rid, id);
                assert_eq!(res.unwrap().total, 2);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
