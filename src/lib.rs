//! Public API facade for the Tokelang engine.
//!
//! Wires the compiler, parser, compression, and runtime subsystems into
//! a single [`Engine`] type that exposes the full pipeline: natural
//! language in, compressed IR + expanded prompt out.
//!
//! # Example
//!
//! ```rust
//! use tokelang_core::Engine;
//!
//! let engine = Engine::new();
//!
//! let result = engine.compile("Explain quantum entanglement in simple terms").unwrap();
//! assert_eq!(result.program.to_compact(), "INP:EXP:QENT:SIMPLE");
//!
//! let prompt = engine.expand(&result.program).unwrap();
//! assert!(prompt.contains("Explain"));
//! assert!(prompt.contains("quantum entanglement"));
//! ```

mod engine;
mod error;

pub use engine::{CompileResult, Engine};
pub use error::EngineError;

pub use tokelang_compression::{CompressedIR, HuffmanTable, PrefixCodeTable};
pub use tokelang_parser::{BlockType, TokelangIR, TokelangProgram};
pub use tokelang_symbols::{Instruction, Modifier};
