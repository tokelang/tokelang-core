//! Typed semantic IR and compact-format parsing for Tokelang.

mod error;
mod parser;
mod types;

pub use error::ParseError;
pub use types::{
    BlockType, ContextFlags, Entity, OutputHint, Relation, RelationKind, SemanticFrame, SourceSpan,
    SurfaceProfile, TokelangBlock, TokelangIR, TokelangProgram,
};
