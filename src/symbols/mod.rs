//! Shared symbolic vocabulary for Tokelang.
//!
//! This module is the single source of truth for instruction mnemonics,
//! modifier mnemonics, output formats, subject abbreviations, synonym
//! resolution, and reserved control symbols.

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

pub const BLOCK_MARKERS: [char; 3] = ['↹', '§', 'Σ'];
pub const CONTROL_SYMBOLS: [char; 5] = ['Φ', 'Ψ', 'Ξ', '•', '→'];

/// Returns true when the character has Tokelang control meaning and therefore
/// must be escaped before user text is embedded in the IR.
pub fn is_reserved_symbol(c: char) -> bool {
    BLOCK_MARKERS.contains(&c)
        || CONTROL_SYMBOLS.contains(&c)
        || Instruction::all()
            .iter()
            .any(|instruction| instruction.mnemonic_char() == c)
        || Modifier::all()
            .iter()
            .any(|modifier| modifier.mnemonic_char() == c)
}
