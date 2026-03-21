use thiserror::Error;

/// Failures during compact-form parsing.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("compact input is empty")]
    EmptyInput,

    #[error("unknown block marker: {0}")]
    UnknownBlock(String),

    #[error("missing instruction mnemonic in line: {0}")]
    MissingInstruction(String),

    #[error("unknown instruction mnemonic: {0}")]
    UnknownInstruction(String),

    #[error("invalid sequence prefix in line: {0}")]
    InvalidSequence(String),

    #[error("invalid compact line: {0}")]
    InvalidLine(String),
}
