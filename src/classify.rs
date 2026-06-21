//! Prompt classifier for Model-Emulation Compression (MEC) — the P0
//! "classify-then-route" gate from `dogfood/IR_RESTRUCTURE_PRINCIPLES.md`.
//!
//! The engine's core mistake (and the NB#29 bug class) is applying the
//! instruction-IR to *every* prompt. The full-corpus analysis (n=1031) shows the
//! faithful-compression headroom is sharply route-dependent: pasted reference
//! payloads carry 75% of all tokens at a ~76% faithful ceiling, while short
//! conversational prompts have ~0% headroom and are *damaged* by restructuring.
//! So MEC classifies each prompt first, then dispatches it to the handling its
//! type warrants.
//!
//! This is pure and tokenizer-free: the caller passes the cl100k token count so
//! the classifier stays unit-testable without a live tiktoken worker. Thresholds
//! mirror the validated Python prototype that produced the full-corpus route
//! table.
//!
//! MEC-0 lands the classifier behavior-preservingly: it is exposed for
//! measurement (`Engine::classify_route`) but is **not** yet wired into the
//! compile output path. Wiring routes to new behavior (short→passthrough,
//! paste→literal-island) happens in later, separately-gated iterations.

/// The route a prompt is dispatched to. Variant order matches the first-match
/// priority in [`classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptRoute {
    /// Pasted reference payload — terminal/tool output, logs, handover docs,
    /// code blocks. ~18% of prompts but ~75% of all tokens; the compression
    /// upside lives here (literal-island + dedup).
    Paste,
    /// Short conversational text (≤20 tok, not an imperative). ~0% faithful
    /// headroom; wants passthrough/light-clean, never restructuring.
    ShortConv,
    /// A question by form (longer than the short-conv cutoff). Keep the
    /// interrogative intent; never coerce into an instruction frame.
    Question,
    /// A genuine imperative instruction.
    Instruction,
    /// Everything else — medium-length, mixed declarative content.
    Other,
}

impl PromptRoute {
    pub fn as_str(self) -> &'static str {
        match self {
            PromptRoute::Paste => "paste",
            PromptRoute::ShortConv => "short_conv",
            PromptRoute::Question => "question",
            PromptRoute::Instruction => "instruction",
            PromptRoute::Other => "other",
        }
    }
}

/// Token cutoff (inclusive) below which a non-imperative prompt is treated as
/// short conversational text.
const SHORT_CONV_MAX_TOKENS: usize = 20;

/// Leading base verbs that mark an imperative instruction (checked as the first
/// word only). `do` doubles as an interrogative opener but is treated as
/// imperative here, matching the prototype.
const IMPERATIVE_VERBS: &[&str] = &[
    "add",
    "make",
    "update",
    "fix",
    "change",
    "create",
    "run",
    "build",
    "write",
    "implement",
    "remove",
    "delete",
    "use",
    "check",
    "read",
    "push",
    "deploy",
    "generate",
    "refactor",
    "move",
    "rename",
    "set",
    "give",
    "show",
    "list",
    "explain",
    "analyze",
    "analyse",
    "do",
    "let",
    "lets",
    "please",
    "ensure",
    "consider",
    "look",
    "find",
    "test",
    "review",
    "go",
    "continue",
    "pick",
    "start",
    "keep",
    "put",
    "include",
    "provide",
    "modify",
    "convert",
    "extract",
    "merge",
    "split",
    "install",
    "enable",
    "disable",
    "drop",
    "save",
    "commit",
    "send",
];

/// Leading interrogative openers (checked as the first word only).
const INTERROGATIVES: &[&str] = &[
    "is", "are", "am", "can", "could", "do", "does", "did", "should", "would", "will", "why",
    "how", "hows", "what", "whats", "which", "who", "where", "when", "whose", "isnt", "arent",
    "dont", "wont",
];

/// Embedded markers that signal a question even mid-sentence (e.g. tag
/// questions). Matched with a leading word boundary so `or not` does not fire on
/// `for not`.
const EMBEDDED_QUESTION_MARKERS: &[&str] = &[
    "is it",
    "are they",
    "are these",
    "is that",
    "right?",
    "isnt it",
    "isn't it",
    "or not",
    "fine?",
    "ok?",
    "okay?",
];

/// Classify `input` into its MEC route. `token_count` is the cl100k token count
/// of `input` (the caller has the tokenizer; tests pass a known value).
pub fn classify(input: &str, token_count: usize) -> PromptRoute {
    if is_paste(input) {
        return PromptRoute::Paste;
    }
    let imperative = is_imperative(input);
    if token_count <= SHORT_CONV_MAX_TOKENS && !imperative {
        return PromptRoute::ShortConv;
    }
    if !imperative && is_question(input) {
        return PromptRoute::Question;
    }
    if imperative {
        return PromptRoute::Instruction;
    }
    PromptRoute::Other
}

/// Leading run of ASCII-alphabetic characters, lowercased. Empty when the first
/// non-whitespace character is not alphabetic.
fn first_word(input: &str) -> String {
    input
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn is_imperative(input: &str) -> bool {
    let word = first_word(input);
    IMPERATIVE_VERBS.contains(&word.as_str())
}

fn is_question(input: &str) -> bool {
    let word = first_word(input);
    if INTERROGATIVES.contains(&word.as_str()) {
        return true;
    }
    if input.trim_end().ends_with('?') {
        return true;
    }
    let lowered = input.to_ascii_lowercase();
    EMBEDDED_QUESTION_MARKERS
        .iter()
        .any(|marker| contains_word(&lowered, marker))
}

/// True if `needle` (already lowercase, ASCII) appears in `hay` with a leading
/// word boundary. Char-boundary safe (advances by the ASCII needle length).
fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(offset) = hay[start..].find(needle) {
        let abs = start + offset;
        let boundary_before = abs == 0
            || !hay[..abs]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if boundary_before {
            return true;
        }
        start = abs + needle.len();
    }
    false
}

/// Pasted reference payloads: multi-line dumps, terminal/tool output, code
/// blocks, markdown docs, or literal-dense reference text.
fn is_paste(input: &str) -> bool {
    if input.matches('\n').count() >= 3 {
        return true;
    }
    if has_structural_marker(input) {
        return true;
    }
    if input.len() > 200 && literal_density(input) >= 3 {
        return true;
    }
    false
}

fn has_structural_marker(input: &str) -> bool {
    input.contains("```")
        || input.contains("root@")
        || input.contains(">>>")
        || input.contains("====")
        || has_prompt_sigil(input)
        || has_percent_after_digit(input)
        || has_progress_ratio(input)
        || has_markdown_header_line(input)
}

/// Shell-prompt sigils: `~#`/`~$` at a word end, or `$ ` followed by a command.
fn has_prompt_sigil(input: &str) -> bool {
    let bytes = input.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'~' && i + 1 < bytes.len() && matches!(bytes[i + 1], b'#' | b'$') {
            let after = i + 2;
            if after >= bytes.len() || bytes[after].is_ascii_whitespace() {
                return true;
            }
        }
        if bytes[i] == b'$'
            && i + 2 < bytes.len()
            && bytes[i + 1] == b' '
            && bytes[i + 2].is_ascii_alphanumeric()
        {
            return true;
        }
    }
    false
}

/// A percent figure attached to a digit, e.g. `100%` (progress bars).
fn has_percent_after_digit(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'%' && i > 0 && bytes[i - 1].is_ascii_digit())
}

/// Download/progress ratios like `11.4M/11.4M` (tqdm / HuggingFace output).
fn has_progress_ratio(input: &str) -> bool {
    let b = input.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j < b.len() && b[j] == b'.' {
            let mut k = j + 1;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > j + 1 {
                if k < b.len() && matches!(b[k], b'K' | b'M' | b'G' | b'k' | b'm' | b'g') {
                    k += 1;
                }
                if k + 1 < b.len() && b[k] == b'/' && b[k + 1].is_ascii_digit() {
                    return true;
                }
            }
        }
        i = j.max(i + 1);
    }
    false
}

/// A markdown header line (`#`..`######` followed by a space).
fn has_markdown_header_line(input: &str) -> bool {
    input.lines().any(|line| {
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|&c| c == b'#').count();
        (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ')
    })
}

/// Count of whitespace-delimited tokens that look like load-bearing literals:
/// URLs, multi-segment paths, or hex hashes.
fn literal_density(input: &str) -> usize {
    input
        .split_whitespace()
        .filter(|token| is_url(token) || is_path(token) || is_hash(token))
        .count()
}

fn is_url(token: &str) -> bool {
    token.contains("://")
}

fn is_path(token: &str) -> bool {
    token.matches('/').count() >= 2
}

fn is_hash(token: &str) -> bool {
    (7..=40).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(input: &str, token_count: usize) -> PromptRoute {
        classify(input, token_count)
    }

    #[test]
    fn short_non_imperative_is_short_conv() {
        assert_eq!(
            route("how do i run it? is it in apps?", 10),
            PromptRoute::ShortConv
        );
        assert_eq!(
            route("okay what do you want me to do now?", 9),
            PromptRoute::ShortConv
        );
        assert_eq!(
            route("its not a ipynb is it fine ?", 8),
            PromptRoute::ShortConv
        );
        assert_eq!(route("yeah that works", 3), PromptRoute::ShortConv);
    }

    #[test]
    fn short_conv_boundary_is_inclusive_at_20() {
        assert_eq!(
            route("a fairly short status note that we kept around twenty", 20),
            PromptRoute::ShortConv
        );
        // 21 tokens, non-imperative, no question form -> Other
        assert_eq!(
            route(
                "a fairly short status note that we kept around twenty one",
                21
            ),
            PromptRoute::Other
        );
    }

    #[test]
    fn long_question_routes_to_question() {
        let q = "what are the prompts I can test out on claude code to really stress \
                 the engine and see where it drops meaning across many domains";
        assert_eq!(route(q, 30), PromptRoute::Question);
    }

    #[test]
    fn trailing_question_mark_routes_long_to_question() {
        let q = "the migration ran on staging and it all came back green and stable, right?";
        assert_eq!(route(q, 25), PromptRoute::Question);
    }

    #[test]
    fn imperative_beats_short_and_question() {
        assert_eq!(route("fix the bug", 3), PromptRoute::Instruction);
        assert_eq!(
            route("update it in the md files", 7),
            PromptRoute::Instruction
        );
        // "do we have work?" -> first word "do" is imperative-listed -> Instruction
        assert_eq!(
            route("do we have work or not?", 6),
            PromptRoute::Instruction
        );
    }

    #[test]
    fn long_imperative_routes_to_instruction() {
        let s = "Add another hundred, make sure to consider from class 7-8 for the \
                 dataset and rebalance everything before training";
        assert_eq!(route(s, 30), PromptRoute::Instruction);
    }

    #[test]
    fn medium_declarative_routes_to_other() {
        let s = "yes ai node is better than rulebased and we should keep iterating \
                 on the routing policy before shipping anything";
        assert_eq!(route(s, 25), PromptRoute::Other);
    }

    #[test]
    fn paste_detected_by_newlines() {
        assert_eq!(
            route("line one\nline two\nline three\nline four", 12),
            PromptRoute::Paste
        );
        // priority: short by tokens but multi-line -> Paste
        assert_eq!(route("a\nb\nc\nd", 4), PromptRoute::Paste);
    }

    #[test]
    fn paste_detected_by_terminal_markers() {
        assert_eq!(
            route("root@docker:~# git fetch --prune origin", 12),
            PromptRoute::Paste
        );
        assert_eq!(
            route("tokenizer.json: 100% 11.4M/11.4M done", 12),
            PromptRoute::Paste
        );
        assert_eq!(
            route("here is the code: ```rust fn main(){}```", 12),
            PromptRoute::Paste
        );
        assert_eq!(route(">>> import torch", 4), PromptRoute::Paste);
    }

    #[test]
    fn paste_detected_by_markdown_header() {
        assert_eq!(
            route("# AGENTS.md instructions for the repo", 8),
            PromptRoute::Paste
        );
        assert_eq!(
            route("### section three of the doc here", 7),
            PromptRoute::Paste
        );
    }

    #[test]
    fn paste_detected_by_literal_density() {
        let s = "the failing files after the rebase landed yesterday afternoon are \
                 crates/tokelang-core/src/engine.rs and apps/web/src/app/page.tsx and \
                 crates/tokelang-server/src/main.rs so please take a careful look soon";
        assert!(s.len() > 200);
        assert_eq!(route(s, 40), PromptRoute::Paste);
    }

    #[test]
    fn contains_word_respects_leading_boundary() {
        // "or not" must not fire inside "for not"
        assert!(!contains_word(
            "we waited for not very long today",
            "or not"
        ));
        assert!(contains_word("should we ship it or not today", "or not"));
        assert!(contains_word("or not at the very start", "or not"));
    }

    #[test]
    fn no_false_positive_or_not_in_question_detection() {
        let s = "we waited for not very long before the build finished and then we shipped it";
        assert_eq!(route(s, 25), PromptRoute::Other);
    }

    #[test]
    fn route_as_str_round_trips() {
        for r in [
            PromptRoute::Paste,
            PromptRoute::ShortConv,
            PromptRoute::Question,
            PromptRoute::Instruction,
            PromptRoute::Other,
        ] {
            assert!(!r.as_str().is_empty());
        }
    }
}
