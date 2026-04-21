use crate::compiler::normalize;
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
            Self::Input => Some("input"),
            Self::Process => Some("process"),
            Self::Output => Some("output"),
            Self::Default => None,
        }
    }

    pub fn from_marker(marker: &str) -> Option<Self> {
        match marker.trim().to_ascii_lowercase().as_str() {
            "input" => Some(Self::Input),
            "process" => Some(Self::Process),
            "output" => Some(Self::Output),
            _ => None,
        }
    }
}

/// Surface emission profile for compact Tokelang output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SurfaceProfile {
    #[default]
    Default,
    Robust,
}

impl SurfaceProfile {
    pub fn rewrite_action_token(self, token: &str) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Robust => match token {
                "return" => Some("write"),
                _ => None,
            },
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
        String::new()
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
            Self::LeadsTo => "leads",
            Self::Causes => "causes",
            Self::Requires => "requires",
            Self::Enables => "enables",
            Self::Sequence => "then",
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
    pub literal_islands: Vec<String>,
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
    pub compact_override: Option<String>,
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
            compact_override: None,
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
                relation.kind.natural_phrase(),
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
                let label = format!("shape {}", format.label());
                if !chunks.contains(&label) {
                    chunks.push(label);
                }
            }
        }

        for literal in &self.frame.literal_islands {
            let chunk = format!("`{literal}`");
            if !chunks.contains(&chunk) {
                chunks.push(chunk);
            }
        }

        for term in &self.frame.residual_terms {
            if !chunks.contains(term) {
                chunks.push(term.clone());
            }
        }

        chunks
    }

    pub fn default_compact(&self) -> String {
        self.default_compact_for_profile(SurfaceProfile::Default, true)
    }

    pub fn default_compact_without_sequence(&self) -> String {
        self.default_compact_for_profile(SurfaceProfile::Default, false)
    }

    fn default_compact_for_profile(
        &self,
        profile: SurfaceProfile,
        include_sequence: bool,
    ) -> String {
        let mut parts = Vec::new();

        if include_sequence && let Some(sequence_id) = self.sequence_id {
            parts.push(sequence_id.to_string());
        }

        parts.push(self.instruction.mnemonic_for_profile(profile).to_string());

        let chunks = self.legacy_subject_chunks();
        if !chunks.is_empty() {
            let subject_str = chunks
                .into_iter()
                .map(|chunk| {
                    if chunk.starts_with('`') && chunk.ends_with('`') {
                        chunk
                    } else {
                        chunk.replace("•", " ").to_lowercase()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            parts.push(subject_str);
        }

        for modifier in &self.modifiers {
            parts.push(modifier.mnemonic().to_string());
        }

        parts.join(" ")
    }

    pub fn covers_label(&self, label: &str) -> bool {
        let label_upper = label.to_uppercase();

        self.frame.entities.iter().any(|entity| {
            entity.canonical == label_upper || entity.surface.to_uppercase() == label_upper
        }) || self
            .frame
            .literal_islands
            .iter()
            .any(|literal| literal.to_uppercase().contains(&label_upper))
            || self
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
        self.to_compact_with_profile_and_sequence(SurfaceProfile::Default, true)
    }

    pub fn to_compact_with_profile(&self, profile: SurfaceProfile) -> String {
        self.to_compact_with_profile_and_sequence(profile, true)
    }

    pub fn to_compact_with_sequence(&self, include_sequence: bool) -> String {
        self.to_compact_with_profile_and_sequence(SurfaceProfile::Default, include_sequence)
    }

    pub fn to_compact_with_profile_and_sequence(
        &self,
        profile: SurfaceProfile,
        include_sequence: bool,
    ) -> String {
        if let Some(compact_override) = &self.compact_override {
            let trimmed = compact_override.trim();
            let body = if trimmed
                .split_whitespace()
                .next()
                .is_some_and(|token| token.chars().all(|ch| ch.is_ascii_digit()))
            {
                trimmed
                    .split_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                trimmed.to_string()
            };
            let body = rewrite_compact_body_for_profile(body, profile);
            let body = decorate_compact_body_with_literals(body, &self.frame.literal_islands);

            if include_sequence {
                if let Some(sequence_id) = self.sequence_id {
                    format!("{sequence_id} {body}")
                } else {
                    body
                }
            } else {
                body
            }
        } else {
            self.default_compact_for_profile(profile, include_sequence)
        }
    }

    pub fn to_compact_with_defaults(
        &self,
        profile: SurfaceProfile,
        include_sequence: bool,
        inherited_modifier: Option<Modifier>,
    ) -> String {
        let mut line = self.to_compact_with_profile_and_sequence(profile, include_sequence);
        if let Some(modifier) = inherited_modifier
            && self.modifiers.len() == 1
            && self.modifiers[0] == modifier
        {
            line = strip_trailing_modifier_suffix(line, modifier);
        }
        line
    }
}

impl std::fmt::Display for TokelangIR {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_compact())
    }
}

fn rewrite_compact_body_for_profile(body: String, profile: SurfaceProfile) -> String {
    if profile == SurfaceProfile::Default && !body.contains("goto ") {
        return body;
    }

    let mut tokens = body
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();

    if profile != SurfaceProfile::Default {
        if let Some(first) = tokens.first_mut()
            && let Some(alias) = profile.rewrite_action_token(first.as_str())
        {
            *first = alias.to_string();
        }

        for index in 0..tokens.len().saturating_sub(1) {
            if tokens[index] == "else"
                && let Some(alias) = profile.rewrite_action_token(tokens[index + 1].as_str())
            {
                tokens[index + 1] = alias.to_string();
            }
        }
    }

    apply_lexical_joins(&mut tokens);
    tokens.join(" ")
}

fn apply_lexical_joins(tokens: &mut Vec<String>) {
    let mut index = 0usize;
    while index + 1 < tokens.len() {
        if tokens[index] == "goto"
            && tokens[index + 1]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            let target = tokens.remove(index + 1);
            tokens[index] = format!("goto{target}");
        } else {
            index += 1;
        }
    }
}

fn strip_trailing_modifier_suffix(line: String, modifier: Modifier) -> String {
    let suffix = format!(" {}", modifier.mnemonic());
    if line.ends_with(&suffix) {
        line[..line.len() - suffix.len()].to_string()
    } else {
        line
    }
}

fn decorate_compact_body_with_literals(body: String, literal_islands: &[String]) -> String {
    let mut decorated = body;
    let mut literals = literal_islands
        .iter()
        .filter(|literal| !literal.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    literals.sort_by_key(|literal| std::cmp::Reverse(literal.len()));

    for literal in literals {
        let wrapped = format!("`{literal}`");
        if decorated.contains(&wrapped) {
            continue;
        }

        if decorated.contains(&literal) {
            decorated = decorated.replace(&literal, &wrapped);
            continue;
        }

        let cleaned = normalize::clean_input(&literal);
        if !cleaned.is_empty() && decorated.contains(&cleaned) {
            decorated = decorated.replace(&cleaned, &wrapped);
        }
    }

    decorated
}

/// Group of related IR items under a structural block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokelangBlock {
    pub block_type: BlockType,
    pub default_modifier: Option<Modifier>,
    pub items: Vec<TokelangIR>,
}

impl TokelangBlock {
    pub fn new(block_type: BlockType) -> Self {
        Self {
            block_type,
            default_modifier: None,
            items: Vec::new(),
        }
    }

    pub fn with_default_modifier(mut self, modifier: Modifier) -> Self {
        self.default_modifier = Some(modifier);
        self
    }

    pub fn add_item(mut self, item: TokelangIR) -> Self {
        self.items.push(item);
        self
    }

    fn effective_default_modifier(&self) -> Option<Modifier> {
        self.default_modifier
            .or_else(|| detect_block_default_modifier(&self.items))
    }

    pub fn to_compact(&self) -> String {
        self.to_compact_with_profile(SurfaceProfile::Default)
    }

    pub fn to_compact_with_profile(&self, profile: SurfaceProfile) -> String {
        self.to_compact_with_options(profile, false, false)
    }

    pub fn to_compact_with_options(
        &self,
        profile: SurfaceProfile,
        preserve_process_sequence: bool,
        preserve_output_sequence: bool,
    ) -> String {
        let include_sequence = match self.block_type {
            BlockType::Process => preserve_process_sequence,
            BlockType::Output => preserve_output_sequence,
            _ => false,
        };
        let default_modifier = self.effective_default_modifier();
        let mut lines = self
            .items
            .iter()
            .map(|item| item.to_compact_with_defaults(profile, include_sequence, default_modifier))
            .collect::<Vec<_>>();

        if self.block_type == BlockType::Process {
            lines = merge_adjacent_if_else_lines(lines);
        }

        let items = lines.join("\n");
        let default_line =
            default_modifier.map(|modifier| format!("default {}", modifier.mnemonic()));

        match (self.block_type.marker(), default_line) {
            (Some(marker), Some(default_line)) => format!("{marker}\n{default_line}\n{items}"),
            (Some(marker), None) => format!("{marker}\n{items}"),
            (None, Some(default_line)) => format!("{default_line}\n{items}"),
            (None, None) => items,
        }
    }
}

fn detect_block_default_modifier(items: &[TokelangIR]) -> Option<Modifier> {
    if items.len() < 2 {
        return None;
    }

    let modifiers = items
        .iter()
        .map(explicit_surface_modifier)
        .collect::<Option<Vec<_>>>()?;

    let mut counts = std::collections::HashMap::<Modifier, usize>::new();
    for modifier in modifiers {
        *counts.entry(modifier).or_insert(0) += 1;
    }

    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    let (best_modifier, best_count) = *ranked.first()?;
    let second_count = ranked.get(1).map(|(_, count)| *count).unwrap_or(0);

    if best_count >= 2 && best_count > second_count {
        Some(best_modifier)
    } else {
        None
    }
}

fn explicit_surface_modifier(item: &TokelangIR) -> Option<Modifier> {
    let line = item.to_compact_with_sequence(false);
    let tail = line.split_whitespace().last()?;
    Modifier::from_mnemonic(tail)
}

fn merge_adjacent_if_else_lines(lines: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        if let Some(current) = lines.get(index)
            && let Some(next) = lines.get(index + 1)
            && let Some((prefix, if_body)) = split_leading_sequence(current)
            && let Some((_, else_body)) = split_leading_sequence(next)
            && if_body.starts_with("if ")
            && else_body.starts_with("else ")
        {
            merged.push(format!("{prefix}{if_body} {}", else_body));
            index += 2;
            continue;
        }

        merged.push(lines[index].clone());
        index += 1;
    }

    merged
}

fn split_leading_sequence(line: &str) -> Option<(String, &str)> {
    let (first, rest) = line.split_once(' ')?;
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        Some((format!("{first} "), rest))
    } else {
        Some((String::new(), line))
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
        self.to_compact_with_profile(SurfaceProfile::Default)
    }

    pub fn to_compact_with_profile(&self, profile: SurfaceProfile) -> String {
        let mut output = String::new();
        let preserve_numbered_targets = self.blocks.iter().any(|block| {
            block.items.iter().any(|item| {
                item.compact_override.as_deref().is_some_and(|line| {
                    contains_goto_reference(line) || contains_step_reference(line)
                })
            })
        });

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
                .map(|block| {
                    block.to_compact_with_options(
                        profile,
                        preserve_numbered_targets,
                        preserve_numbered_targets,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );

        output
    }

    pub fn parse_compact(input: &str) -> Result<Self, crate::ir::ParseError> {
        parse_program(input)
    }
}

fn contains_step_reference(line: &str) -> bool {
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    tokens
        .windows(2)
        .any(|window| window[0] == "step" && window[1].chars().all(|ch| ch.is_ascii_digit()))
}

fn contains_goto_reference(line: &str) -> bool {
    line.split_whitespace().any(|token| {
        token == "goto"
            || token.strip_prefix("goto").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
            })
    })
}

impl std::fmt::Display for TokelangProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_compact())
    }
}
