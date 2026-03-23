use crate::ir::parser::parse_program;
use crate::symbols::{Instruction, Modifier, OutputFormat};
use serde::{Deserialize, Serialize};

/// Structural stage markers used by the compact Tokelang format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlockType {
    Input,
    Process,
    Output,
    #[default]
    Default,
}

impl BlockType {
    pub fn marker(&self) -> Option<&'static str> {
        match self {
            Self::Input => Some("↹"),
            Self::Process => Some("§"),
            Self::Output => Some("Σ"),
            Self::Default => None,
        }
    }

    pub fn from_marker(marker: &str) -> Option<Self> {
        match marker {
            "↹" => Some(Self::Input),
            "§" => Some(Self::Process),
            "Σ" => Some(Self::Output),
            _ => None,
        }
    }
}

/// Original source span for compiler-attributed IR items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

/// Contextual execution flags derived from the source prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFlags {
    pub urgent: bool,
    pub with_confidence: bool,
    pub with_sources: bool,
    pub role: Option<String>,
    pub audience: Option<String>,
}

impl ContextFlags {
    pub fn to_compact_prefix(&self) -> String {
        let mut parts = Vec::new();
        if let Some(role) = &self.role {
            parts.push(format!("Φ {} ", role.replace("•", " ").to_lowercase()));
        }
        if let Some(audience) = &self.audience {
            parts.push(format!("Ψ {} ", audience.replace("•", " ").to_lowercase()));
        }
        parts.join("").trim().to_string()
    }
}

/// Canonical entity reference extracted from a clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub surface: String,
    pub canonical: String,
}

/// Relation between canonical entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub from: String,
    pub kind: RelationKind,
    pub to: String,
}

/// Relation types that serialize as arrows in the compact format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationKind {
    LeadsTo,
    Causes,
    Requires,
    Enables,
    Sequence,
}

impl RelationKind {
    pub fn arrow_label(&self) -> &'static str {
        "→"
    }

    pub fn natural_phrase(&self) -> &'static str {
        match self {
            Self::LeadsTo => "leads to",
            Self::Causes => "causes",
            Self::Requires => "requires",
            Self::Enables => "enables",
            Self::Sequence => "then moves to",
        }
    }
}

/// Output-structure hint extracted from the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputHint {
    pub format: Option<OutputFormat>,
    pub target: Option<String>,
}

/// Typed semantic payload for a single instruction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticFrame {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub output_hint: Option<OutputHint>,
    pub residual_terms: Vec<String>,
}

/// Single Tokelang instruction item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokelangIR {
    pub sequence_id: Option<usize>,
    pub instruction: Instruction,
    pub frame: SemanticFrame,
    pub modifiers: Vec<Modifier>,
    pub flags: ContextFlags,
    pub source_span: Option<SourceSpan>,
    pub recovered_from_coverage: bool,
}

impl TokelangIR {
    pub fn new(instruction: Instruction) -> Self {
        Self {
            sequence_id: None,
            instruction,
            frame: SemanticFrame::default(),
            modifiers: Vec::new(),
            flags: ContextFlags::default(),
            source_span: None,
            recovered_from_coverage: false,
        }
    }

    pub fn with_sequence(mut self, sequence_id: usize) -> Self {
        self.sequence_id = Some(sequence_id);
        self
    }

    pub fn with_modifier(mut self, modifier: Modifier) -> Self {
        self.modifiers.push(modifier);
        self
    }

    pub fn with_source_span(mut self, span: SourceSpan) -> Self {
        self.source_span = Some(span);
        self
    }

    pub fn mark_recovered(mut self) -> Self {
        self.recovered_from_coverage = true;
        self
    }

    pub fn legacy_subject_chunks(&self) -> Vec<String> {
        let mut chunks = Vec::new();

        for entity in &self.frame.entities {
            if !chunks.contains(&entity.canonical) {
                chunks.push(entity.canonical.clone());
            }
        }

        for relation in &self.frame.relations {
            let chunk = format!(
                "{} {} {}",
                relation.from.replace("•", " "),
                relation.kind.arrow_label(),
                relation.to.replace("•", " ")
            );
            if !chunks.contains(&chunk) {
                chunks.push(chunk);
            }
        }

        if let Some(output_hint) = &self.frame.output_hint {
            if let Some(target) = &output_hint.target
                && !chunks.contains(target)
            {
                chunks.push(target.clone());
            }
            if let Some(format) = output_hint.format {
                let label = format.label().to_string();
                if !chunks.contains(&label) {
                    chunks.push(label);
                }
            }
        }

        for term in &self.frame.residual_terms {
            if !chunks.contains(term) {
                chunks.push(term.clone());
            }
        }

        chunks
    }

    pub fn covers_label(&self, label: &str) -> bool {
        let label_upper = label.to_uppercase();

        self.frame.entities.iter().any(|entity| {
            entity.canonical == label_upper || entity.surface.to_uppercase() == label_upper
        }) || self
            .frame
            .residual_terms
            .iter()
            .any(|term| term == &label_upper)
            || self
                .frame
                .output_hint
                .as_ref()
                .and_then(|hint| hint.format)
                .map(|format| format.label() == label_upper)
                .unwrap_or(false)
            || self
                .frame
                .relations
                .iter()
                .any(|relation| relation.from == label_upper || relation.to == label_upper)
    }

    pub fn to_compact(&self) -> String {
        let mut output = String::new();

        if let Some(sequence_id) = self.sequence_id {
            output.push_str(&format!("{sequence_id}>"));
        }

        output.push_str(self.instruction.mnemonic());

        let chunks = self.legacy_subject_chunks();
        if !chunks.is_empty() {
            output.push_str(" ");
            let subject_str = chunks.join(" ").replace("•", " ");
            output.push_str(&subject_str.to_lowercase());
        }

        if !self.modifiers.is_empty() {
            output.push_str(" ");
        }

        for modifier in &self.modifiers {
            output.push_str(modifier.mnemonic());
        }

        output
    }
}

impl std::fmt::Display for TokelangIR {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_compact())
    }
}

/// Group of related IR items under a structural block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokelangBlock {
    pub block_type: BlockType,
    pub items: Vec<TokelangIR>,
}

impl TokelangBlock {
    pub fn new(block_type: BlockType) -> Self {
        Self {
            block_type,
            items: Vec::new(),
        }
    }

    pub fn add_item(mut self, item: TokelangIR) -> Self {
        self.items.push(item);
        self
    }

    pub fn to_compact(&self) -> String {
        let items = self
            .items
            .iter()
            .map(TokelangIR::to_compact)
            .collect::<Vec<_>>()
            .join("\n");

        match self.block_type.marker() {
            Some(marker) => format!("{marker}\n{items}"),
            None => items,
        }
    }
}

/// Full Tokelang program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokelangProgram {
    pub blocks: Vec<TokelangBlock>,
}

impl TokelangProgram {
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    pub fn with_block(mut self, block: TokelangBlock) -> Self {
        self.blocks.push(block);
        self
    }

    pub fn to_compact(&self) -> String {
        let mut output = String::new();

        if let Some(first_block) = self.blocks.first()
            && let Some(first_item) = first_block.items.first()
        {
            let prefix = first_item.flags.to_compact_prefix();
            if !prefix.is_empty() {
                output.push_str(&prefix);
                output.push('\n');
            }
        }

        output.push_str(
            &self
                .blocks
                .iter()
                .map(TokelangBlock::to_compact)
                .collect::<Vec<_>>()
                .join("\n"),
        );

        output
    }

    pub fn parse_compact(input: &str) -> Result<Self, crate::ir::ParseError> {
        parse_program(input)
    }
}

impl std::fmt::Display for TokelangProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_compact())
    }
}
