use crate::symbols::{Instruction, Modifier, OutputFormat};
use std::collections::HashMap;

/// Rule-based synonym lookup for instructions, modifiers, and output hints.
pub struct SynonymTable {
    instruction_synonyms: HashMap<String, Instruction>,
    modifier_synonyms: HashMap<String, Modifier>,
}

impl SynonymTable {
    pub fn default_table() -> Self {
        Self {
            instruction_synonyms: HashMap::new(),
            modifier_synonyms: HashMap::new(),
        }
    }

    pub fn resolve_instruction(&self, phrase: &str) -> Option<Instruction> {
        let word = phrase.to_ascii_lowercase();
        match word.as_str() {
            "explain" | "elaborate" | "clarify" | "describe" | "illustrate" | "expound"
            | "break down" => Some(Instruction::Explain),
            "summarize" | "summarise" | "recap" | "condense" | "outline" | "overview"
            | "abstract" | "digest" | "sum up" => Some(Instruction::Summarize),
            "analyze" | "analyse" | "examine" | "evaluate" | "assess" | "inspect"
            | "investigate" | "study" | "review" | "discuss" | "simulate" | "recompute"
            | "recalculate" | "detect" | "identify" | "correlate" | "classify" | "track"
            | "cross-reference" | "cross reference" | "extract" | "audit" | "check" | "verify"
            | "run" | "read" | "score" => Some(Instruction::Analyze),
            "show" => Some(Instruction::Explain),
            "generate" | "create" | "produce" | "write" | "compose" | "draft" | "build"
            | "make" | "construct" | "propose" | "design" | "implement" => {
                Some(Instruction::Generate)
            }
            "translate" | "convert" | "interpret" | "render" => Some(Instruction::Translate),
            "compare" | "contrast" | "differentiate" | "distinguish" => Some(Instruction::Compare),
            "search" | "find" | "look up" | "lookup" | "locate" | "discover" | "collect"
            | "gather" | "capture" | "scan" | "survey" => Some(Instruction::Search),
            "transform" | "reshape" | "reformat" | "restructure" | "rework" | "compress"
            | "reconstruct" | "modify" | "filter" | "normalize" | "repair" | "redesign"
            | "optimize" | "minimize" | "preserve" | "handle" | "scale" | "remove" | "ignore"
            | "clean" | "fill" | "ensure" | "keep" | "retain" | "route" | "escalate"
            | "simplify" | "update" => Some(Instruction::Transform),
            "list" | "enumerate" | "itemize" | "catalog" => Some(Instruction::List),
            "define" | "meaning of" | "what is" | "what are" => Some(Instruction::Define),
            "conclude" | "conclusion" | "wrap up" | "finish" => Some(Instruction::Conclude),
            _ => self.instruction_synonyms.get(&word).copied(),
        }
    }

    pub fn resolve_modifier(&self, phrase: &str) -> Option<Modifier> {
        let word = phrase.to_ascii_lowercase();
        match word.as_str() {
            "simple" | "simply" | "easy" | "plain" | "basic" | "layman" => Some(Modifier::Simple),
            "brief" | "briefly" | "short" | "concise" | "succinct" => Some(Modifier::Brief),
            "detail" | "detailed" | "comprehensive" | "thorough" | "in-depth" | "complete"
            | "carefully" | "thoroughly" | "deep" | "deeply" => Some(Modifier::Detailed),
            "fast" | "quick" | "quickly" | "rapid" | "immediately" | "urgent" | "urgently"
            | "asap" => Some(Modifier::Fast),
            "formal" | "formally" | "professional" | "academic" | "business" => {
                Some(Modifier::Formal)
            }
            "technical" | "advanced" | "specialized" | "expert" => Some(Modifier::Technical),
            "creative" | "creatively" | "novel" | "innovative" | "imaginative" => {
                Some(Modifier::Creative)
            }
            "step-by-step" | "step by step" | "stepwise" | "sequential" | "ordered" => {
                Some(Modifier::StepByStep)
            }
            "examples" | "with examples" | "illustrated" | "sample" => Some(Modifier::WithExamples),
            "structured" | "formatted" | "organized" | "tabular" | "clearly" => {
                Some(Modifier::Structured)
            }
            _ => self.modifier_synonyms.get(&word).copied(),
        }
    }

    pub fn resolve_output_format(&self, phrase: &str) -> Option<OutputFormat> {
        match phrase.to_ascii_lowercase().as_str() {
            "report" => Some(OutputFormat::Report),
            "list" => Some(OutputFormat::List),
            "summary" | "summaries" => Some(OutputFormat::Summary),
            "comparison" | "compare" => Some(OutputFormat::Comparison),
            "definition" | "define" => Some(OutputFormat::Definition),
            "table" | "tabular" => Some(OutputFormat::Table),
            _ => None,
        }
    }

    pub fn starts_with_instruction(&self, words: &[String]) -> bool {
        for width in (1..=3).rev() {
            if words.len() < width {
                continue;
            }
            let phrase = words[..width].join(" ");
            if self.resolve_instruction(&phrase).is_some() {
                return true;
            }
        }
        false
    }

    pub fn register_instruction(&mut self, phrase: &str, instruction: Instruction) {
        self.instruction_synonyms
            .insert(phrase.to_lowercase(), instruction);
    }

    pub fn register_modifier(&mut self, phrase: &str, modifier: Modifier) {
        self.modifier_synonyms
            .insert(phrase.to_lowercase(), modifier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_instruction_synonyms() {
        let table = SynonymTable::default_table();
        assert_eq!(
            table.resolve_instruction("clarify"),
            Some(Instruction::Explain)
        );
        assert_eq!(
            table.resolve_instruction("contrast"),
            Some(Instruction::Compare)
        );
    }

    #[test]
    fn resolves_hard_fail_instruction_verbs() {
        let table = SynonymTable::default_table();
        assert_eq!(
            table.resolve_instruction("simulate"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("recompute"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("compress"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("reconstruct"),
            Some(Instruction::Transform)
        );
    }

    #[test]
    fn resolves_structured_workflow_verbs() {
        let table = SynonymTable::default_table();
        assert_eq!(
            table.resolve_instruction("detect"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("cross-reference"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("modify"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("optimize"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("simplify"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("update"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("propose"),
            Some(Instruction::Generate)
        );
        assert_eq!(
            table.resolve_instruction("design"),
            Some(Instruction::Generate)
        );
        assert_eq!(
            table.resolve_instruction("implement"),
            Some(Instruction::Generate)
        );
        assert_eq!(
            table.resolve_instruction("extract"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("collect"),
            Some(Instruction::Search)
        );
        assert_eq!(
            table.resolve_instruction("keep"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("route"),
            Some(Instruction::Transform)
        );
        assert_eq!(
            table.resolve_instruction("audit"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("check"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("verify"),
            Some(Instruction::Analyze)
        );
        assert_eq!(table.resolve_instruction("run"), Some(Instruction::Analyze));
        assert_eq!(
            table.resolve_instruction("read"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("score"),
            Some(Instruction::Analyze)
        );
        assert_eq!(
            table.resolve_instruction("show"),
            Some(Instruction::Explain)
        );
        assert_eq!(
            table.resolve_instruction("capture"),
            Some(Instruction::Search)
        );
        assert_eq!(table.resolve_instruction("scan"), Some(Instruction::Search));
        assert_eq!(
            table.resolve_instruction("survey"),
            Some(Instruction::Search)
        );
    }

    #[test]
    fn resolves_modifier_synonyms() {
        let table = SynonymTable::default_table();
        assert_eq!(
            table.resolve_modifier("carefully"),
            Some(Modifier::Detailed)
        );
        assert_eq!(table.resolve_modifier("urgently"), Some(Modifier::Fast));
    }

    #[test]
    fn resolves_output_formats() {
        let table = SynonymTable::default_table();
        assert_eq!(
            table.resolve_output_format("report"),
            Some(OutputFormat::Report)
        );
    }
}
