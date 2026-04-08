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
            Self::Report => "report",
            Self::List => "list",
            Self::Summary => "summary",
            Self::Comparison => "comparison",
            Self::Definition => "definition",
            Self::Table => "table",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "report" => Some(Self::Report),
            "list" => Some(Self::List),
            "summary" => Some(Self::Summary),
            "comparison" => Some(Self::Comparison),
            "define" | "definition" => Some(Self::Definition),
            "table" => Some(Self::Table),
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
