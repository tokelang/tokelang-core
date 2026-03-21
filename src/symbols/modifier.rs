use serde::{Deserialize, Serialize};
use std::fmt;

/// Execution-style qualifiers applied to Tokelang instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Modifier {
    Simple,
    Brief,
    Detailed,
    Fast,
    Formal,
    Technical,
    Creative,
    StepByStep,
    WithExamples,
    Structured,
}

impl Modifier {
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Self::Simple => "α",
            Self::Brief => "β",
            Self::Detailed => "γ",
            Self::Fast => "δ",
            Self::Formal => "ε",
            Self::Technical => "ζ",
            Self::Creative => "η",
            Self::StepByStep => "θ",
            Self::WithExamples => "¨",
            Self::Structured => "κ",
        }
    }

    pub fn mnemonic_char(&self) -> char {
        self.mnemonic().chars().next().unwrap_or_default()
    }

    pub fn from_mnemonic(s: &str) -> Option<Self> {
        match s {
            "α" => Some(Self::Simple),
            "β" => Some(Self::Brief),
            "γ" => Some(Self::Detailed),
            "δ" => Some(Self::Fast),
            "ε" => Some(Self::Formal),
            "ζ" => Some(Self::Technical),
            "η" => Some(Self::Creative),
            "θ" => Some(Self::StepByStep),
            "¨" => Some(Self::WithExamples),
            "κ" => Some(Self::Structured),
            _ => None,
        }
    }

    pub fn expansion(&self) -> &'static str {
        match self {
            Self::Simple => "in simple terms",
            Self::Brief => "briefly",
            Self::Detailed => "in detail",
            Self::Fast => "quickly",
            Self::Formal => "in a formal tone",
            Self::Technical => "using technical language",
            Self::Creative => "creatively",
            Self::StepByStep => "step by step",
            Self::WithExamples => "with examples",
            Self::Structured => "in a structured format",
        }
    }

    pub fn all() -> &'static [Modifier] {
        &[
            Self::Simple,
            Self::Brief,
            Self::Detailed,
            Self::Fast,
            Self::Formal,
            Self::Technical,
            Self::Creative,
            Self::StepByStep,
            Self::WithExamples,
            Self::Structured,
        ]
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.mnemonic())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_roundtrip() {
        for modifier in Modifier::all() {
            assert_eq!(
                Modifier::from_mnemonic(modifier.mnemonic()),
                Some(*modifier)
            );
        }
    }
}
