use fst::{
    automaton::{Levenshtein, Subsequence},
    Automaton,
};
use regex_automata::DenseDFA;
use rustdoc_seeker::RustDoc;
use std::fs;

const DOC_JSON_PATHS: [&str; 3] = [
    "doc-json/core.json",
    "doc-json/alloc.json",
    "doc-json/std.json",
];

fn main() {
    let rustdoc = DOC_JSON_PATHS
        .into_iter()
        .map(|path| {
            fs::read_to_string(path)
                .expect(&format!("Failed to read file {}", path))
                .parse()
                .expect(&format!("Failed to parse file {path}"))
        })
        .reduce(|mut all_docs: RustDoc, current_doc| {
            all_docs.extend(current_doc);
            all_docs
        })
        .expect("At least one rustdoc file must be provided");
    let seeker = rustdoc.build();

    let dfa = DenseDFA::new(".*dedup.*").unwrap();
    for i in seeker.search(&dfa) {
        println!("Regex {}", i);
    }

    let edist = Levenshtein::new("dedXp", 1).unwrap();
    for i in seeker.search(&edist) {
        println!("Edit Distance {}", i);
    }

    let subsq = Subsequence::new("dedup");
    for i in seeker.search(&subsq) {
        println!("Subsequence {}", i);
    }

    let union = subsq.union(dfa);
    for i in seeker.search(&union) {
        println!("Union {}", i);
    }

    let starts = edist.starts_with();
    for i in seeker.search(&starts) {
        println!("Starts_with {}", i);
    }
}
