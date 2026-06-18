//! # tokelang-core
//!
//! The compression engine behind **Tokelang Lite** — pragmatic English-compression middleware
//! that sits in front of a standard LLM tokenizer. Given a natural-language prompt it returns
//! either a shorter **compact** form that preserves the instruction's meaning, or — when it
//! cannot do that safely — the **original prompt unchanged**.
//!
//! The governing invariant is **safety beats savings**: when it is uncertain whether the compact
//! form is faithful, the engine passes the original through rather than risk dropping a negation,
//! a number, a path, or an instruction.
//!
//! ## Quick start
//!
//! ```
//! use tokelang_core::Engine;
//!
//! let engine = Engine::new();
//! let result = engine.compile("Please summarize the following text in three bullet points")?;
//! println!("{} ({})", result.compact, result.mode.as_str());
//! # Ok::<(), tokelang_core::EngineError>(())
//! ```
//!
//! ## Where to start reading
//!
//! - [`Engine`] — the top-level facade; `compile` / `compile_with_options` are the entry points.
//! - [`CompileResult`] / [`CompileMode`] — what a compilation returns (and whether it compressed
//!   or passed through).
//! - [`CompileOptions`] / [`InputMode`] / [`ProtectedRange`] — caller-supplied inputs. The default
//!   mode is a provably-lossless lexical fold; [`InputMode::Ir`] opts into the aggressive
//!   instruction-IR restructuring, and [`InputMode::ContextFile`] holds a higher recall floor for
//!   reused system prompts.
//! - `ARCHITECTURE.md` (repo root) — the prompt-flow walkthrough and the rationale behind the
//!   layered routing guards.

pub mod classify;
pub mod compiler;
pub mod engine;
pub mod error;
mod general_text;
pub mod ir;
mod options;
pub mod symbols;
mod token_metrics;
mod validator;

// Prompt classifier: the MEC route a prompt is dispatched to.
pub use classify::PromptRoute;
// Engine facade: the primary entry points and their result types.
pub use engine::{CompileMode, CompileResult, Engine, PassthroughDiagnostics};
pub use error::EngineError;
// Typed semantic IR: the structured representation a prompt is parsed into.
pub use ir::{
    BlockType, ContextFlags, Entity, OutputHint, Relation, RelationKind, SemanticFrame, SourceSpan,
    SurfaceProfile, TokelangBlock, TokelangIR, TokelangProgram,
};
// Caller-supplied compilation inputs.
pub use options::{CompileOptions, InputMode, ProtectedRange};
// Compact vocabulary surface.
pub use symbols::{Instruction, Modifier, OutputFormat};
pub use token_metrics::Tokenizer;
