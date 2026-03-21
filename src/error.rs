use crate::compiler::CompileError;
use crate::ir::ParseError;
use thiserror::Error;

/// Unified engine errors across compilation and compact parsing.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("compilation error: {0}")]
    Compile(#[from] CompileError),

    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
}
