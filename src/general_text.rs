use crate::token_metrics::Tokenizer;
use crate::validator;
use std::collections::HashSet;

const MIN_GENERAL_SAVINGS_PCT: f64 = 8.0;

#[derive(Debug, Clone)]
pub(crate) struct GeneralTextCandidate {
    pub compact: String,
    pub savings_pct: f64,
    pub content_recall: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GeneralTextRisk {
    pub content_tokens: usize,
    pub negation_hits: usize,
    pub constraint_hits: usize,
    pub output_shape_hits: usize,
    pub number_hits: usize,
    pub role_hits: usize,
}

impl GeneralTextRisk {
    pub fn is_context_rich(self) -> bool {
        self.content_tokens >= 14
            || self.negation_hits > 0
            || self.constraint_hits >= 2
            || self.output_shape_hits >= 2
            || self.number_hits >= 2
            || self.role_hits > 0
    }
}

pub(crate) fn candidate(input: &str, tokenizer: &Tokenizer) -> Option<GeneralTextCandidate> {
    let compact = compress(input)?;
    if compact == input.trim() {
        return None;
    }

    let original_tokens = tokenizer.count(input);
    let compact_tokens = tokenizer.count(&compact);
    if compact_tokens >= original_tokens {
        return None;
    }

    let savings_pct = pct_savings(original_tokens, compact_tokens);
    if savings_pct < MIN_GENERAL_SAVINGS_PCT {
        return None;
    }

    Some(GeneralTextCandidate {
        content_recall: content_recall(input, &compact),
        compact,
        savings_pct,
    })
}

pub(crate) fn risk(input: &str) -> GeneralTextRisk {
    let tokens = lexical_tokens(input);
    let content = content_token_set_from_tokens(&tokens);

    GeneralTextRisk {
        content_tokens: content.len(),
        negation_hits: tokens
            .iter()
            .filter(|token| is_negation(&token.to_ascii_lowercase()))
            .count(),
        constraint_hits: tokens
            .iter()
            .filter(|token| is_constraint_word(&token.to_ascii_lowercase()))
            .count(),
        output_shape_hits: tokens
            .iter()
            .filter(|token| is_output_shape_word(&token.to_ascii_lowercase()))
            .count(),
        number_hits: tokens
            .iter()
            .filter(|token| token.chars().any(|ch| ch.is_ascii_digit()))
            .count(),
        role_hits: count_role_hits(&tokens),
    }
}

pub(crate) fn content_recall(original: &str, compact: &str) -> f64 {
    let original_tokens = content_token_set(original);
    if original_tokens.is_empty() {
        return 1.0;
    }
    let compact_tokens = content_token_set(compact);
    let retained = original_tokens
        .iter()
        .filter(|token| compact_tokens.contains(*token))
        .count();
    retained as f64 / original_tokens.len() as f64
}

pub(crate) fn should_prefer_general(
    input: &str,
    tokelang_compact: &str,
    general: &GeneralTextCandidate,
) -> bool {
    let risk = risk(input);
    let tokelang_recall = content_recall(input, tokelang_compact);
    if risk.negation_hits > 0
        && !preserves_any_negation(tokelang_compact)
        && preserves_any_negation(&general.compact)
    {
        return true;
    }

    if structured_drops_preserved_special_token(input, tokelang_compact, &general.compact) {
        return true;
    }

    if risk.role_hits > 0 {
        return general.content_recall >= 0.74
            && general.content_recall >= tokelang_recall + 0.05
            && general.savings_pct >= MIN_GENERAL_SAVINGS_PCT;
    }

    if !risk.is_context_rich() {
        return has_request_wrapper_noise(input)
            && tokelang_recall < 0.88
            && general.content_recall >= tokelang_recall + 0.12
            && general.savings_pct >= MIN_GENERAL_SAVINGS_PCT;
    }

    let required_structured_recall = if risk.role_hits > 0
        || risk.negation_hits > 0
        || risk.constraint_hits >= 2
        || risk.output_shape_hits >= 2
        || risk.number_hits >= 2
    {
        0.92
    } else {
        0.86
    };

    if tokelang_recall >= required_structured_recall {
        return false;
    }

    general.content_recall >= 0.86
        && general.content_recall >= tokelang_recall + 0.06
        && general.savings_pct >= MIN_GENERAL_SAVINGS_PCT
}

fn compress(input: &str) -> Option<String> {
    let hard_zones = hard_zones(input);
    if hard_zones.is_empty() {
        return compress_unprotected(input);
    }

    let mut parts = Vec::new();
    let mut cursor = 0usize;

    for zone in hard_zones {
        if cursor < zone.start {
            parts.extend(compress_unprotected_parts(&input[cursor..zone.start]));
        }

        let literal = input[zone.start..zone.end].trim();
        if !literal.is_empty() {
            parts.push(literal.to_string());
        }

        cursor = zone.end;
    }

    if cursor < input.len() {
        parts.extend(compress_unprotected_parts(&input[cursor..]));
    }

    let compact = parts.join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

fn compress_unprotected(input: &str) -> Option<String> {
    let parts = compress_unprotected_parts(input);
    if parts.is_empty() {
        return None;
    }

    let compact = parts.join(" ");
    let compact = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

fn compress_unprotected_parts(input: &str) -> Vec<String> {
    let tokens = lexical_tokens(input);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut kept = Vec::new();
    for index in 0..tokens.len() {
        let token = &tokens[index];
        let lower = token.to_ascii_lowercase();
        let previous = index.checked_sub(1).and_then(|prev| tokens.get(prev));
        let next = tokens.get(index + 1);

        if should_drop_token(&lower, previous, next, index) {
            continue;
        }

        let normalized = normalize_kept_token(token);
        if normalized.is_empty() {
            continue;
        }
        kept.push(normalized);
    }

    if kept.is_empty() {
        return Vec::new();
    }

    collapse_common_phrases(kept)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HardZone {
    start: usize,
    end: usize,
}

fn hard_zones(input: &str) -> Vec<HardZone> {
    let mut zones = Vec::new();
    collect_bracket_placeholders(input, &mut zones);
    collect_template_placeholders(input, &mut zones);
    collect_delimited_hard_zones(input, '"', &mut zones);
    if !input.contains("```") {
        collect_delimited_hard_zones(input, '`', &mut zones);
    }
    normalize_hard_zones(zones)
}

fn normalize_hard_zones(mut zones: Vec<HardZone>) -> Vec<HardZone> {
    zones.sort_by_key(|zone| (zone.start, zone.end));
    let mut normalized = Vec::new();

    for zone in zones {
        if zone.start >= zone.end {
            continue;
        }
        if normalized
            .last()
            .is_some_and(|previous: &HardZone| zone.start < previous.end)
        {
            continue;
        }
        normalized.push(zone);
    }

    normalized
}

fn collect_bracket_placeholders(input: &str, zones: &mut Vec<HardZone>) {
    let bytes = input.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }

        let Some(end_index) = input[index + 1..]
            .find(']')
            .map(|offset| index + 1 + offset)
        else {
            break;
        };

        let inner = &input[index + 1..end_index];
        if is_bracket_placeholder_inner(inner) {
            zones.push(HardZone {
                start: index,
                end: end_index + 1,
            });
        }

        index = end_index + 1;
    }
}

fn is_bracket_placeholder_inner(inner: &str) -> bool {
    let trimmed = inner.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 80
        && trimmed.split_whitespace().count() <= 8
        && !trimmed.contains('\n')
        && trimmed.chars().all(|ch| {
            ch.is_alphanumeric()
                || ch.is_whitespace()
                || matches!(ch, '_' | '-' | '.' | '/' | '$' | ':')
        })
}

fn collect_template_placeholders(input: &str, zones: &mut Vec<HardZone>) {
    let bytes = input.as_bytes();
    let mut index = 0usize;

    while index + 1 < bytes.len() {
        if bytes[index] != b'$' || bytes[index + 1] != b'{' {
            index += 1;
            continue;
        }

        let Some(end_index) = input[index + 2..]
            .find('}')
            .map(|offset| index + 2 + offset)
        else {
            break;
        };

        let inner = &input[index + 2..end_index];
        if is_template_placeholder_inner(inner) {
            zones.push(HardZone {
                start: index,
                end: end_index + 1,
            });
        }

        index = end_index + 1;
    }
}

fn is_template_placeholder_inner(inner: &str) -> bool {
    let trimmed = inner.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 100
        && !trimmed.contains('\n')
        && trimmed.chars().all(|ch| {
            ch.is_alphanumeric()
                || ch.is_whitespace()
                || matches!(ch, '_' | '-' | '.' | '/' | ':' | ',')
        })
}

fn collect_delimited_hard_zones(input: &str, delimiter: char, zones: &mut Vec<HardZone>) {
    let mut start = None;

    for (index, ch) in input.char_indices() {
        if ch != delimiter {
            continue;
        }

        if let Some(open_index) = start.take() {
            let inner = input[open_index + delimiter.len_utf8()..index].trim();
            if is_delimited_hard_zone_inner(inner) {
                zones.push(HardZone {
                    start: open_index,
                    end: index + delimiter.len_utf8(),
                });
            }
        } else {
            start = Some(index);
        }
    }
}

fn is_delimited_hard_zone_inner(inner: &str) -> bool {
    !inner.is_empty() && inner.len() <= 1000
}

fn lexical_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric()
            || matches!(
                ch,
                '_' | '-' | '/' | '@' | '$' | '%' | ':' | '.' | '\'' | '+' | '#' | '='
            )
        {
            current.push(ch);
            continue;
        }

        if !current.is_empty() {
            tokens.push(trim_token(&current));
            current.clear();
        }
    }

    if !current.is_empty() {
        tokens.push(trim_token(&current));
    }

    tokens
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect()
}

fn trim_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | ';' | ':' | '.' | '!' | '?' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        })
        .to_string()
}

fn should_drop_token(
    lower: &str,
    previous: Option<&String>,
    next: Option<&String>,
    index: usize,
) -> bool {
    if validator::is_critical_token(lower)
        || is_negation(lower)
        || is_constraint_word(lower)
        || is_output_shape_word(lower)
    {
        return false;
    }

    if matches!(lower, "a" | "an" | "the" | "please" | "kindly" | "just") {
        return true;
    }

    if matches!(
        lower,
        "i" | "me" | "you" | "we" | "us" | "it" | "he" | "she" | "they" | "them"
    ) {
        return true;
    }

    if matches!(
        lower,
        "my" | "your" | "our" | "their" | "his" | "her" | "hers" | "its"
    ) {
        return true;
    }

    if matches!(
        lower,
        "can"
            | "could"
            | "would"
            | "will"
            | "am"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "being"
            | "been"
            | "have"
            | "has"
            | "had"
    ) && !next.is_some_and(|token| is_negation(&token.to_ascii_lowercase()))
    {
        return true;
    }

    if matches!(lower, "do" | "does" | "did") {
        return true;
    }

    if matches!(
        lower,
        "about"
            | "of"
            | "for"
            | "with"
            | "in"
            | "on"
            | "at"
            | "where"
            | "who"
            | "whom"
            | "which"
            | "that"
            | "this"
            | "these"
            | "those"
    ) {
        return true;
    }

    if lower == "order"
        && previous.is_some_and(|token| token.eq_ignore_ascii_case("in"))
        && next.is_some_and(|token| token.eq_ignore_ascii_case("to"))
    {
        return true;
    }

    if lower == "to"
        && next.is_some_and(|token| is_action_or_control_word(&token.to_ascii_lowercase()))
        && (index <= 4
            || previous.is_some_and(|token| {
                matches!(
                    token.to_ascii_lowercase().as_str(),
                    "want"
                        | "need"
                        | "like"
                        | "try"
                        | "trying"
                        | "help"
                        | "you"
                        | "me"
                        | "us"
                        | "i"
                        | "we"
                        | "is"
                        | "are"
                        | "was"
                        | "were"
                        | "be"
                        | "order"
                )
            }))
    {
        return true;
    }

    if matches!(
        lower,
        "want"
            | "need"
            | "needs"
            | "needed"
            | "like"
            | "really"
            | "very"
            | "some"
            | "thing"
            | "things"
            | "kind"
            | "type"
    ) {
        return true;
    }

    if lower == "help" && index <= 2 {
        return true;
    }

    if matches!(lower, "and" | "also") {
        return true;
    }

    false
}

fn normalize_kept_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if token.chars().any(|ch| ch.is_ascii_digit())
        || token.contains('@')
        || token.contains('/')
        || token.contains('_')
        || token.contains('$')
        || token.chars().any(|ch| ch.is_ascii_uppercase())
    {
        token.to_string()
    } else {
        lower
    }
}

fn collapse_common_phrases(tokens: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if index + 2 < tokens.len()
            && tokens[index].eq_ignore_ascii_case("step")
            && tokens[index + 1].eq_ignore_ascii_case("by")
            && tokens[index + 2].eq_ignore_ascii_case("step")
        {
            output.push("steps".to_string());
            index += 3;
            continue;
        }

        if index + 1 < tokens.len()
            && tokens[index].eq_ignore_ascii_case("decision")
            && tokens[index + 1].eq_ignore_ascii_case("tree")
        {
            output.push("decision tree".to_string());
            index += 2;
            continue;
        }

        output.push(tokens[index].clone());
        index += 1;
    }
    output
}

fn content_token_set(text: &str) -> HashSet<String> {
    content_token_set_from_tokens(&lexical_tokens(text))
}

fn content_token_set_from_tokens(tokens: &[String]) -> HashSet<String> {
    tokens
        .iter()
        .map(|token| token.to_ascii_lowercase())
        .filter(|token| is_content_token(token))
        .collect()
}

fn is_content_token(token: &str) -> bool {
    if token.chars().any(|ch| ch.is_ascii_digit()) {
        return true;
    }
    if is_negation(token) || is_constraint_word(token) || is_output_shape_word(token) {
        return true;
    }
    if token.len() < 3 {
        return false;
    }
    !matches!(
        token,
        "and"
            | "the"
            | "for"
            | "with"
            | "from"
            | "that"
            | "this"
            | "these"
            | "those"
            | "can"
            | "help"
            | "need"
            | "want"
            | "give"
            | "make"
            | "provide"
            | "include"
            | "should"
            | "would"
            | "could"
            | "into"
    )
}

fn is_negation(token: &str) -> bool {
    matches!(
        token,
        "no" | "not"
            | "never"
            | "avoid"
            | "without"
            | "unless"
            | "except"
            | "only"
            | "cannot"
            | "can't"
            | "cant"
            | "won't"
            | "wont"
            | "don't"
            | "dont"
    )
}

fn preserves_any_negation(text: &str) -> bool {
    lexical_tokens(text)
        .iter()
        .any(|token| is_negation(&token.to_ascii_lowercase()))
}

fn has_request_wrapper_noise(input: &str) -> bool {
    let tokens = lexical_tokens(input)
        .into_iter()
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();

    tokens.windows(2).any(|window| {
        matches!(
            (window[0].as_str(), window[1].as_str()),
            ("help", "me")
                | ("write", "me")
                | ("draft", "me")
                | ("give", "me")
                | ("tell", "me")
                | ("show", "me")
                | ("i", "want")
                | ("i", "need")
                | ("i", "would")
                | ("i", "am")
                | ("can", "you")
                | ("could", "you")
                | ("would", "you")
        )
    }) || tokens
        .iter()
        .take(5)
        .any(|token| matches!(token.as_str(), "please" | "kindly"))
}

fn structured_drops_preserved_special_token(
    input: &str,
    tokelang_compact: &str,
    general_compact: &str,
) -> bool {
    let required = lexical_tokens(input)
        .into_iter()
        .map(|token| token.to_ascii_lowercase())
        .filter(|token| {
            token.chars().any(|ch| ch.is_ascii_digit())
                || is_negation(token)
                || is_constraint_word(token)
                || is_output_shape_word(token)
                || is_role_word(token)
        })
        .collect::<HashSet<_>>();
    if required.is_empty() {
        return false;
    }

    let structured = content_token_set(tokelang_compact);
    let general = content_token_set(general_compact);
    required
        .iter()
        .any(|token| !structured.contains(token) && general.contains(token))
}

fn is_constraint_word(token: &str) -> bool {
    matches!(
        token,
        "under"
            | "over"
            | "within"
            | "before"
            | "after"
            | "during"
            | "because"
            | "but"
            | "if"
            | "when"
            | "while"
            | "versus"
            | "instead"
            | "include"
            | "separate"
            | "rank"
            | "group"
            | "prioritize"
            | "limit"
            | "budget"
            | "deadline"
    )
}

fn is_output_shape_word(token: &str) -> bool {
    matches!(
        token,
        "list"
            | "checklist"
            | "table"
            | "rubric"
            | "timeline"
            | "plan"
            | "email"
            | "memo"
            | "essay"
            | "summary"
            | "outline"
            | "decision"
            | "tree"
            | "steps"
            | "step-by-step"
            | "questions"
            | "options"
            | "ideas"
            | "examples"
    )
}

fn is_action_or_control_word(token: &str) -> bool {
    is_output_shape_word(token)
        || matches!(
            token,
            "act"
                | "advise"
                | "ask"
                | "answer"
                | "build"
                | "choose"
                | "compare"
                | "compose"
                | "convert"
                | "create"
                | "debug"
                | "decide"
                | "design"
                | "develop"
                | "draft"
                | "explain"
                | "extract"
                | "find"
                | "generate"
                | "give"
                | "handle"
                | "identify"
                | "improve"
                | "keep"
                | "list"
                | "organize"
                | "plan"
                | "prepare"
                | "provide"
                | "recommend"
                | "reduce"
                | "reply"
                | "replace"
                | "research"
                | "rewrite"
                | "select"
                | "show"
                | "suggest"
                | "summarize"
                | "take"
                | "tell"
                | "trace"
                | "translate"
                | "turn"
                | "use"
                | "write"
                | "only"
                | "strictly"
        )
}

fn count_role_hits(tokens: &[String]) -> usize {
    let explicit_act_as = tokens
        .windows(2)
        .filter(|window| {
            window[0].eq_ignore_ascii_case("act") && window[1].eq_ignore_ascii_case("as")
        })
        .count()
        + tokens
            .windows(3)
            .filter(|window| {
                window[0].eq_ignore_ascii_case("as")
                    && matches!(window[1].to_ascii_lowercase().as_str(), "a" | "an")
                    && is_role_word(&window[2].to_ascii_lowercase())
            })
            .count();

    explicit_act_as
        + tokens
            .iter()
            .filter(|token| is_role_word(&token.to_ascii_lowercase()))
            .count()
}

fn is_role_word(token: &str) -> bool {
    matches!(
        token,
        "advisor"
            | "advertiser"
            | "behaviorist"
            | "coach"
            | "counselor"
            | "critic"
            | "decorator"
            | "dietitian"
            | "etymologist"
            | "expert"
            | "generator"
            | "guide"
            | "historian"
            | "interviewer"
            | "logistician"
            | "manager"
            | "mechanic"
            | "planner"
            | "recruiter"
            | "shopper"
            | "statistician"
            | "storyteller"
            | "teacher"
            | "trainer"
            | "translator"
            | "tutor"
            | "yogi"
    )
}

fn pct_savings(original: usize, compact: usize) -> f64 {
    if original == 0 {
        0.0
    } else {
        (1.0 - (compact as f64 / original as f64)) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokenizer;

    #[test]
    fn compresses_short_direct_request() {
        let tokenizer = Tokenizer::detect();
        let candidate = candidate(
            "Can you help me write an essay about photosynthesis?",
            &tokenizer,
        )
        .expect("short request should compress");
        assert!(candidate.compact.contains("write essay photosynthesis"));
    }

    #[test]
    fn preserves_constraints_and_negations() {
        let tokenizer = Tokenizer::detect();
        let input = "I need a grocery list for high-protein breakfasts that do not require protein powder, are not sweet, and can be made before work.";
        let candidate = candidate(input, &tokenizer).expect("constraint prompt should compress");
        assert!(candidate.compact.contains("not require protein powder"));
        assert!(candidate.compact.contains("not sweet"));
        assert!(candidate.compact.contains("before work"));
    }

    #[test]
    fn source_retention_keeps_validator_critical_tokens() {
        let tokenizer = Tokenizer::detect();
        let input = "Please create a JSON checklist before Friday, include exactly 3 items, and only reply with pseudocode.";

        let candidate =
            candidate(input, &tokenizer).expect("critical-token prompt should compress");

        for anchor in [
            "JSON",
            "checklist",
            "before",
            "Friday",
            "include",
            "exactly",
            "3",
            "only",
            "pseudocode",
        ] {
            assert!(
                candidate.compact.contains(anchor),
                "expected `{anchor}` in compact output:\n{}",
                candidate.compact
            );
        }
    }

    #[test]
    fn preserves_bracket_placeholder_exactly() {
        let tokenizer = Tokenizer::detect();
        let input =
            "Can you please summarize the following meeting notes into a checklist: [paste notes].";

        let candidate = candidate(input, &tokenizer).expect("placeholder prompt should compress");

        assert!(candidate.compact.contains("[paste notes]"));
        assert!(!candidate.compact.contains("Can you please"));
        assert!(candidate.compact.contains("summarize"));
        assert!(candidate.compact.contains("checklist"));
    }

    #[test]
    fn preserves_template_placeholder_exactly() {
        let tokenizer = Tokenizer::detect();
        let input = "I want you to only reply as the interviewer for the ${Position:Software Developer} position. Do not write explanations.";

        let candidate =
            candidate(input, &tokenizer).expect("template placeholder prompt should compress");

        assert!(candidate.compact.contains("${Position:Software Developer}"));
        assert!(candidate.compact.contains("only reply"));
        assert!(candidate.compact.contains("not write explanations"));
    }

    #[test]
    fn preserves_quoted_payload_exactly() {
        let tokenizer = Tokenizer::detect();
        let input = "Please act as an interviewer, ask one question, and wait for my answer. My first sentence is \"Hi\".";

        let candidate =
            candidate(input, &tokenizer).expect("quoted-payload prompt should compress");

        assert!(candidate.compact.contains("\"Hi\""));
        assert!(candidate.compact.contains("act as interviewer"));
        assert!(candidate.compact.contains("ask one question"));
        assert!(candidate.compact.contains("wait answer"));
    }

    #[test]
    fn preserves_hard_zone_internal_spacing() {
        let tokenizer = Tokenizer::detect();
        let input = "Please summarize this placeholder without changing it: [paste   exact notes].";

        let candidate =
            candidate(input, &tokenizer).expect("spacing placeholder prompt should compress");

        assert!(candidate.compact.contains("[paste   exact notes]"));
    }

    #[test]
    fn role_prompt_prefers_general_over_losy_structured_output() {
        let tokenizer = Tokenizer::detect();
        let input = "I want you to act as a etymologist. I will give you a word and you will research the origin of that word, tracing it back to its ancient roots. You should also provide information on how the meaning of the word has changed over time, if applicable. My first request is \"I want to trace the origins of the word 'pizza'.\"";
        let lossy =
            "process\ndefine also provide information meaning word changed time applicable simple";
        let candidate = candidate(input, &tokenizer).expect("role prompt should compress");
        assert!(
            should_prefer_general(input, lossy, &candidate),
            "candidate should beat lossy structured output:\n{candidate:#?}"
        );
    }
}
