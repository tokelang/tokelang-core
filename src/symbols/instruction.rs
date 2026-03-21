use serde::{Deserialize, Serialize};
use std::fmt;

/// Closed set of Tokelang instruction symbols.
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
    /// Stable single-character mnemonic for compact IR serialization.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Self::Explain => "¡",
            Self::Summarize => "¥",
            Self::Analyze => "¢",
            Self::Generate => "ƒ",
            Self::Translate => "Ð",
            Self::Compare => "¦",
            Self::Search => "¿",
            Self::Transform => "«",
            Self::List => "¤",
            Self::Define => "£",
            Self::Conclude => "Ω",
        }
    }

    pub fn mnemonic_char(&self) -> char {
        self.mnemonic().chars().next().unwrap_or_default()
    }

    /// Inverse of [`mnemonic`](Self::mnemonic).
    pub fn from_mnemonic(s: &str) -> Option<Self> {
        match s {
            "¡" => Some(Self::Explain),
            "¥" => Some(Self::Summarize),
            "¢" => Some(Self::Analyze),
            "ƒ" => Some(Self::Generate),
            "Ð" => Some(Self::Translate),
            "¦" => Some(Self::Compare),
            "¿" => Some(Self::Search),
            "«" => Some(Self::Transform),
            "¤" => Some(Self::List),
            "£" => Some(Self::Define),
            "Ω" => Some(Self::Conclude),
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
}
