//! # Example
//! ```
//! # use rustdoc_seeker::RustDoc;
//! # use std::fs;
//! let data = fs::read_to_string("doc-json/alloc.json")?;
//! let rustdoc: RustDoc = data.parse()?;
//! let seeker = rustdoc.build();
//!
//! let aut = fst::automaton::Levenshtein::new("dedXp", 1).unwrap();
//! assert_eq!(
//!     seeker
//!         .search(&aut)
//!         .map(|item| format!("{}", item))
//!         .collect::<Vec<_>>(),
//!     vec!["alloc/vec/struct.Vec.html#method.dedup",],
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod parser;
mod rustdoc_types;
mod seeker;

pub use parser::RustDocParseError;
pub use seeker::{DocItem, DocItemKind, RustDoc, RustDocSeeker, TypeItem};
