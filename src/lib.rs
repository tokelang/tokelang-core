pub mod compiler;
pub mod engine;
pub mod error;
mod general_text;
pub mod ir;
mod options;
pub mod symbols;
mod token_metrics;

pub use engine::{CompileMode, CompileResult, Engine, PassthroughDiagnostics};
pub use error::EngineError;
pub use ir::{
    BlockType, ContextFlags, Entity, OutputHint, Relation, RelationKind, SemanticFrame, SourceSpan,
    SurfaceProfile, TokelangBlock, TokelangIR, TokelangProgram,
};
pub use options::{CompileOptions, ProtectedRange};
pub use symbols::{Instruction, Modifier, OutputFormat};
pub use token_metrics::Tokenizer;
