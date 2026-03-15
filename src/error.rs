use thiserror::Error;
use tokelang_compiler::CompileError;
use tokelang_parser::ParseError;
use tokelang_runtime::RuntimeError;

/// Unified error type aggregating failures from all subsystems.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("compilation error: {0}")]
    Compile(#[from] CompileError),

    #[error("parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
}
