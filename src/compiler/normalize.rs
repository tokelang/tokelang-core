use crate::symbols::is_reserved_symbol;

static STOP_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "this",
    "that",
    "these",
    "those",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "shall",
    "may",
    "might",
    "can",
    "to",
    "of",
    "in",
    "for",
    "on",
    "with",
    "at",
    "by",
    "from",
    "as",
    "into",
    "about",
    "between",
    "through",
    "during",
    "before",
    "after",
    "above",
    "below",
    "up",
    "down",
    "out",
    "off",
    "over",
    "under",
    "again",
    "further",
    "once",
    "here",
    "there",
    "when",
    "where",
    "why",
    "how",
    "all",
    "each",
    "every",
    "both",
    "few",
    "more",
    "most",
    "other",
    "some",
    "such",
    "no",
    "nor",
    "not",
    "only",
    "own",
    "same",
    "so",
    "than",
    "too",
    "very",
    "just",
    "because",
    "but",
    "and",
    "or",
    "if",
    "while",
    "although",
    "please",
    "me",
    "my",
    "i",
    "you",
    "your",
    "it",
    "its",
    "we",
    "our",
    "they",
    "their",
    "what",
    "which",
    "who",
    "whom",
    "following",
    "given",
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "finally",
    "next",
    "then",
    "return",
];

static DESCRIPTOR_WORDS: &[&str] = &[
    "carefully",
    "thoroughly",
    "deep",
    "deeply",
    "useful",
    "important",
    "emerging",
    "likely",
    "relatively",
    "stable",
    "common",
    "basic",
    "raw",
    "final",
    "possible",
    "societal",
    "multiple",
    "single",
    "different",
];

static COMMON_NOISE_CHARS: &[char] = &[
    '¨', '©', 'ª', '«', '¬', '®', '¯', '°', '±', '²', '³', 'µ', '¶', '¹', 'º', '»', '¼', '½', '¾',
    'À', 'Á',
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProtectedContentStats {
    pub total_chars: usize,
    pub protected_chars: usize,
}

pub fn escape_reserved_symbols(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        if is_reserved_symbol(character) {
            escaped.push('Ξ');
        }
        escaped.push(character);
    }
    escaped
}

pub fn clean_input(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_alphanumeric()
                || character.is_whitespace()
                || character == '-'
                || is_reserved_symbol(character)
            {
                if is_reserved_symbol(character) {
                    character
                } else {
                    character.to_ascii_lowercase()
                }
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn tokenize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(normalize_token)
        .collect()
}

pub fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word)
}

pub fn is_descriptor_word(word: &str) -> bool {
    DESCRIPTOR_WORDS.contains(&word)
}

pub fn canonicalize_term(term: &str) -> String {
    term.chars()
        .map(|character| {
            if character.is_ascii_alphabetic() {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn protected_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = fenced_code_ranges(input);
    ranges.extend(inline_code_ranges(input, &ranges));
    merge_ranges(ranges)
}

pub(crate) fn strip_protected_content(input: &str) -> String {
    let masked = mask_ranges_preserving_newlines(input, &protected_ranges(input));
    strip_equation_content(&masked)
}

pub(crate) fn protected_content_stats(input: &str) -> ProtectedContentStats {
    let total_chars = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let ranges = protected_ranges(input);
    let mut protected_chars = count_non_whitespace_chars_in_ranges(input, &ranges);
    let masked = mask_ranges_preserving_newlines(input, &ranges);

    for line in masked.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_equation_heavy_line(trimmed) {
            protected_chars += trimmed
                .chars()
                .filter(|character| !character.is_whitespace())
                .count();
            continue;
        }

        if let Some((_, suffix)) = split_equation_suffix(trimmed) {
            protected_chars += suffix
                .chars()
                .filter(|character| !character.is_whitespace())
                .count();
        }
    }

    ProtectedContentStats {
        total_chars,
        protected_chars,
    }
}

pub(crate) fn is_equation_heavy_line(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    trimmed.starts_with("$$")
        || trimmed.ends_with("$$")
        || trimmed.contains("\\frac")
        || (trimmed.contains('=')
            && contains_equation_fragment(trimmed)
            && !trimmed.to_ascii_lowercase().starts_with("write "))
}

fn normalize_token(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(is_noise_boundary_char);
    if trimmed.is_empty() || is_noise_only_token(trimmed) {
        return None;
    }

    Some(trimmed.to_string())
}

fn is_noise_boundary_char(character: char) -> bool {
    character == 'Ξ' || is_reserved_symbol(character) || COMMON_NOISE_CHARS.contains(&character)
}

fn is_noise_only_token(token: &str) -> bool {
    !token
        .chars()
        .any(|character| character.is_ascii_alphanumeric())
        && token.chars().all(is_noise_boundary_char)
}

fn fenced_code_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut search_start = 0usize;

    while let Some(relative_start) = input[search_start..].find("```") {
        let start = search_start + relative_start;
        let fence_body_start = start + 3;
        let Some(relative_end) = input[fence_body_start..].find("```") else {
            ranges.push((start, input.len()));
            break;
        };
        let end = fence_body_start + relative_end + 3;
        ranges.push((start, end));
        search_start = end;
    }

    ranges
}

fn inline_code_ranges(input: &str, fenced_ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        if is_within_ranges(index, fenced_ranges) || bytes[index] != b'`' {
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < bytes.len() {
            if bytes[index] == b'`' && !is_within_ranges(index, fenced_ranges) {
                ranges.push((start, index + 1));
                index += 1;
                break;
            }
            index += 1;
        }
    }

    ranges
}

fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }

    ranges.sort_unstable_by_key(|range| range.0);
    let mut merged = vec![ranges[0]];

    for (start, end) in ranges.into_iter().skip(1) {
        let last = merged.last_mut().expect("merged should be non-empty");
        if start <= last.1 {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }

    merged
}

fn is_within_ranges(index: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= index && index < *end)
}

fn mask_ranges_preserving_newlines(input: &str, ranges: &[(usize, usize)]) -> String {
    let mut bytes = input.as_bytes().to_vec();

    for (start, end) in ranges {
        for byte in bytes.iter_mut().take(*end).skip(*start) {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }

    String::from_utf8(bytes).expect("masked text should remain valid utf-8")
}

fn strip_equation_content(input: &str) -> String {
    let mut output = String::with_capacity(input.len());

    for (line_index, line) in input.split('\n').enumerate() {
        if line_index > 0 {
            output.push('\n');
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_equation_heavy_line(trimmed) {
            continue;
        }

        if let Some((prefix, _)) = split_equation_suffix(line) {
            output.push_str(prefix.trim_end());
        } else {
            output.push_str(line);
        }
    }

    output
}

fn split_equation_suffix(line: &str) -> Option<(&str, &str)> {
    let colon = line.rfind(':')?;
    let prefix = &line[..colon];
    let suffix = line[colon + 1..].trim();
    if is_equation_fragment(suffix) {
        Some((prefix, suffix))
    } else {
        None
    }
}

fn contains_equation_fragment(text: &str) -> bool {
    is_equation_fragment(text)
}

fn is_equation_fragment(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let has_latex = trimmed.contains("\\frac")
        || trimmed.contains("\\sum")
        || trimmed.contains("\\int")
        || trimmed.contains("\\sqrt");
    let has_digits = trimmed.chars().any(|character| character.is_ascii_digit());
    let has_operator = trimmed
        .chars()
        .any(|character| matches!(character, '=' | '+' | '*' | '/' | '^'));
    let has_math_call = trimmed.char_indices().any(|(index, character)| {
        character == '('
            && index > 0
            && trimmed[..index]
                .chars()
                .last()
                .is_some_and(|previous| previous.is_ascii_alphabetic())
    });

    has_latex
        || (has_operator && (has_digits || has_math_call || trimmed.contains('=')))
        || (trimmed.contains('=') && (has_digits || has_math_call))
}

fn count_non_whitespace_chars_in_ranges(input: &str, ranges: &[(usize, usize)]) -> usize {
    ranges
        .iter()
        .map(|(start, end)| {
            input[*start..*end]
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_words_strips_noise_wrappers_from_semantic_tokens() {
        let words = tokenize_words("Ξ¡Ξ¡ph 10 00Ξ¡Ξ¡");
        assert_eq!(words, vec!["ph", "10", "00"]);
    }

    #[test]
    fn tokenize_words_drops_noise_only_tokens() {
        let words = tokenize_words("Ξ¡Ξ¡Ξ¢Ξ£ ²³µ ÀÁ");
        assert!(words.is_empty());
    }

    #[test]
    fn strip_protected_content_removes_inline_code_and_equation_suffix() {
        let stripped = strip_protected_content(
            "Explain this code: `for i in range(3): print(i)`\nNow assume noise is added: f(x) + random(-2, 2)",
        );

        assert!(stripped.contains("Explain this code:"));
        assert!(stripped.contains("Now assume noise is added"));
        assert!(!stripped.contains("range(3)"));
        assert!(!stripped.contains("random(-2, 2)"));
    }

    #[test]
    fn protected_content_stats_counts_code_block_payload() {
        let stats = protected_content_stats(
            "Explain this:\n```python\nfor i in range(3):\n    print(i)\n```",
        );

        assert!(stats.protected_chars > 0);
        assert!(stats.total_chars >= stats.protected_chars);
    }
}
