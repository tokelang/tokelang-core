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
            Self::Simple => "simple",
            Self::Brief => "brief",
            Self::Detailed => "detail",
            Self::Fast => "fast",
            Self::Formal => "professional",
            Self::Technical => "technical",
            Self::Creative => "creative",
            Self::StepByStep => "ordered",
            Self::WithExamples => "examples",
            Self::Structured => "structured",
        }
    }

    pub fn from_mnemonic(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "simple" => Some(Self::Simple),
            "brief" => Some(Self::Brief),
            "detail" | "detailed" => Some(Self::Detailed),
            "fast" => Some(Self::Fast),
            "professional" | "formal" => Some(Self::Formal),
            "technical" => Some(Self::Technical),
            "creative" => Some(Self::Creative),
            "ordered" | "steps" | "stepwise" => Some(Self::StepByStep),
            "examples" => Some(Self::WithExamples),
            "structured" => Some(Self::Structured),
            _ => None,
        }
    }

    pub fn expansion(&self) -> &'static str {
        match self {
            Self::Simple => "in simple terms",
            Self::Brief => "briefly",
            Self::Detailed => "in detail",
            Self::Fast => "quickly",
            Self::Formal => "in a professional tone",
            Self::Technical => "using technical language",
            Self::Creative => "creatively",
            Self::StepByStep => "in ordered steps",
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
