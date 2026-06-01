use std::collections::HashSet;

const NORMAL_RECALL_FLOOR: f64 = 0.68;
const RISKY_RECALL_FLOOR: f64 = 0.74;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidationReport {
    pub passed: bool,
    pub content_recall: f64,
    pub missing_critical_tokens: Vec<String>,
}

pub(crate) fn validate_compact(original: &str, compact: &str) -> ValidationReport {
    let content_recall = content_recall(original, compact);
    let missing_critical_tokens = missing_critical_tokens(original, compact);
    let floor = recall_floor(original);
    let passed = content_recall >= floor && missing_critical_tokens.is_empty();

    ValidationReport {
        passed,
        content_recall,
        missing_critical_tokens,
    }
}

fn recall_floor(original: &str) -> f64 {
    let tokens = lexical_tokens(original);
    let critical_count = tokens
        .iter()
        .filter(|token| is_critical_token(&token.to_ascii_lowercase()))
        .count();
    let content_count = content_token_set_from_tokens(&tokens).len();

    if critical_count >= 3 || content_count >= 18 {
        RISKY_RECALL_FLOOR
    } else {
        NORMAL_RECALL_FLOOR
    }
}

fn missing_critical_tokens(original: &str, compact: &str) -> Vec<String> {
    let compact_tokens = lexical_tokens(compact)
        .into_iter()
        .map(|token| token.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut missing = Vec::new();

    for token in lexical_tokens(original) {
        let lower = token.to_ascii_lowercase();
        if !is_critical_token(&lower) || compact_tokens.contains(&lower) || !seen.insert(lower) {
            continue;
        }
        missing.push(token);
    }

    missing
}

fn content_recall(original: &str, compact: &str) -> f64 {
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

fn lexical_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric()
            || matches!(
                ch,
                '_' | '-' | '/' | '@' | '$' | '%' | ':' | '.' | '\'' | '+' | '#' | '='
                | '<' | '>' | '*'
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

fn is_content_token(token: &str) -> bool {
    if token.chars().any(|ch| ch.is_ascii_digit()) {
        return true;
    }
    if is_critical_token(token) {
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
            | "should"
            | "would"
            | "could"
            | "will"
            | "some"
            | "also"
            | "like"
            | "type"
            | "kind"
            | "order"
            | "into"
            | "about"
            | "being"
            | "been"
            | "have"
            | "has"
            | "had"
            | "was"
            | "were"
            | "does"
            | "did"
            | "needed"
            | "things"
    )
}

pub(crate) fn is_critical_token(token: &str) -> bool {
    is_negation(token)
        || is_constraint_word(token)
        || is_output_shape_word(token)
        || is_temporal_word(token)
        || is_exactish_token(token)
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

fn is_constraint_word(token: &str) -> bool {
    matches!(
        token,
        "under"
            | "over"
            | "within"
            | "before"
            | "after"
            | "because"
            | "but"
            | "if"
            | "when"
            | "while"
            | "versus"
            | "instead"
            | "include"
            | "includes"
            | "separate"
            | "rank"
            | "group"
            | "prioritize"
            | "limit"
            | "budget"
            | "deadline"
            | "required"
            | "must"
            | "strictly"
            | "exactly"
            | "intact"
            | "preserve"
    )
}

fn is_output_shape_word(token: &str) -> bool {
    matches!(
        token,
        "list"
            | "table"
            | "checklist"
            | "rubric"
            | "outline"
            | "summary"
            | "memo"
            | "brief"
            | "script"
            | "email"
            | "pseudocode"
            | "json"
            | "question"
            | "questions"
            | "answer"
            | "answers"
            | "recipe"
            | "itinerary"
            | "announcement"
            | "message"
            | "paragraph"
    )
}

fn is_temporal_word(token: &str) -> bool {
    matches!(
        token,
        "monday"
            | "tuesday"
            | "wednesday"
            | "thursday"
            | "friday"
            | "saturday"
            | "sunday"
            | "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
    )
}

fn is_exactish_token(token: &str) -> bool {
    token.chars().any(|ch| ch.is_ascii_digit())
        || token.contains('@')
        || token.contains('/')
        || token.contains('_')
        || token.contains('$')
        || token.contains('%')
        || token.contains('#')
        || token.contains('=')
        || token.contains('<')
        || token.contains('>')
        || token.contains('*')
        || token.contains("://")
}

#[cfg(test)]
mod tests {
    use super::validate_compact;

    #[test]
    fn accepts_telegraphic_prompt_when_critical_meaning_survives() {
        let original = "Create a vegetarian recipe for 2 people with 500 calories per serving and a low glycemic index.";
        let compact =
            "create vegetarian recipe 2 people 500 calories per serving low glycemic index";

        let report = validate_compact(original, compact);

        assert!(report.passed, "{report:?}");
    }

    #[test]
    fn rejects_missing_negation_and_budget() {
        let original =
            "Suggest five dress options under 100 dollars and only reply with the items.";
        let compact = "suggest five dress options reply items";

        let report = validate_compact(original, compact);

        assert!(!report.passed);
        assert!(report.missing_critical_tokens.contains(&"under".into()));
        assert!(report.missing_critical_tokens.contains(&"100".into()));
        assert!(report.missing_critical_tokens.contains(&"only".into()));
    }

    #[test]
    fn rejects_low_content_recall_even_without_specific_critical_tokens() {
        let original = "Explain the bathroom ceiling leak, repeated reports, spreading damp patch, repair date, and polite landlord tone.";
        let compact = "explain landlord tone";

        let report = validate_compact(original, compact);

        assert!(!report.passed);
        assert!(report.content_recall < 0.74);
    }
}
