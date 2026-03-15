use tokelang_compiler::Compiler;
use tokelang_compression::{CompressedIR, PrefixCodeTable};
use tokelang_parser::{TokelangIR, parse_compact};
use tokelang_runtime::{ExpandableIR, Runtime};
use tokelang_symbols::Modifier;

use crate::error::EngineError;

/// Output of a successful compilation through the full pipeline.
#[derive(Debug, Clone)]
pub struct CompileResult {
    /// Tokelang IR.
    pub ir: TokelangIR,
    /// Compact mnemonic form (e.g. `EXP:QENT:SIMPLE`).
    pub compact: String,
    /// Prefix-coded compressed IR.
    pub compressed: CompressedIR,
    /// Compressed compact string (e.g. `A:QENT:A`).
    pub compressed_compact: String,
}

/// Top-level engine coordinating the compiler, parser, compression, and
/// runtime subsystems.
pub struct Engine {
    compiler: Compiler,
    runtime: Runtime,
    instr_prefix_table: PrefixCodeTable,
    mod_prefix_table: PrefixCodeTable,
}

impl Engine {
    /// Create an engine with default symbol tables and prefix code tables.
    pub fn new() -> Self {
        let instr_prefix_table = PrefixCodeTable::default_instruction_table();

        let mod_freqs: Vec<(String, u32)> = Modifier::all()
            .iter()
            .map(|m| (m.mnemonic().to_string(), m.base_frequency()))
            .collect();
        let mod_prefix_table = PrefixCodeTable::build(mod_freqs);

        Self {
            compiler: Compiler::new(),
            runtime: Runtime::new(),
            instr_prefix_table,
            mod_prefix_table,
        }
    }

    /// Compile a natural-language prompt through the full pipeline.
    ///
    /// Stages 1-6 (compiler) produce the IR; stage 7 (compression)
    /// produces the prefix-coded form.
    pub fn compile(&self, input: &str) -> Result<CompileResult, EngineError> {
        let ir = self.compiler.compile(input)?;
        let compact = ir.to_compact();

        let compressed = CompressedIR::compress(
            ir.instruction,
            &ir.subject,
            &ir.modifiers,
            &self.instr_prefix_table,
            &self.mod_prefix_table,
        );
        let compressed_compact = compressed.to_compact();

        Ok(CompileResult {
            ir,
            compact,
            compressed,
            compressed_compact,
        })
    }

    /// Parse a compact Tokelang string into IR.
    pub fn parse(&self, compact: &str) -> Result<TokelangIR, EngineError> {
        Ok(parse_compact(compact)?)
    }

    /// Expand Tokelang IR back into a natural-language prompt.
    pub fn expand(&self, ir: &TokelangIR) -> Result<String, EngineError> {
        let expandable = ExpandableIR {
            instruction: ir.instruction,
            subject: ir.subject.clone(),
            modifiers: ir.modifiers.clone(),
            urgent: ir.flags.urgent,
            with_confidence: ir.flags.with_confidence,
            with_sources: ir.flags.with_sources,
        };
        Ok(self.runtime.expand(&expandable)?)
    }

    /// Full round-trip: natural language -> IR -> expanded prompt.
    pub fn round_trip(&self, input: &str) -> Result<String, EngineError> {
        let result = self.compile(input)?;
        self.expand(&result.ir)
    }

    /// Access the instruction prefix code table.
    pub fn instruction_codes(&self) -> &PrefixCodeTable {
        &self.instr_prefix_table
    }

    /// Access the modifier prefix code table.
    pub fn modifier_codes(&self) -> &PrefixCodeTable {
        &self.mod_prefix_table
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokelang_symbols::Instruction;

    #[test]
    fn compile_explain_quantum_simple() {
        let engine = Engine::new();
        let result = engine
            .compile("Explain quantum entanglement in simple terms")
            .unwrap();

        assert_eq!(result.ir.instruction, Instruction::Explain);
        assert_eq!(result.ir.subject, "QENT");
        assert_eq!(result.compact, "EXP:QENT:SIMPLE");
        assert!(result.compressed_compact.len() <= result.compact.len());
    }

    #[test]
    fn compile_summarize_article_fast() {
        let engine = Engine::new();
        let result = engine.compile("Summarize this article quickly").unwrap();

        assert_eq!(result.ir.instruction, Instruction::Summarize);
        assert_eq!(result.ir.subject, "ARTICLE");
        assert_eq!(result.compact, "SUM:ARTICLE:FAST");
    }

    #[test]
    fn expand_from_compact() {
        let engine = Engine::new();
        let ir = engine.parse("EXP:QENT:SIMPLE").unwrap();
        let prompt = engine.expand(&ir).unwrap();

        assert!(prompt.contains("Explain"));
        assert!(prompt.contains("quantum entanglement"));
        assert!(prompt.contains("in simple terms"));
    }

    #[test]
    fn full_round_trip() {
        let engine = Engine::new();
        let prompt = engine
            .round_trip("Explain quantum entanglement in simple terms")
            .unwrap();

        assert!(prompt.contains("Explain"));
        assert!(prompt.contains("quantum entanglement"));
        assert!(prompt.contains("in simple terms"));
    }

    #[test]
    fn parse_and_expand_analyze() {
        let engine = Engine::new();
        let ir = engine.parse("ANL:DATA:DETAIL:FAST").unwrap();
        let prompt = engine.expand(&ir).unwrap();

        assert!(prompt.contains("Analyze"));
        assert!(prompt.contains("data"));
        assert!(prompt.contains("in detail"));
        assert!(prompt.contains("quickly"));
    }

    #[test]
    fn compression_saves_space() {
        let engine = Engine::new();
        let result = engine
            .compile("Explain neural networks in simple terms with examples")
            .unwrap();

        let original_len = result.compact.len();
        let compressed_len = result.compressed_compact.len();

        assert!(
            compressed_len <= original_len,
            "compressed ({compressed_len}) should be <= original ({original_len}): {} vs {}",
            result.compressed_compact,
            result.compact
        );
    }

    #[test]
    fn elaborate_synonym_works() {
        let engine = Engine::new();
        let result = engine.compile("Elaborate on neural networks").unwrap();
        assert_eq!(result.ir.instruction, Instruction::Explain);
    }

    #[test]
    fn empty_input_error() {
        let engine = Engine::new();
        assert!(engine.compile("").is_err());
    }
}
