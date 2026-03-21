use thiserror::Error;

/// Failures during natural-language compilation.
#[derive(Debug, Error)]
pub enum CompileError {
    #[error("input is empty")]
    EmptyInput,

    #[error("no instruction could be detected in input")]
    NoInstruction,

    #[error("no semantic content could be extracted from input")]
    NoSemanticContent,
}
