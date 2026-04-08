//! Shared compact vocabulary for Tokelang.
//!
//! This module is the single source of truth for instruction keywords,
//! modifier keywords, output formats, subject abbreviations, and synonym
//! resolution.

mod error;
mod instruction;
mod modifier;
mod output_format;
mod subject;
mod synonym;

pub use error::SymbolError;
pub use instruction::Instruction;
pub use modifier::Modifier;
pub use output_format::OutputFormat;
pub use subject::{SubjectMatch, SubjectTable};
pub use synonym::SynonymTable;

/// v0.8.0 uses a word-based surface format, so the compact output no longer
/// reserves a special non-ASCII control alphabet inside user text.
pub fn is_reserved_symbol(_c: char) -> bool {
    false
}
