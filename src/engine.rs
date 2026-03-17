use tokelang_compiler::Compiler;
use tokelang_compression::{CompressedIR, PrefixCodeTable};
use tokelang_parser::{TokelangIR, TokelangProgram, parse_compact};
use tokelang_runtime::{ExpandableIR, Runtime};
use tokelang_symbols::Modifier;

use crate::error::EngineError;

/// Output of a successful compilation through the full pipeline.
#[derive(Debug, Clone)]
pub struct CompileResult {
    /// Tokelang Program.
    pub program: TokelangProgram,
    /// Compact mnemonic form.
    pub compact: String,
    /// Compressed compact string.
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
    pub fn compile(&self, input: &str) -> Result<CompileResult, EngineError> {
        let program = self.compiler.compile(input)?;
        let compact = program.to_compact();

        let mut compressed_parts = Vec::new();
        for block in &program.blocks {
            let mut block_items = Vec::new();
            for ir in &block.items {
                let instr_code = self.instr_prefix_table.encode(ir.instruction.mnemonic()).unwrap_or(ir.instruction.mnemonic());
                let mut parts = vec![instr_code.to_string()];
                if !ir.subjects.is_empty() {
                    parts.push(ir.subjects.join("•"));
                }
                for m in &ir.modifiers {
                    let m_code = self.mod_prefix_table.encode(m.mnemonic()).unwrap_or(m.mnemonic());
                    parts.push(m_code.to_string());
                }
                block_items.push(parts.join(":"));
            }
            
            let mut prefix = String::new();
            if block.block_type != tokelang_parser::BlockType::Default {
                prefix = format!("{}:", block.block_type.mnemonic());
            }
            
            compressed_parts.push(format!("{}{}", prefix, block_items.join(",")));
        }
        
        let compressed_compact = compressed_parts.join("\n");

        Ok(CompileResult {
            program,
            compact,
            compressed_compact,
        })
    }

    /// Parse a compact Tokelang string into IR.
    pub fn parse(&self, compact: &str) -> Result<TokelangIR, EngineError> {
        Ok(parse_compact(compact)?)
    }

    /// Expand Tokelang IR back into a natural-language prompt.
    pub fn expand(&self, program: &TokelangProgram) -> Result<String, EngineError> {
        let mut expansions = Vec::new();
        for block in &program.blocks {
            for ir in &block.items {
                let expandable = ExpandableIR {
                    instruction: ir.instruction,
                    subject: ir.subjects.join(" "),
                    modifiers: ir.modifiers.clone(),
                    urgent: ir.flags.urgent,
                    with_confidence: ir.flags.with_confidence,
                    with_sources: ir.flags.with_sources,
                };
                expansions.push(self.runtime.expand(&expandable)?);
            }
        }
        
        Ok(expansions.join(" "))
    }

    /// Full round-trip: natural language -> IR -> expanded prompt.
    pub fn round_trip(&self, input: &str) -> Result<String, EngineError> {
        let result = self.compile(input)?;
        self.expand(&result.program)
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
