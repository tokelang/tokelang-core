use serde::{Deserialize, Serialize};

use crate::compiler::CompileError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtectedRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CompileOptions {
    pub protected_ranges: Vec<ProtectedRange>,
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
