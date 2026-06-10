use serde::{Deserialize, Serialize};

use crate::compiler::CompileError;

/// A half-open byte range `[start, end)` in the input that must survive compression verbatim.
///
/// Use for spans whose exact bytes matter — quoted literals, code, identifiers. Ranges are
/// normalized (sorted, merged, and validated against UTF-8 boundaries) before use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtectedRange {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

/// How the input is being used, which sets the engine's risk tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum InputMode {
    /// Per-call user prompts. Optimizes for savings under the safety invariant.
    #[default]
    Default,
    /// System prompts, agent personas, and RAG headers — reused across many calls, so a higher
    /// content-recall floor is enforced.
    ContextFile,
}

impl InputMode {
    /// The wire/string label for this mode: `"default"` or `"context_file"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ContextFile => "context_file",
        }
    }
}

/// Caller-supplied inputs to a compilation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CompileOptions {
    /// Byte ranges to preserve verbatim (see [`ProtectedRange`]).
    pub protected_ranges: Vec<ProtectedRange>,
    /// The input mode, which sets the recall floor (see [`InputMode`]).
    pub mode: InputMode,
}

pub(crate) fn normalize_protected_ranges(
    input: &str,
    ranges: &[ProtectedRange],
) -> Result<Vec<ProtectedRange>, CompileError> {
    if ranges.is_empty() {
        return Ok(Vec::new());
    }

    let mut normalized = ranges.to_vec();
    normalized.sort_unstable_by_key(|range| (range.start, range.end));
    normalized.dedup();

    let mut result: Vec<ProtectedRange> = Vec::with_capacity(normalized.len());

    for (index, range) in normalized.into_iter().enumerate() {
        if range.start >= range.end {
            return Err(CompileError::InvalidProtectedSpan(format!(
                "protected range {index} has start >= end"
            )));
        }
        if range.end > input.len() {
            return Err(CompileError::InvalidProtectedSpan(format!(
                "protected range {index} exceeds prompt length"
            )));
        }
        if !input.is_char_boundary(range.start) || !input.is_char_boundary(range.end) {
            return Err(CompileError::InvalidProtectedSpan(format!(
                "protected range {index} does not align to utf-8 boundaries"
            )));
        }

        if let Some(last) = result.last_mut() {
            if range.start < last.end {
                return Err(CompileError::InvalidProtectedSpan(format!(
                    "protected range {index} overlaps a previous protected range"
                )));
            }
            if range.start == last.end {
                last.end = range.end;
                continue;
            }
        }

        result.push(range);
    }

    Ok(result)
}
