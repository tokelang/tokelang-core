use serde::{Deserialize, Serialize};

/// Output shape hints extracted from natural-language prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputFormat {
    Report,
    List,
    Summary,
    Comparison,
    Definition,
    Table,
}

impl OutputFormat {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Report => "REPORT",
            Self::List => "LIST",
            Self::Summary => "SUMMARY",
            Self::Comparison => "COMPARISON",
            Self::Definition => "DEFINITION",
            Self::Table => "TABLE",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "REPORT" => Some(Self::Report),
            "LIST" => Some(Self::List),
            "SUMMARY" => Some(Self::Summary),
            "COMPARISON" => Some(Self::Comparison),
            "DEFINITION" => Some(Self::Definition),
            "TABLE" => Some(Self::Table),
            _ => None,
        }
    }

    pub fn natural_phrase(&self) -> &'static str {
        match self {
            Self::Report => "as a report",
            Self::List => "as a list",
            Self::Summary => "as a summary",
            Self::Comparison => "as a comparison",
            Self::Definition => "as a definition",
            Self::Table => "as a table",
        }
    }
}
