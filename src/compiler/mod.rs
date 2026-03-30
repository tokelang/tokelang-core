//! Natural-language to Tokelang compiler pipeline.

mod coverage;
mod error;
pub(crate) mod normalize;
mod pipeline;
mod segment;

pub use error::CompileError;
pub use pipeline::Compiler;
