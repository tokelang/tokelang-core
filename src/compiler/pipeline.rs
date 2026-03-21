use crate::compiler::coverage::{extract_coverage_items, reconcile_program};
use crate::compiler::error::CompileError;
use crate::compiler::normalize;
use crate::compiler::segment::{ClauseSpan, split_clauses};
use crate::ir::{
    BlockType, ContextFlags, Entity, OutputHint, Relation, RelationKind, SemanticFrame, SourceSpan,
    TokelangBlock, TokelangIR, TokelangProgram,
};
use crate::symbols::{Instruction, Modifier, SubjectTable, SynonymTable};

/// Natural-language prompt compiler.
pub struct Compiler {
    synonyms: SynonymTable,
    subjects: SubjectTable,
}

#[derive(Debug, Clone)]
struct MatchedEntity {
    start: usize,
    end: usize,
    surface: String,
    canonical: String,
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            synonyms: SynonymTable::default_table(),
            subjects: SubjectTable::default_table(),
        }
    }

    pub fn compile(&self, input: &str) -> Result<TokelangProgram, CompileError> {
        if input.trim().is_empty() {
            return Err(CompileError::EmptyInput);
        }

        let global_escaped = normalize::escape_reserved_symbols(input);
        let global_cleaned = normalize::clean_input(&global_escaped);
        let global_words = normalize::tokenize_words(&global_cleaned);
        let global_flags = self.detect_flags(&global_words);
        let clauses = split_clauses(input, &self.synonyms);
        let coverage_items = extract_coverage_items(input);

        let mut compiled_items = Vec::new();
        let mut active_instruction = Instruction::Explain;

        for clause in clauses {
            let escaped = normalize::escape_reserved_symbols(&clause.text);
            let cleaned = normalize::clean_input(&escaped);
            let words = normalize::tokenize_words(&cleaned);

            if let Ok(inst) = self.detect_instruction(&words) {
                active_instruction = inst;
            }

            if let Ok(ir) = self.compile_clause_with_words(&clause, &words, active_instruction) {
                compiled_items.push((clause, ir));
            }
        }

        if compiled_items.is_empty() {
            let whole_clause = ClauseSpan {
                start: 0,
                end: input.len(),
                text: input.trim().to_string(),
                marker: None,
            };
            let escaped = normalize::escape_reserved_symbols(&whole_clause.text);
            let cleaned = normalize::clean_input(&escaped);
            let words = normalize::tokenize_words(&cleaned);
            
            if let Ok(inst) = self.detect_instruction(&words) {
                active_instruction = inst;
            }
            if let Ok(ir) = self.compile_clause_with_words(&whole_clause, &words, active_instruction) {
                 compiled_items.push((whole_clause, ir));
            } else if let Ok(ir) = self.compile_clause_with_words(&whole_clause, &words, Instruction::Explain) {
                 // Absolute fallback
                 compiled_items.push((whole_clause, ir));
            } else {
                return Err(CompileError::NoSemanticContent);
            }
        }

        if let Some((_, first_item)) = compiled_items.first_mut() {
            first_item.flags.role = global_flags.role.clone();
            first_item.flags.audience = global_flags.audience.clone();
        }

        let mut program = self.assemble_program(compiled_items);
        reconcile_program(&mut program, &coverage_items);
        Ok(program)
    }

    fn assemble_program(&self, compiled_items: Vec<(ClauseSpan, TokelangIR)>) -> TokelangProgram {
        let mut program = TokelangProgram::new();
        let mut current_type = BlockType::Default;
        let mut current_block = TokelangBlock::new(BlockType::Default);
        let mut process_sequence = 1usize;

        for (_, mut item) in compiled_items {
            let target_type = self.block_type_for(item.instruction);
            if current_block.items.is_empty() {
                current_type = target_type;
                current_block = TokelangBlock::new(target_type);
            } else if current_type != target_type {
                program = program.with_block(current_block);
                current_type = target_type;
                current_block = TokelangBlock::new(target_type);
                if current_type == BlockType::Process {
                    process_sequence = 1;
                }
            }

            if current_type == BlockType::Process {
                item.sequence_id = Some(process_sequence);
                process_sequence += 1;
            } else {
                item.sequence_id = None;
            }

            current_block = current_block.add_item(item);
        }

        if !current_block.items.is_empty() {
            program = program.with_block(current_block);
        }

        program
    }

    fn compile_clause_with_words(
        &self,
        clause: &ClauseSpan,
        words: &[String],
        instruction: Instruction,
    ) -> Result<TokelangIR, CompileError> {
        if words.is_empty() {
            return Err(CompileError::NoSemanticContent);
        }

        let flags = self.detect_flags(words);
        let mut modifiers = self.detect_modifiers(words);
        let output_hint = self.detect_output_hint(words);
        let entities = self.extract_entities(words);
        let relations = self.extract_relations(words, &entities);
        let residual_terms = self.extract_residual_terms(words, &entities);

        let mut frame = SemanticFrame {
            entities: entities
                .iter()
                .map(|entity| Entity {
                    surface: entity.surface.clone(),
                    canonical: entity.canonical.clone(),
                })
                .collect(),
            relations,
            output_hint: output_hint.clone(),
            residual_terms,
        };

        if frame.entities.is_empty()
            && frame.relations.is_empty()
            && frame.output_hint.is_none()
            && frame.residual_terms.is_empty()
        {
            return Err(CompileError::NoSemanticContent);
        }

        if let Some(hint) = frame.output_hint.as_mut() {
            if hint.target.is_none() {
                if let Some(entity) = frame.entities.first() {
                    hint.target = Some(entity.canonical.clone());
                }
            }
        }

        optimize_modifiers(&mut modifiers, instruction);

        Ok(TokelangIR {
            sequence_id: None,
            instruction,
            frame,
            modifiers,
            flags,
            source_span: Some(SourceSpan {
                start: clause.start,
                end: clause.end,
            }),
            recovered_from_coverage: false,
        })
    }

    fn detect_instruction(&self, words: &[String]) -> Result<Instruction, CompileError> {
        for start in 0..words.len() {
            for width in (1..=3).rev() {
                if start + width > words.len() {
                    continue;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(instruction) = self.synonyms.resolve_instruction(&phrase) {
                    return Ok(instruction);
                }
            }
        }

        Err(CompileError::NoInstruction)
    }

    fn detect_modifiers(&self, words: &[String]) -> Vec<Modifier> {
        let mut modifiers = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for start in 0..words.len() {
            for width in (1..=3).rev() {
                if start + width > words.len() {
                    continue;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(modifier) = self.synonyms.resolve_modifier(&phrase)
                    && seen.insert(modifier)
                {
                    modifiers.push(modifier);
                }
            }
        }

        modifiers
    }

    fn detect_output_hint(&self, words: &[String]) -> Option<OutputHint> {
        let mut output_hint = OutputHint {
            format: None,
            target: None,
        };

        for start in 0..words.len() {
            for width in (1..=2).rev() {
                if start + width > words.len() {
                    continue;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(format) = self.synonyms.resolve_output_format(&phrase) {
                    output_hint.format = Some(format);
                    return Some(output_hint);
                }
            }
        }

        None
    }

    fn extract_entities(&self, words: &[String]) -> Vec<MatchedEntity> {
        let mut entities = Vec::new();
        let mut index = 0usize;

        while index < words.len() {
            let word = words[index].as_str();

            if should_skip_entity_word(word, &self.synonyms) {
                index += 1;
                continue;
            }

            if let Some(subject_match) = self.subjects.longest_match_from(words, index) {
                entities.push(MatchedEntity {
                    start: index,
                    end: index + subject_match.consumed,
                    surface: subject_match.surface,
                    canonical: subject_match.canonical,
                });
                index += subject_match.consumed;
                continue;
            }

            entities.push(MatchedEntity {
                start: index,
                end: index + 1,
                surface: word.to_string(),
                canonical: normalize::canonicalize_term(word),
            });
            index += 1;
        }

        dedupe_entities(entities)
    }

    fn extract_relations(&self, words: &[String], entities: &[MatchedEntity]) -> Vec<Relation> {
        let mut relations = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (index, word) in words.iter().enumerate() {
            let Some(kind) = relation_kind(word) else {
                continue;
            };

            let Some(previous_entity) = entities.iter().rev().find(|entity| entity.end <= index)
            else {
                continue;
            };
            let Some(next_entity) = entities.iter().find(|entity| entity.start > index) else {
                continue;
            };

            let key = (
                previous_entity.canonical.clone(),
                kind,
                next_entity.canonical.clone(),
            );
            if seen.insert(key.clone()) {
                relations.push(Relation {
                    from: key.0,
                    kind: key.1,
                    to: key.2,
                });
            }
        }

        relations
    }

    fn extract_residual_terms(&self, words: &[String], entities: &[MatchedEntity]) -> Vec<String> {
        let mut covered_indices = std::collections::HashSet::new();
        for entity in entities {
            for index in entity.start..entity.end {
                covered_indices.insert(index);
            }
        }

        let mut residuals = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (index, word) in words.iter().enumerate() {
            if covered_indices.contains(&index) || should_skip_entity_word(word, &self.synonyms) {
                continue;
            }

            if normalize::is_descriptor_word(word) || is_content_residual(word) {
                let canonical = normalize::canonicalize_term(word);
                if seen.insert(canonical.clone()) {
                    residuals.push(canonical);
                }
            }
        }

        residuals
    }

    fn detect_flags(&self, words: &[String]) -> ContextFlags {
        let text = words.join(" ");
        let role = detect_role(words);
        let audience = detect_audience(words);

        ContextFlags {
            urgent: text.contains("urgent")
                || text.contains("urgently")
                || text.contains("immediately")
                || text.contains("asap"),
            with_confidence: text.contains("confidence") || text.contains("certainty"),
            with_sources: text.contains("source")
                || text.contains("sources")
                || text.contains("citation")
                || text.contains("citations")
                || text.contains("reference")
                || text.contains("references"),
            role,
            audience,
        }
    }

    fn block_type_for(&self, instruction: Instruction) -> BlockType {
        match instruction {
            Instruction::Search => BlockType::Input,
            Instruction::Summarize
            | Instruction::Generate
            | Instruction::List
            | Instruction::Conclude => BlockType::Output,
            _ => BlockType::Process,
        }
    }
}

fn optimize_modifiers(modifiers: &mut Vec<Modifier>, instruction: Instruction) {
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for modifier in modifiers.drain(..) {
        if seen.insert(modifier) {
            deduped.push(modifier);
        }
    }

    if deduped.contains(&Modifier::Simple) && deduped.contains(&Modifier::Detailed) {
        deduped.retain(|modifier| *modifier != Modifier::Simple);
    }

    if deduped.contains(&Modifier::Brief) && deduped.contains(&Modifier::Detailed) {
        deduped.retain(|modifier| *modifier != Modifier::Brief);
    }

    if deduped.is_empty() {
        match instruction {
            Instruction::Explain | Instruction::Generate | Instruction::Conclude => {
                deduped.push(Modifier::Detailed);
            }
            _ => deduped.push(Modifier::Simple),
        }
    }

    *modifiers = deduped;
}

fn should_skip_entity_word(word: &str, synonyms: &SynonymTable) -> bool {
    normalize::is_stop_word(word)
        || synonyms.resolve_instruction(word).is_some()
        || synonyms.resolve_modifier(word).is_some()
        || synonyms.resolve_output_format(word).is_some()
        || relation_kind(word).is_some()
}

fn relation_kind(word: &str) -> Option<RelationKind> {
    match word {
        "leads" | "lead" => Some(RelationKind::LeadsTo),
        "causes" | "cause" => Some(RelationKind::Causes),
        "requires" | "require" => Some(RelationKind::Requires),
        "allows" | "allow" | "enables" | "enable" | "creates" | "create" => {
            Some(RelationKind::Enables)
        }
        _ => None,
    }
}

fn is_content_residual(word: &str) -> bool {
    matches!(
        word,
        "limitations" | "limitation" | "example" | "examples" | "assumptions" | "scenario"
    )
}

fn dedupe_entities(entities: Vec<MatchedEntity>) -> Vec<MatchedEntity> {
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for entity in entities {
        if seen.insert(entity.canonical.clone()) {
            deduped.push(entity);
        }
    }

    deduped
}

fn detect_role(words: &[String]) -> Option<String> {
    if let Some(expert_index) = words.iter().position(|word| word == "expert") {
        let role_words = words
            .iter()
            .skip(expert_index)
            .take_while(|word| !is_role_boundary(word))
            .cloned()
            .collect::<Vec<_>>();
        if !role_words.is_empty() {
            return Some(
                role_words
                    .iter()
                    .map(|word| normalize::canonicalize_term(word))
                    .collect::<Vec<_>>()
                    .join("•"),
            );
        }
    }

    if let Some(act_index) = words
        .windows(2)
        .position(|window| window[0] == "act" && window[1] == "as")
    {
        return collect_role_after(words, act_index + 2);
    }

    if let Some(acting_index) = words
        .windows(2)
        .position(|window| window[0] == "acting" && window[1] == "as")
    {
        return collect_role_after(words, acting_index + 2);
    }

    None
}

fn collect_role_after(words: &[String], start: usize) -> Option<String> {
    let offset = match words.get(start).map(String::as_str) {
        Some("a" | "an") => start + 1,
        _ => start,
    };

    let role_words = words
        .iter()
        .skip(offset)
        .take_while(|word| !is_role_boundary(word))
        .cloned()
        .collect::<Vec<_>>();

    if role_words.is_empty() {
        None
    } else {
        Some(
            role_words
                .iter()
                .map(|word| normalize::canonicalize_term(word))
                .collect::<Vec<_>>()
                .join("•"),
        )
    }
}

fn is_role_boundary(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "task"
            | "tasked"
            | "goal"
            | "who"
            | "to"
            | "for"
            | "audience"
            | "explain"
            | "analyze"
            | "summarize"
            | "generate"
            | "compare"
            | "search"
            | "translate"
            | "define"
            | "conclude"
            | "first"
            | "second"
            | "third"
            | "fourth"
            | "fifth"
            | "sixth"
            | "finally"
            | "then"
            | "next"
    )
}

fn detect_audience(words: &[String]) -> Option<String> {
    if let Some(audience_index) = words.iter().position(|word| word == "audience") {
        let audience_words = words
            .iter()
            .skip(audience_index + 1)
            .filter(|word| !matches!(word.as_str(), "that" | "consists" | "of"))
            .take_while(|word| word.as_str() != "who")
            .cloned()
            .collect::<Vec<_>>();
        if !audience_words.is_empty() {
            return Some(
                audience_words
                    .iter()
                    .map(|word| normalize::canonicalize_term(word))
                    .collect::<Vec<_>>()
                    .join("•"),
            );
        }
    }

    for marker in ["aimed", "targeted"] {
        if let Some(index) = words
            .windows(2)
            .position(|window| window[0] == marker && window[1] == "at")
        {
            let audience_words = words
                .iter()
                .skip(index + 2)
                .take_while(|word| word.as_str() != "who")
                .cloned()
                .collect::<Vec<_>>();
            if !audience_words.is_empty() {
                return Some(
                    audience_words
                        .iter()
                        .map(|word| normalize::canonicalize_term(word))
                        .collect::<Vec<_>>()
                        .join("•"),
                );
            }
        }
    }

    None
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_first_instruction_in_clause() {
        let compiler = Compiler::new();
        let words = vec![
            "discuss".to_string(),
            "how".to_string(),
            "ai".to_string(),
            "could".to_string(),
            "transform".to_string(),
            "the".to_string(),
            "labor".to_string(),
            "market".to_string(),
        ];
        assert_eq!(
            compiler.detect_instruction(&words).unwrap(),
            Instruction::Analyze
        );
    }

    #[test]
    fn extracts_relation_without_flattening() {
        let compiler = Compiler::new();
        let clause = ClauseSpan {
            start: 0,
            end: 80,
            text: "Explain why backpropagation allows the network to learn patterns from data"
                .to_string(),
            marker: None,
        };
        let escaped = normalize::escape_reserved_symbols(&clause.text);
        let cleaned = normalize::clean_input(&escaped);
        let words = normalize::tokenize_words(&cleaned);
        
        let ir = compiler.compile_clause_with_words(&clause, &words, Instruction::Explain).unwrap();
        assert!(
            ir.frame
                .relations
                .iter()
                .any(|relation| relation.from == "BACKPROPAGATION")
        );
    }
}
