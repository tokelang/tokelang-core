use crate::compiler::Compiler;
use crate::compiler::normalize;
use crate::error::EngineError;
use crate::ir::TokelangProgram;
use crate::token_metrics::Tokenizer;

const MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG: f64 = 15.0;

/// Whether the final returned output is Tokelang IR or the original prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileMode {
    Tokelang,
    Passthrough,
}

impl CompileMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tokelang => "tokelang",
            Self::Passthrough => "passthrough",
        }
    }
}

/// Result of compiling a natural-language prompt into Tokelang.
#[derive(Debug, Clone)]
pub struct CompileResult {
    pub program: TokelangProgram,
    pub compact: String,
    pub mode: CompileMode,
}

/// Local diagnostics for eval tooling and passthrough analysis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassthroughDiagnostics {
    pub protected_chars: usize,
    pub total_chars: usize,
    pub protected_ratio_pct: f64,
    pub natural_language_word_count: usize,
    pub protected_content_passthrough: bool,
    pub passthrough_threshold_pct: f64,
}

/// Top-level facade for Tokelang compilation and compact parsing.
pub struct Engine {
    compiler: Compiler,
    tokenizer: Tokenizer,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            compiler: Compiler::new(),
            tokenizer: Tokenizer::detect(),
        }
    }

    pub fn compile(&self, input: &str) -> Result<CompileResult, EngineError> {
        let program = self.compiler.compile(input)?;
        let tokelang_compact = program.to_compact();
        let mode = if self.protected_content_demands_passthrough(input) {
            CompileMode::Passthrough
        } else {
            self.output_mode(input, &tokelang_compact)
        };
        let compact = match mode {
            CompileMode::Tokelang => tokelang_compact,
            CompileMode::Passthrough => input.to_string(),
        };
        Ok(CompileResult {
            program,
            compact,
            mode,
        })
    }

    pub fn parse_compact(&self, input: &str) -> Result<TokelangProgram, EngineError> {
        Ok(TokelangProgram::parse_compact(input)?)
    }

    pub fn passthrough_threshold_pct(&self) -> f64 {
        MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG
    }

    pub fn passthrough_diagnostics(&self, input: &str) -> PassthroughDiagnostics {
        let stats = normalize::protected_content_stats(input);
        let stripped = normalize::strip_protected_content(input);
        let escaped = normalize::escape_reserved_symbols(&stripped);
        let cleaned = normalize::clean_input(&escaped);
        let words = normalize::tokenize_words(&cleaned);
        let protected_ratio_pct = if stats.total_chars == 0 {
            0.0
        } else {
            stats.protected_chars as f64 * 100.0 / stats.total_chars as f64
        };

        PassthroughDiagnostics {
            protected_chars: stats.protected_chars,
            total_chars: stats.total_chars,
            protected_ratio_pct,
            natural_language_word_count: words.len(),
            protected_content_passthrough: stats.total_chars != 0
                && stats.protected_chars != 0
                && stats.protected_chars * 100 >= stats.total_chars * 40
                && words.len() < 16,
            passthrough_threshold_pct: MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG,
        }
    }

    fn output_mode(&self, input: &str, tokelang_compact: &str) -> CompileMode {
        let original_tokens = self.tokenizer.count(input);
        if original_tokens == 0 {
            return CompileMode::Tokelang;
        }

        let compact_tokens = self.tokenizer.count(tokelang_compact);
        let reduction_pct =
            (original_tokens as f64 - compact_tokens as f64) / original_tokens as f64 * 100.0;

        if reduction_pct <= MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG {
            CompileMode::Passthrough
        } else {
            CompileMode::Tokelang
        }
    }

    fn protected_content_demands_passthrough(&self, input: &str) -> bool {
        let diagnostics = self.passthrough_diagnostics(input);
        let lowered = input.to_ascii_lowercase();
        diagnostics.protected_content_passthrough
            || demands_fenced_code_passthrough(input, diagnostics.natural_language_word_count)
            || demands_math_passthrough(input, diagnostics.natural_language_word_count)
            || demands_row_heavy_passthrough(
                input,
                &lowered,
                diagnostics.natural_language_word_count,
            )
            || demands_contract_sensitive_passthrough(input, diagnostics.natural_language_word_count)
    }
}

fn demands_fenced_code_passthrough(input: &str, natural_language_word_count: usize) -> bool {
    if !input.contains("```") || natural_language_word_count >= 48 {
        return false;
    }

    let lowered = input.to_ascii_lowercase();
    let explicit_code_context = [
        "code",
        "function",
        "sql",
        "query",
        "algorithm",
        "bug",
        "traceback",
        "stack trace",
        "exception",
        "compile error",
    ]
    .iter()
    .any(|needle| lowered.contains(needle));

    explicit_code_context || !has_workflow_scaffold(input)
}

fn demands_math_passthrough(input: &str, natural_language_word_count: usize) -> bool {
    if natural_language_word_count >= 56 || has_workflow_scaffold(input) {
        return false;
    }

    let lowered = input.to_ascii_lowercase();
    let equation_payload = input
        .lines()
        .map(str::trim)
        .any(|line| normalize::is_equation_heavy_line(line) || looks_like_inline_equation(line));
    let exactness_hits = count_contains(
        &lowered,
        &[
        "solve this exactly",
        "solve exactly",
        "exact solution",
        "exactly",
        "symbolic derivation",
        "symbolic proof",
        "show all steps",
        "keep the symbolic derivation explicit",
        "do not approximate",
        "without approximation",
        "preserve the algebra",
        ],
    );
    let math_action_hits = count_contains(
        &lowered,
        &[
        "solve",
        "compute",
        "find",
        "derive",
        "differentiate",
        "integrate",
        "factor",
        "simplify",
        "project",
        "maximize",
        "minimize",
        "prove",
        "classify",
        ],
    );
    let math_topic_hits = count_contains(
        &lowered,
        &[
        "critical points",
        "local extrema",
        "eigenvalue",
        "characteristic polynomial",
        "lagrange multiplier",
        "constraint",
        "confidence interval",
        "margin of error",
        "sample mean",
        "variance",
        "standard deviation",
        "median",
        "triangle",
        "similarity",
        "angle",
        "projection",
        "dot product",
        "probability",
        "conditional probability",
        "expected value",
        "recurrence",
        "closed form",
        "induction",
        "matrix",
        "spectrum",
        "root",
        "roots",
        "calculus",
        "derivative",
        "integral",
        "factorization",
        ],
    );

    (equation_payload && (math_action_hits >= 1 || exactness_hits >= 1 || math_topic_hits >= 2))
        || (math_action_hits >= 1 && math_topic_hits >= 1)
        || (exactness_hits >= 1 && math_topic_hits >= 1)
        || math_topic_hits >= 2
}

fn demands_contract_sensitive_passthrough(input: &str, natural_language_word_count: usize) -> bool {
    if natural_language_word_count >= 32 || has_workflow_scaffold(input) {
        return false;
    }

    let lowered = input.to_ascii_lowercase();
    let rewrite_hits = count_contains(&lowered, &["rewrite", "adapt the tone", "adapt tone"]);
    let translation_hits = count_contains(&lowered, &["translate"]);
    let extraction_hits = count_contains(&lowered, &["extract", "normalize"])
        + count_contains(&lowered, &[" fields", " field", "invoice number", "due date", "sender", "subject"]);
    let search_hits = count_contains(&lowered, &["sources", "citations", "literature", "research gap", "themes"]);
    let tutoring_artifact_hits = count_contains(&lowered, &["quiz", "answer key", "hint plan", "scaffolding"]);
    let contract_hits = count_contains(
        &lowered,
        &[
            "return only",
            "return the",
            "keep",
            "preserve",
            "include",
            "do not",
            "avoid",
            "intact",
            "friendly",
            "formal",
            "polite",
            "concise",
            "reassuring",
            "direct",
            "citations",
            "answer key",
            "hint",
            "themes",
            "gap",
            "only",
        ],
    );

    (translation_hits >= 1 && contract_hits >= 1)
        || (rewrite_hits >= 1 && contract_hits >= 2)
        || (extraction_hits >= 2 && contract_hits >= 1)
        || (search_hits >= 1 && contract_hits >= 2)
        || (tutoring_artifact_hits >= 1 && contract_hits >= 2)
}

fn demands_row_heavy_passthrough(
    input: &str,
    lowered: &str,
    natural_language_word_count: usize,
) -> bool {
    let tuple_rows = input
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('(') && line.ends_with(')') && line.matches(',').count() >= 2)
        .count();
    if tuple_rows < 2 {
        return false;
    }

    if has_workflow_scaffold(input) {
        return natural_language_word_count < 32 && lowered.contains("evidence:");
    }

    let exactness_hits = count_contains(
        lowered,
        &[
            "rows:",
            "data:",
            "keep the row values intact",
            "row values intact",
            "compact table",
            "fields",
            "return a short research note",
            "return the",
        ],
    );

    exactness_hits >= 2
}

fn looks_like_inline_equation(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.contains('=') {
        return false;
    }

    let has_digit = trimmed.chars().any(|character| character.is_ascii_digit());
    let has_operator = trimmed
        .chars()
        .any(|character| matches!(character, '+' | '-' | '*' | '/' | '^' | '='));
    let has_symbolic_variable = trimmed
        .chars()
        .any(|character| matches!(character, 'x' | 'y' | 'z'));

    has_operator && (has_digit || has_symbolic_variable)
}

fn has_workflow_scaffold(input: &str) -> bool {
    let lowered = input.to_ascii_lowercase();
    if ["tasks:", "workflow:", "step 1", "phase 1", "stage 1", "then:"]
        .iter()
        .any(|needle| lowered.contains(needle))
    {
        return true;
    }

    input.lines()
        .filter(|line| looks_like_list_line(line.trim_start()))
        .take(2)
        .count()
        >= 2
}

fn looks_like_list_line(line: &str) -> bool {
    if line.starts_with("- ") || line.starts_with("* ") {
        return true;
    }

    let mut chars = line.chars().peekable();
    let mut saw_digit = false;
    while matches!(chars.peek(), Some(character) if character.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }

    saw_digit && matches!(chars.next(), Some('.' | ')'))
}

fn count_contains(text: &str, needles: &[&str]) -> usize {
    needles.iter().filter(|needle| text.contains(**needle)).count()
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CompileMode, Engine};

    #[test]
    fn keeps_tokelang_output_when_token_savings_clear_threshold() {
        let engine = Engine::new();
        let prompt = "First, search for the Q1 sales data in the database. Then, carefully analyze the data for emerging trends. Finally, summarize the trends in a detailed report.";

        let result = engine
            .compile(prompt)
            .expect("baseline prompt should compile");

        assert_eq!(result.mode, CompileMode::Tokelang);
        assert_ne!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_original_prompt_when_token_savings_are_too_small() {
        let engine = Engine::new();
        let prompt = "Explain AI in depth.";

        let result = engine.compile(prompt).expect("short prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
        assert!(
            !result.program.blocks.is_empty(),
            "fallback should still retain the attempted compiled program for internal use"
        );
    }

    #[test]
    fn falls_back_to_passthrough_for_code_dominated_prompt() {
        let engine = Engine::new();
        let prompt = r#"Explain this code:

```python
def moving_average(xs, window):
    total = 0
    out = []
    for i, x in enumerate(xs):
        total += x
        if i >= window:
            total -= xs[i - window]
        out.append(total / window)
    return out
```"#;

        let result = engine
            .compile(prompt)
            .expect("code-dominated prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn passthrough_diagnostics_capture_protected_ratio_and_natural_language_words() {
        let engine = Engine::new();
        let prompt = r#"Explain this formula:

$$
f(x) = 3x^2 - 2x + 7
$$

Keep the intuition brief."#;

        let diagnostics = engine.passthrough_diagnostics(prompt);

        assert!(diagnostics.total_chars > 0);
        assert!(diagnostics.protected_chars > 0);
        assert!(diagnostics.protected_ratio_pct > 0.0);
        assert!(diagnostics.natural_language_word_count > 0);
        assert_eq!(
            diagnostics.passthrough_threshold_pct,
            engine.passthrough_threshold_pct()
        );
    }

    #[test]
    fn falls_back_to_passthrough_for_exact_symbolic_math_prompt() {
        let engine = Engine::new();
        let prompt = r#"Solve this exactly:

Let f(x) = x^4 - 6x^2 + 8x - 3.
Find all critical points, classify them, and report the local extrema.
Keep the symbolic derivation explicit."#;

        let result = engine
            .compile(prompt)
            .expect("exact symbolic math prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_fenced_code_review_prompt() {
        let engine = Engine::new();
        let prompt = r#"Explain the lock-ordering problem and keep the concurrency terms.

```go
muA.Lock()
muB.Lock()
// work
muB.Unlock()
muA.Unlock()
```

State what deadlock risk exists if another goroutine takes the locks in reverse order."#;

        let result = engine
            .compile(prompt)
            .expect("fenced code review prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_statistics_prompt_without_workflow_scaffold() {
        let engine = Engine::new();
        let prompt = r#"Summarize the dataset and keep the statistics words.

Data: 4, 7, 9, 10, 10, 13, 21
Return the median, variance, and standard deviation."#;

        let result = engine
            .compile(prompt)
            .expect("statistics prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_short_rewrite_contract_prompt() {
        let engine = Engine::new();
        let prompt = r#"Rewrite this email for a manager.

Please keep the deadline and apology intact, but make it concise and polite."#;

        let result = engine
            .compile(prompt)
            .expect("rewrite contract prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_citation_only_search_prompt() {
        let engine = Engine::new();
        let prompt = r#"Find the three most relevant sources about a 2024 shipping delay.

Return only the citations and a one-paragraph summary."#;

        let result = engine
            .compile(prompt)
            .expect("citation-only search prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_short_quiz_contract_prompt() {
        let engine = Engine::new();
        let prompt = r#"Create a 5-question quiz on fractions.

Include an answer key and one hint per question."#;

        let result = engine
            .compile(prompt)
            .expect("quiz contract prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_exact_field_extraction_prompt() {
        let engine = Engine::new();
        let prompt = r#"Extract the invoice fields from the note.

Return the invoice number, amount due, and due date only."#;

        let result = engine
            .compile(prompt)
            .expect("field extraction prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_row_heavy_research_prompt() {
        let engine = Engine::new();
        let prompt = r#"Analyze the survey results.

Data:
(Group-A, satisfaction, 4.8)
(Group-B, satisfaction, 3.1)
(Group-C, satisfaction, 4.0)

Return a short research note."#;

        let result = engine
            .compile(prompt)
            .expect("row-heavy research prompt should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_row_heavy_evidence_workflow() {
        let engine = Engine::new();
        let prompt = r#"Postmortem briefing.

Evidence:
(2026-03-12, host-7, login-failure, 18 attempts)
(2026-03-12, host-7, geo-mismatch, remote address)

Tasks:
- Identify the first failure point
- Return a short postmortem brief"#;

        let result = engine
            .compile(prompt)
            .expect("row-heavy evidence workflow should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn keeps_tokelang_for_short_numbered_branch_workflow_when_compact_clears_threshold() {
        let engine = Engine::new();
        let prompt = r#"Design an experiment plan.

1. Define the independent variable
2. If the control group is missing, stop and request it
3. Otherwise compare the treatment outcomes
4. Return a concise experimental protocol"#;

        let result = engine
            .compile(prompt)
            .expect("short numbered branch workflow should compile");

        assert_eq!(result.mode, CompileMode::Tokelang);
        let compact = result.compact.to_lowercase();
        assert!(compact.contains("independent variable"));
        assert!(compact.contains("control group"));
        assert!(compact.contains("request"));
        assert!(compact.contains("treatment outcomes"));
        assert!(compact.contains("experimental-protocol"));
        assert!(!compact.contains("definition"));
        assert!(!compact.contains("comparison"));
        assert!(!compact.contains("stop request"));
    }
}
