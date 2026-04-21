use crate::ir::SurfaceProfile;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Closed set of Tokelang instruction keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Instruction {
    Explain,
    Summarize,
    Analyze,
    Generate,
    Translate,
    Compare,
    Search,
    Transform,
    List,
    Define,
    Conclude,
}

impl Instruction {
    /// Stable compact keyword for IR serialization.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Self::Explain => "explain",
            Self::Summarize => "summarize",
            Self::Analyze => "analyze",
            Self::Generate => "generate",
            Self::Translate => "translate",
            Self::Compare => "compare",
            Self::Search => "search",
            Self::Transform => "transform",
            Self::List => "list",
            Self::Define => "define",
            Self::Conclude => "conclude",
        }
    }

    pub fn mnemonic_for_profile(&self, profile: SurfaceProfile) -> &'static str {
        match profile {
            SurfaceProfile::Default => self.mnemonic(),
            SurfaceProfile::Robust => self.mnemonic(),
        }
    }

    /// Inverse of [`mnemonic`](Self::mnemonic).
    pub fn from_mnemonic(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "explain" => Some(Self::Explain),
            "summarize" | "digest" => Some(Self::Summarize),
            "analyze" | "review" => Some(Self::Analyze),
            "generate" => Some(Self::Generate),
            "translate" => Some(Self::Translate),
            "compare" => Some(Self::Compare),
            "search" | "find" => Some(Self::Search),
            "transform" | "change" => Some(Self::Transform),
            "list" => Some(Self::List),
            "define" => Some(Self::Define),
            "conclude" | "finish" => Some(Self::Conclude),
            _ => None,
        }
    }

    /// Imperative verb phrase used by the runtime.
    pub fn verb_phrase(&self) -> &'static str {
        match self {
            Self::Explain => "Explain",
            Self::Summarize => "Summarize",
            Self::Analyze => "Analyze",
            Self::Generate => "Generate",
            Self::Translate => "Translate",
            Self::Compare => "Compare",
            Self::Search => "Search for",
            Self::Transform => "Transform",
            Self::List => "List",
            Self::Define => "Define",
            Self::Conclude => "Conclude with",
        }
    }

    pub fn all() -> &'static [Instruction] {
        &[
            Self::Explain,
            Self::Summarize,
            Self::Analyze,
            Self::Generate,
            Self::Translate,
            Self::Compare,
            Self::Search,
            Self::Transform,
            Self::List,
            Self::Define,
            Self::Conclude,
        ]
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.mnemonic())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_roundtrip() {
        for instruction in Instruction::all() {
            assert_eq!(
                Instruction::from_mnemonic(instruction.mnemonic()),
                Some(*instruction)
            );
        }
    }

    #[test]
    fn robust_alias_roundtrip() {
        assert_eq!(
            Instruction::from_mnemonic("review"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            Instruction::from_mnemonic("find"),
            Some(Instruction::Search)
        );
        assert_eq!(
            Instruction::from_mnemonic("digest"),
            Some(Instruction::Summarize)
        );
        assert_eq!(
            Instruction::from_mnemonic("change"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            Instruction::from_mnemonic("finish"),
            Some(Instruction::Conclude)
        );
    }
}
