use crate::compiler::Compiler;
use crate::error::EngineError;
use crate::ir::TokelangProgram;

/// Result of compiling a natural-language prompt into Tokelang.
#[derive(Debug, Clone)]
pub struct CompileResult {
    pub program: TokelangProgram,
    pub compact: String,
}

/// Top-level facade for Tokelang compilation and compact parsing.
pub struct Engine {
    compiler: Compiler,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            compiler: Compiler::new(),
        }
    }

    pub fn compile(&self, input: &str) -> Result<CompileResult, EngineError> {
        let program = self.compiler.compile(input)?;
        let compact = program.to_compact();
        Ok(CompileResult { program, compact })
    }

    pub fn parse_compact(&self, input: &str) -> Result<TokelangProgram, EngineError> {
        Ok(TokelangProgram::parse_compact(input)?)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}
