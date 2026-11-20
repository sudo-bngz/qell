//! Time indexing and a few searches on a folder:
//! `cargo run --release -p qell-index --example index_bench -- <dir>`

use std::time::Instant;

use qell_index::{Indexer, Query};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: index_bench <dir>");
    let t = Instant::now();
    let indexer = Indexer::new(dir.into(), vec!["target".into(), "node_modules".into()], None);
    while indexer.status().files == 0 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    println!("searchable after {:?} ({} files)", t.elapsed(), indexer.status().files);
    while indexer.status().indexing {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let s = indexer.status();
    println!("indexed {} files, {} symbols in {:?}", s.files, s.symbols, t.elapsed());
    let index = indexer.index();
    let index = index.read().unwrap();
    for (pattern, regex) in [("fn main", false), ("HashMap::new", false), ("zzqqxx", false), (r"impl\s+Drop\s+for", true)] {
        let t = Instant::now();
        let r = index.search(&Query { pattern: pattern.into(), is_regex: regex, ..Default::default() }, &|| false).unwrap();
        println!("{pattern:>20}: {:>6} matches in {:>4} files, {:?}", r.total, r.files.len(), t.elapsed());
    }
    let t = Instant::now();
    let d = index.definitions("new");
    println!("definitions of `new`: {} in {:?}", d.len(), t.elapsed());
}
