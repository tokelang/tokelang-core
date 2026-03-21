use thiserror::Error;

/// Failures during symbol resolution (mnemonic lookup, subject expansion).
#[derive(Debug, Error)]
pub enum SymbolError {
    #[error("unknown instruction mnemonic: {0}")]
    UnknownInstruction(String),

    #[error("unknown modifier mnemonic: {0}")]
    UnknownModifier(String),

    #[error("unknown subject abbreviation: {0}")]
    UnknownSubject(String),
}
