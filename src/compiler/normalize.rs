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
    text.split_whitespace().map(ToString::to_string).collect()
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
