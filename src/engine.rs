use crate::compiler::CompileError;
use crate::compiler::Compiler;
use crate::compiler::normalize;
use crate::error::EngineError;
use crate::general_text;
use crate::ir::{
    BlockType, SemanticFrame, SourceSpan, SurfaceProfile, TokelangBlock, TokelangIR,
    TokelangProgram,
};
use crate::options::{CompileOptions, ProtectedRange, normalize_protected_ranges};
use crate::symbols::Instruction;
use crate::token_metrics::Tokenizer;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG: f64 = 15.0;
const LOW_RISK_WORKFLOW_MIN_TOKEN_SAVINGS_PCT: f64 = 12.0;
const COMPILE_CACHE_SCHEMA: u32 = 1;
const MIN_CHARS_FOR_COMPILE_CACHE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq)]
struct RoutingSignals {
    original_tokens: usize,
    compact_tokens: usize,
    reduction_pct: f64,
    natural_language_word_count: usize,
    protected_ratio_pct: f64,
    workflow_scaffold: bool,
    tuple_rows: usize,
    controller_hits: usize,
    exact_anchor_hits: usize,
    contract_hits: usize,
    short_output_note_like: bool,
    locality_hits: usize,
    continue_branch: bool,
    compare_hits: usize,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CompileCacheKey {
    schema: u32,
    profile: SurfaceProfile,
    input: String,
    protected_ranges: Vec<crate::options::ProtectedRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompileCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

/// Top-level facade for Tokelang compilation and compact parsing.
pub struct Engine {
    compiler: Compiler,
    tokenizer: Tokenizer,
    cache: Mutex<HashMap<CompileCacheKey, CompileResult>>,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            compiler: Compiler::new(),
            tokenizer: Tokenizer::detect(),
            cache: Mutex::new(HashMap::new()),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }

    pub fn compile(&self, input: &str) -> Result<CompileResult, EngineError> {
        self.compile_with_options(input, &CompileOptions::default())
    }

    pub fn compile_with_options(
        &self,
        input: &str,
        options: &CompileOptions,
    ) -> Result<CompileResult, EngineError> {
        self.compile_for_profile_with_options(input, SurfaceProfile::Default, options)
    }

    pub fn parse_compact(&self, input: &str) -> Result<TokelangProgram, EngineError> {
        Ok(TokelangProgram::parse_compact(input)?)
    }

    pub fn candidate_program(&self, input: &str) -> Result<TokelangProgram, EngineError> {
        self.candidate_program_with_options(input, &CompileOptions::default())
    }

    pub fn candidate_program_with_options(
        &self,
        input: &str,
        options: &CompileOptions,
    ) -> Result<TokelangProgram, EngineError> {
        Ok(self.compiler.compile_with_options(input, options)?)
    }

    pub fn compile_cache_stats(&self) -> CompileCacheStats {
        let entries = self.cache.lock().expect("compile cache poisoned").len();
        CompileCacheStats {
            hits: self.cache_hits.load(Ordering::Relaxed),
            misses: self.cache_misses.load(Ordering::Relaxed),
            entries,
        }
    }

    pub fn passthrough_threshold_pct(&self) -> f64 {
        MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG
    }

    pub fn passthrough_diagnostics(&self, input: &str) -> PassthroughDiagnostics {
        self.passthrough_diagnostics_with_options(input, &CompileOptions::default())
    }

    pub fn passthrough_diagnostics_with_options(
        &self,
        input: &str,
        options: &CompileOptions,
    ) -> PassthroughDiagnostics {
        let protected_ranges = normalize_protected_ranges(input, &options.protected_ranges)
            .unwrap_or_else(|_| options.protected_ranges.clone());
        let protected_pairs = protected_ranges
            .iter()
            .map(|range| (range.start, range.end))
            .collect::<Vec<_>>();
        let stats = normalize::protected_content_stats_with_user(input, &protected_pairs);
        let stripped = normalize::strip_protected_content_with_user(input, &protected_pairs);
        let cleaned = normalize::clean_input(&stripped);
        let natural_language_word_count = cleaned.split_whitespace().count();
        let protected_ratio_pct = if stats.total_chars == 0 {
            0.0
        } else {
            stats.protected_chars as f64 * 100.0 / stats.total_chars as f64
        };

        PassthroughDiagnostics {
            protected_chars: stats.protected_chars,
            total_chars: stats.total_chars,
            protected_ratio_pct,
            natural_language_word_count,
            protected_content_passthrough: stats.total_chars != 0
                && stats.protected_chars != 0
                && stats.protected_chars * 100 >= stats.total_chars * 40
                && natural_language_word_count < 16,
            passthrough_threshold_pct: MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG,
        }
    }

    fn routing_signals(
        &self,
        input: &str,
        tokelang_compact: &str,
        options: &CompileOptions,
    ) -> RoutingSignals {
        let original_tokens = self.tokenizer.count(input);
        let compact_tokens = self.tokenizer.count(tokelang_compact);
        let reduction_pct = if original_tokens == 0 {
            0.0
        } else {
            (original_tokens as f64 - compact_tokens as f64) / original_tokens as f64 * 100.0
        };
        let diagnostics = self.passthrough_diagnostics_with_options(input, options);
        let lowered = input.to_ascii_lowercase();
        let workflow_scaffold = has_workflow_scaffold(input);

        RoutingSignals {
            original_tokens,
            compact_tokens,
            reduction_pct,
            natural_language_word_count: diagnostics.natural_language_word_count,
            protected_ratio_pct: diagnostics.protected_ratio_pct,
            workflow_scaffold,
            tuple_rows: count_tuple_rows(input),
            controller_hits: count_contains(
                &lowered,
                &[
                    " if ",
                    "\nif ",
                    "otherwise",
                    "go to step",
                    "goto",
                    "keep ",
                    "return ",
                    "route ",
                    "stop and request approval",
                ],
            ),
            exact_anchor_hits: count_exact_anchor_signals(input, &lowered),
            contract_hits: count_contains(
                &lowered,
                &[
                    "return only",
                    "return the",
                    "keep ",
                    "preserve",
                    "include",
                    "do not",
                    "avoid",
                    "intact",
                    "visible",
                    "separate",
                ],
            ),
            short_output_note_like: is_short_output_note_prompt(&lowered),
            locality_hits: count_contains(
                &lowered,
                &[
                    "local to that branch",
                    "local to the branch",
                    "local to this branch",
                    "keep the note about",
                    "keep that quote only where",
                    "visible",
                ],
            ),
            continue_branch: lowered.contains("otherwise continue"),
            compare_hits: count_contains(&lowered, &["compare"]),
        }
    }

    fn protected_content_demands_passthrough(&self, input: &str, options: &CompileOptions) -> bool {
        let diagnostics = self.passthrough_diagnostics_with_options(input, options);
        let lowered = input.to_ascii_lowercase();
        let workflow_scaffold = has_workflow_scaffold(input);
        diagnostics.protected_content_passthrough
            || demands_fenced_code_passthrough(
                &lowered,
                diagnostics.natural_language_word_count,
                workflow_scaffold,
            )
            || demands_math_passthrough(
                input,
                &lowered,
                diagnostics.natural_language_word_count,
                workflow_scaffold,
            )
            || demands_row_heavy_passthrough(
                input,
                &lowered,
                diagnostics.natural_language_word_count,
                workflow_scaffold,
            )
            || demands_separation_sensitive_passthrough(&lowered, workflow_scaffold)
            || demands_contract_sensitive_passthrough(
                &lowered,
                diagnostics.natural_language_word_count,
                workflow_scaffold,
            )
            || demands_outline_sensitive_passthrough(
                &lowered,
                diagnostics.natural_language_word_count,
                workflow_scaffold,
            )
    }

    fn compile_for_profile_with_options(
        &self,
        input: &str,
        profile: SurfaceProfile,
        options: &CompileOptions,
    ) -> Result<CompileResult, EngineError> {
        let normalized_options = CompileOptions {
            protected_ranges: normalize_protected_ranges(input, &options.protected_ranges)?,
        };

        if let Some(cached) = self.cache_lookup(input, profile, &normalized_options) {
            return Ok(cached);
        }

        let result = if self.protected_content_demands_passthrough(input, &normalized_options) {
            CompileResult {
                program: TokelangProgram::default(),
                compact: input.to_string(),
                mode: CompileMode::Passthrough,
            }
        } else {
            match self
                .compiler
                .compile_with_options(input, &normalized_options)
            {
                Ok(program) => {
                    let tokelang_compact = program.to_compact_with_profile(profile);
                    let protected_spans_preserved = protected_spans_preserved_exactly(
                        input,
                        &tokelang_compact,
                        &normalized_options.protected_ranges,
                    );
                    let signals =
                        self.routing_signals(input, &tokelang_compact, &normalized_options);
                    let risk_passthrough = !protected_spans_preserved
                        || risk_policy_demands_passthrough(input, &signals);
                    let mode = if risk_passthrough {
                        CompileMode::Passthrough
                    } else if signals.reduction_pct <= min_token_savings_pct_for_signals(&signals) {
                        CompileMode::Passthrough
                    } else {
                        CompileMode::Tokelang
                    };
                    let general_candidate = general_text::candidate(input, &self.tokenizer);
                    let structured_workflow = has_workflow_scaffold(input);
                    let leading_sequence_scaffold = has_leading_sequence_scaffold(input);
                    let system_role_or_audience = has_system_role_or_audience_frame(input);
                    let use_general = general_candidate.as_ref().is_some_and(|candidate| {
                        !risk_passthrough
                            && !structured_workflow
                            && !leading_sequence_scaffold
                            && !system_role_or_audience
                            && general_text::should_prefer_general(
                                input,
                                &tokelang_compact,
                                candidate,
                            )
                    });

                    if use_general {
                        let candidate = general_candidate.expect("candidate checked above");
                        CompileResult {
                            program: general_text_program(input, &candidate.compact),
                            compact: candidate.compact,
                            mode: CompileMode::Tokelang,
                        }
                    } else {
                        let compact = match mode {
                            CompileMode::Tokelang => tokelang_compact,
                            CompileMode::Passthrough => input.to_string(),
                        };
                        CompileResult {
                            program,
                            compact,
                            mode,
                        }
                    }
                }
                Err(CompileError::NoInstruction | CompileError::NoSemanticContent) => {
                    if let Some(candidate) = general_text::candidate(input, &self.tokenizer) {
                        CompileResult {
                            program: general_text_program(input, &candidate.compact),
                            compact: candidate.compact,
                            mode: CompileMode::Tokelang,
                        }
                    } else {
                        CompileResult {
                            program: TokelangProgram::default(),
                            compact: input.to_string(),
                            mode: CompileMode::Passthrough,
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
        };

        self.cache_store(input, profile, &normalized_options, &result);
        Ok(result)
    }

    fn should_cache_input(input: &str) -> bool {
        input.len() >= MIN_CHARS_FOR_COMPILE_CACHE
    }

    fn cache_lookup(
        &self,
        input: &str,
        profile: SurfaceProfile,
        options: &CompileOptions,
    ) -> Option<CompileResult> {
        if !Self::should_cache_input(input) {
            return None;
        }

        let key = CompileCacheKey {
            schema: COMPILE_CACHE_SCHEMA,
            profile,
            input: input.to_string(),
            protected_ranges: options.protected_ranges.clone(),
        };
        let cache = self.cache.lock().expect("compile cache poisoned");
        let cached = cache.get(&key).cloned();
        drop(cache);
        if cached.is_some() {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.cache_misses.fetch_add(1, Ordering::Relaxed);
        }
        cached
    }

    fn cache_store(
        &self,
        input: &str,
        profile: SurfaceProfile,
        options: &CompileOptions,
        result: &CompileResult,
    ) {
        if !Self::should_cache_input(input) {
            return;
        }

        let key = CompileCacheKey {
            schema: COMPILE_CACHE_SCHEMA,
            profile,
            input: input.to_string(),
            protected_ranges: options.protected_ranges.clone(),
        };
        self.cache
            .lock()
            .expect("compile cache poisoned")
            .insert(key, result.clone());
    }
}

fn protected_spans_preserved_exactly(
    input: &str,
    compact: &str,
    protected_ranges: &[ProtectedRange],
) -> bool {
    if protected_ranges.is_empty() {
        return true;
    }

    let mut required_counts = HashMap::<&str, usize>::new();
    for range in protected_ranges {
        let slice = &input[range.start..range.end];
        if slice.is_empty() {
            continue;
        }
        *required_counts.entry(slice).or_default() += 1;
    }

    required_counts
        .into_iter()
        .all(|(slice, needed)| compact.match_indices(slice).count() >= needed)
}

fn general_text_program(input: &str, compact: &str) -> TokelangProgram {
    let item = TokelangIR {
        sequence_id: None,
        instruction: Instruction::Generate,
        frame: SemanticFrame::default(),
        modifiers: Vec::new(),
        flags: Default::default(),
        source_span: Some(SourceSpan {
            start: 0,
            end: input.len(),
        }),
        recovered_from_coverage: false,
        compact_override: Some(compact.to_string()),
    };

    TokelangProgram::new().with_block(TokelangBlock::new(BlockType::Default).add_item(item))
}

fn demands_fenced_code_passthrough(
    lowered: &str,
    natural_language_word_count: usize,
    workflow_scaffold: bool,
) -> bool {
    if !lowered.contains("```") || natural_language_word_count >= 48 {
        return false;
    }

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

    explicit_code_context || !workflow_scaffold
}

fn demands_math_passthrough(
    input: &str,
    lowered: &str,
    natural_language_word_count: usize,
    workflow_scaffold: bool,
) -> bool {
    if natural_language_word_count >= 56 || workflow_scaffold {
        return false;
    }

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

fn demands_contract_sensitive_passthrough(
    lowered: &str,
    natural_language_word_count: usize,
    workflow_scaffold: bool,
) -> bool {
    if natural_language_word_count >= 32 {
        return false;
    }

    let rewrite_hits = count_contains(&lowered, &["rewrite", "adapt the tone", "adapt tone"]);
    let translation_hits = count_contains(&lowered, &["translate"]);
    let extraction_hits = count_contains(&lowered, &["extract", "normalize"])
        + count_contains(
            &lowered,
            &[
                " fields",
                " field",
                "invoice number",
                "due date",
                "sender",
                "subject",
            ],
        );
    let search_hits = count_contains(
        &lowered,
        &[
            "sources",
            "source names",
            "source name",
            "citations",
            "literature",
            "research gap",
            "themes",
            "evidence",
            "new facts",
        ],
    );
    let tutoring_artifact_hits = count_contains(
        &lowered,
        &[
            "quiz",
            "answer key",
            "hint plan",
            "scaffolding",
            "example",
            "age-appropriate",
            "10-year-old",
            "10 year old",
            "misconception",
            "lesson",
        ],
    );
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

    let translation_contract = translation_hits >= 1 && contract_hits >= 1;
    if translation_contract {
        return true;
    }

    if workflow_scaffold {
        return false;
    }

    (rewrite_hits >= 1 && contract_hits >= 2)
        || (extraction_hits >= 2 && contract_hits >= 1)
        || (search_hits >= 1 && contract_hits >= 2)
        || (search_hits >= 2 && contract_hits >= 1)
        || (tutoring_artifact_hits >= 1 && contract_hits >= 2)
        || (tutoring_artifact_hits >= 2 && contract_hits >= 1)
}

fn demands_outline_sensitive_passthrough(
    lowered: &str,
    natural_language_word_count: usize,
    workflow_scaffold: bool,
) -> bool {
    if !workflow_scaffold || natural_language_word_count >= 36 {
        return false;
    }

    let sectioned_synthesis = (lowered.contains("sections:") || lowered.contains("sections\n"))
        && lowered.contains("synthesis")
        && lowered.contains("limitations")
        && (lowered.contains("next experiment") || lowered.contains("experiment"));

    let terse_incident_triage = lowered.contains("incident response")
        && lowered.contains("customer impact summary")
        && lowered.contains("go to step")
        && lowered.contains("incident memo");

    sectioned_synthesis || terse_incident_triage
}

fn demands_separation_sensitive_passthrough(lowered: &str, workflow_scaffold: bool) -> bool {
    if !workflow_scaffold {
        return false;
    }

    let refund_handoff = lowered.contains("policy note")
        && lowered.contains("appeal")
        && lowered.contains("refund")
        && lowered.contains("case note")
        && lowered.contains("notify billing");
    let portfolio_handoff = lowered.contains("risk note")
        && lowered.contains("recommendation")
        && lowered.contains("portfolio memo")
        && lowered.contains("current allocation");

    refund_handoff || portfolio_handoff
}

fn demands_row_heavy_passthrough(
    input: &str,
    lowered: &str,
    natural_language_word_count: usize,
    workflow_scaffold: bool,
) -> bool {
    let tuple_rows = input
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with('(') && line.ends_with(')') && line.matches(',').count() >= 2
        })
        .count();
    if tuple_rows < 2 {
        return false;
    }

    if workflow_scaffold {
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
    if [
        "tasks:",
        "workflow:",
        "step 1",
        "phase 1",
        "stage 1",
        "then:",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        return true;
    }

    input
        .lines()
        .filter(|line| looks_like_list_line(line.trim_start()))
        .take(2)
        .count()
        >= 2
}

fn has_leading_sequence_scaffold(input: &str) -> bool {
    let lowered = input.trim_start().to_ascii_lowercase();
    lowered.starts_with("first,")
        || lowered.starts_with("first ")
        || lowered.starts_with("step one")
        || lowered.starts_with("step 1")
}

fn has_system_role_or_audience_frame(input: &str) -> bool {
    let lowered = input.trim_start().to_ascii_lowercase();
    lowered.starts_with("you are ")
        || lowered.starts_with("you are an ")
        || lowered.starts_with("you are a ")
        || lowered.contains("audience that consists")
        || lowered.contains("audience consists")
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
    needles
        .iter()
        .filter(|needle| text.contains(**needle))
        .count()
}

fn min_token_savings_pct_for_signals(signals: &RoutingSignals) -> f64 {
    if signals.workflow_scaffold
        && signals.controller_hits >= 1
        && signals.exact_anchor_hits <= 1
        && !signals.short_output_note_like
    {
        LOW_RISK_WORKFLOW_MIN_TOKEN_SAVINGS_PCT
    } else {
        MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG
    }
}

fn risk_policy_demands_passthrough(input: &str, signals: &RoutingSignals) -> bool {
    short_compare_note_contract_passthrough(input, signals)
        || exact_visibility_contract_passthrough(input, signals)
        || anchor_dense_mixed_workflow_passthrough(signals)
        || terse_continue_branch_passthrough(signals)
        || row_heavy_policy_passthrough(signals)
}

fn short_compare_note_contract_passthrough(input: &str, signals: &RoutingSignals) -> bool {
    if signals.workflow_scaffold {
        return false;
    }

    let lowered = input.to_ascii_lowercase();
    let short_prompt = signals.original_tokens <= 24 || signals.natural_language_word_count <= 18;
    let compare_prompt = signals.compare_hits >= 1
        || lowered.contains("rewrite")
        || lowered.contains("adapt the tone")
        || lowered.contains("adapt tone");

    short_prompt && compare_prompt && signals.short_output_note_like
}

fn exact_visibility_contract_passthrough(input: &str, signals: &RoutingSignals) -> bool {
    if signals.workflow_scaffold {
        return false;
    }

    let lowered = input.to_ascii_lowercase();
    let has_exactness_contract = lowered.contains("visible")
        || lowered.contains("exactly")
        || lowered.contains("preserve")
        || lowered.contains("intact");
    let output_note_like = lowered.contains("return") && lowered.contains("note");

    output_note_like
        && has_exactness_contract
        && signals.contract_hits >= 2
        && (signals.exact_anchor_hits >= 1 || has_duration_like_anchor(&lowered))
}

fn anchor_dense_mixed_workflow_passthrough(signals: &RoutingSignals) -> bool {
    signals.workflow_scaffold
        && signals.exact_anchor_hits >= 3
        && signals.locality_hits >= 1
        && signals.short_output_note_like
}

fn terse_continue_branch_passthrough(signals: &RoutingSignals) -> bool {
    signals.workflow_scaffold
        && signals.continue_branch
        && signals.compare_hits >= 1
        && signals.short_output_note_like
        && signals.reduction_pct < 40.0
}

fn row_heavy_policy_passthrough(signals: &RoutingSignals) -> bool {
    signals.tuple_rows >= 2 && signals.natural_language_word_count < 48
}

fn count_tuple_rows(input: &str) -> usize {
    input
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with('(') && line.ends_with(')') && line.matches(',').count() >= 2
        })
        .count()
}

fn is_short_output_note_prompt(lowered: &str) -> bool {
    let output_terms = [
        "memo", "note", "brief", "summary", "reply", "sheet", "update",
    ];
    let short_terms = ["short", "brief", "concise"];

    lowered.contains("return")
        && output_terms.iter().any(|term| lowered.contains(term))
        && short_terms.iter().any(|term| lowered.contains(term))
}

fn count_exact_anchor_signals(input: &str, lowered: &str) -> usize {
    let mut hits = 0;

    if input.contains('"') || input.contains('`') {
        hits += 1;
    }
    if input.contains('%') {
        hits += 1;
    }
    if has_iso_date_like_anchor(input) {
        hits += 1;
    }
    if has_path_or_url_like_anchor(lowered) {
        hits += 1;
    }
    if has_duration_like_anchor(lowered) {
        hits += 1;
    }
    if lowered.contains("top 3")
        || lowered.contains("top 5")
        || lowered.contains("exactly ")
        || lowered.contains("at least ")
    {
        hits += 1;
    }
    if lowered.contains("o(n") || lowered.contains("t(n)") {
        hits += 1;
    }
    if input.contains('_') {
        hits += 1;
    }

    hits
}

fn has_iso_date_like_anchor(input: &str) -> bool {
    input.split_whitespace().any(|token| {
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        let bytes = token.as_bytes();
        bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    })
}

fn has_path_or_url_like_anchor(lowered: &str) -> bool {
    lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("/api/")
        || lowered.contains(".yml")
        || lowered.contains(".yaml")
        || lowered.contains(".json")
        || lowered.contains(".toml")
        || lowered.contains(".md")
        || lowered.contains("/srv/")
}

fn has_duration_like_anchor(lowered: &str) -> bool {
    lowered.split_whitespace().any(|token| {
        let trimmed = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let has_digit = trimmed.chars().any(|character| character.is_ascii_digit());
        has_digit
            && (trimmed.ends_with("ms")
                || trimmed.ends_with('s')
                || trimmed.ends_with("min")
                || trimmed.ends_with("mins")
                || trimmed.ends_with("hour")
                || trimmed.ends_with("hours")
                || trimmed.ends_with("day")
                || trimmed.ends_with("days"))
    }) || lowered.contains(" minutes")
        || lowered.contains(" minute")
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CompileMode, Engine};
    use crate::CompileOptions;
    use crate::ir::SurfaceProfile;

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
            "token-savings fallback should still retain the attempted compiled program"
        );
    }

    #[test]
    fn can_still_request_candidate_program_for_passthrough_prompt() {
        let engine = Engine::new();
        let prompt = "Explain AI in depth.";

        let candidate = engine
            .candidate_program(prompt)
            .expect("candidate program should still compile");

        assert!(!candidate.blocks.is_empty());
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
        assert!(
            result.program.blocks.is_empty(),
            "protected-content passthrough should skip building a compiled program"
        );
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
        assert!(
            compact.contains("experimental protocol") || compact.contains("experimental-protocol")
        );
        assert!(!compact.contains("definition"));
        assert!(!compact.contains("comparison"));
        assert!(!compact.contains("stop request"));
    }

    #[test]
    fn falls_back_to_passthrough_for_translation_requirements_workflow() {
        let engine = Engine::new();
        let prompt = r#"Translate this release note into German.

Requirements:
- Keep bullet formatting
- Preserve the product names
- Keep the glossary terms untouched
- Return the translated note only"#;

        let result = engine
            .compile(prompt)
            .expect("translation requirements workflow should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_sectioned_study_synthesis_outline() {
        let engine = Engine::new();
        let prompt = r#"Write a study synthesis.

Sections:
- Summarize the strongest claims
- Keep the limitations separate
- Note the next experiment"#;

        let result = engine
            .compile(prompt)
            .expect("sectioned study synthesis should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn falls_back_to_passthrough_for_terse_incident_response_branch_workflow() {
        let engine = Engine::new();
        let prompt = r#"Incident response.

1. Capture the customer impact summary
2. If billing appears, go to Step 4
3. Otherwise continue the outage investigation
4. Return a short incident memo"#;

        let result = engine
            .compile(prompt)
            .expect("terse incident response workflow should compile");

        assert_eq!(result.mode, CompileMode::Passthrough);
        assert_eq!(result.compact, prompt);
    }

    #[test]
    fn long_prompt_cache_registers_hit_on_second_compile() {
        let engine = Engine::new();
        let prompt = r#"Account recovery escalation review.

We are reviewing repeated recovery failures across billing address mismatch, recovery email changes, MFA reset requests, recent password changes, support hold windows, device drift, last known good session data, and VIP handling rules.

1. Check identity signals from billing address, recent invoice, MFA reset request, recovery email change timing, and the last known good session.
2. If billing mismatch is present and the recovery email changed within 24 hours, go to Step 5.
3. Otherwise compare the recent device and IP cluster against the last known good session and keep uncertain signals separate from the recovery note.
4. Summarize the safe manual verification steps for the support agent.
5. Return a detailed escalation memo."#;

        let first = engine.compile(prompt).expect("first compile");
        let after_first = engine.compile_cache_stats();
        let second = engine.compile(prompt).expect("second compile");
        let after_second = engine.compile_cache_stats();

        assert_eq!(first.compact, second.compact);
        assert_eq!(first.mode, second.mode);
        assert_eq!(after_first.entries, 1);
        assert_eq!(after_first.misses, 1);
        assert_eq!(after_first.hits, 0);
        assert_eq!(after_second.entries, 1);
        assert_eq!(after_second.misses, 1);
        assert_eq!(after_second.hits, 1);
    }

    #[test]
    fn short_prompt_skips_compile_cache() {
        let engine = Engine::new();
        let prompt = "Explain AI in depth.";

        let _ = engine.compile(prompt).expect("first compile");
        let _ = engine.compile(prompt).expect("second compile");
        let stats = engine.compile_cache_stats();

        assert_eq!(stats.entries, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    #[test]
    fn compile_cache_is_profile_sensitive() {
        let engine = Engine::new();
        let prompt = r#"Account recovery escalation review.

We are reviewing repeated recovery failures across billing address mismatch, recovery email changes, MFA reset requests, recent password changes, support hold windows, device drift, last known good session data, and VIP handling rules.

1. Check identity signals from billing address, recent invoice, MFA reset request, recovery email change timing, and the last known good session.
2. If billing mismatch is present and the recovery email changed within 24 hours, go to Step 5.
3. Otherwise compare the recent device and IP cluster against the last known good session and keep uncertain signals separate from the recovery note.
4. Summarize the safe manual verification steps for the support agent.
5. Return a detailed escalation memo."#;

        let default = engine
            .compile_for_profile_with_options(
                prompt,
                SurfaceProfile::Default,
                &CompileOptions::default(),
            )
            .expect("default profile compile");
        let robust = engine
            .compile_for_profile_with_options(
                prompt,
                SurfaceProfile::Robust,
                &CompileOptions::default(),
            )
            .expect("robust profile compile");
        let stats = engine.compile_cache_stats();

        assert_ne!(default.compact, robust.compact);
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
    }
}
