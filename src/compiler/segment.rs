use crate::compiler::normalize;
use crate::symbols::SynonymTable;

/// Ordering markers extracted from prompt prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceMarker {
    First,
    Next,
    Then,
    AfterThat,
    Finally,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarkerKind {
    Bullet,
    Numbered,
}

/// Span-aware clause emitted by the segmenter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClauseSpan {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub cleaned_text: String,
    pub marker: Option<SequenceMarker>,
    pub indent: usize,
    pub is_list_item: bool,
    pub list_marker_kind: Option<ListMarkerKind>,
}

impl ClauseSpan {
    pub fn new(
        start: usize,
        end: usize,
        text: String,
        marker: Option<SequenceMarker>,
        indent: usize,
        is_list_item: bool,
        list_marker_kind: Option<ListMarkerKind>,
    ) -> Self {
        let cleaned_text = normalize::clean_input(&text);
        Self {
            start,
            end,
            text,
            cleaned_text,
            marker,
            indent,
            is_list_item,
            list_marker_kind,
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cleaned_text = normalize::clean_input(&self.text);
    }

    pub fn append_text(&mut self, suffix: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        self.text.push_str(suffix);
        self.cleaned_text = normalize::clean_input(&self.text);
    }
}

pub fn split_clauses(input: &str, synonyms: &SynonymTable) -> Vec<ClauseSpan> {
    let mut first_pass = Vec::new();
    let mut start = 0usize;
    let protected_ranges = normalize::protected_ranges(input);

    for (index, character) in input.char_indices() {
        if is_inside_protected_range(index, &protected_ranges) {
            continue;
        }

        if matches!(character, '.' | '?' | '!') && is_inside_literal_payload_line(input, index) {
            continue;
        }

        if character == '.' && is_numbered_list_period(input, start, index) {
            continue;
        }

        if matches!(character, '.' | '?' | '!' | '\n') {
            push_trimmed(input, start, index, None, &mut first_pass);
            start = index + character.len_utf8();
        }
    }

    push_trimmed(input, start, input.len(), None, &mut first_pass);

    let mut second_pass = Vec::new();
    for sentence in first_pass {
        split_sentence(sentence, synonyms, &mut second_pass);
    }

    second_pass
}

fn split_sentence(sentence: ClauseSpan, synonyms: &SynonymTable, output: &mut Vec<ClauseSpan>) {
    if should_preserve_literal_sentence(&sentence.text) {
        output.push(sentence);
        return;
    }

    let mut local_start = sentence.start;
    let text = &sentence.text;

    for (relative_index, character) in text.char_indices() {
        if !matches!(character, ',' | ';') {
            continue;
        }

        let absolute_index = sentence.start + relative_index;
        let head = &text[..relative_index];
        let tail = &text[relative_index + character.len_utf8()..];

        if is_marker_only(head) {
            continue;
        }

        if should_split_on_tail(tail, synonyms)
            && !should_keep_controller_tail_joined(head, tail, synonyms)
        {
            let marker = if local_start == sentence.start {
                sentence.marker
            } else {
                None
            };
            push_trimmed_with_inherited_metadata(
                sentence.text.as_str(),
                local_start - sentence.start,
                relative_index,
                marker,
                output,
                sentence.start,
                sentence.indent,
                sentence.is_list_item,
                sentence.list_marker_kind,
            );
            local_start = absolute_index + character.len_utf8();
        }
    }

    let marker = if local_start == sentence.start {
        sentence.marker
    } else {
        None
    };
    push_trimmed_with_inherited_metadata(
        sentence.text.as_str(),
        local_start - sentence.start,
        sentence.text.len(),
        marker,
        output,
        sentence.start,
        sentence.indent,
        sentence.is_list_item,
        sentence.list_marker_kind,
    );
}

fn should_split_on_tail(tail: &str, synonyms: &SynonymTable) -> bool {
    let lowered = normalize::clean_input(tail);
    if lowered.is_empty() {
        return false;
    }

    if detect_sequence_marker(&lowered).is_some() {
        return true;
    }

    let words = normalize::tokenize_words(&lowered);
    synonyms.starts_with_instruction(&words)
}

fn should_keep_controller_tail_joined(head: &str, tail: &str, synonyms: &SynonymTable) -> bool {
    let head_cleaned = normalize::clean_input(head);
    if !head_cleaned.starts_with("if ")
        && !head_cleaned.starts_with("otherwise ")
        && !head_cleaned.starts_with("else ")
    {
        return false;
    }

    let tail_cleaned = normalize::clean_input(tail);
    let words = normalize::tokenize_words(&tail_cleaned);
    if words.is_empty() || words.len() > 4 || !synonyms.starts_with_instruction(&words) {
        return false;
    }

    !["keep ", "preserve ", "retain ", "ensure ", "return ", "output "]
        .iter()
        .any(|prefix| tail_cleaned.starts_with(prefix))
}

fn is_marker_only(text: &str) -> bool {
    let lowered = normalize::clean_input(text);
    matches!(
        lowered.as_str(),
        "first"
            | "second"
            | "third"
            | "fourth"
            | "fifth"
            | "sixth"
            | "next"
            | "then"
            | "after that"
            | "finally"
    )
}

fn detect_sequence_marker(text: &str) -> Option<(SequenceMarker, usize)> {
    let patterns = [
        ("after that", SequenceMarker::AfterThat),
        ("finally", SequenceMarker::Finally),
        ("first", SequenceMarker::First),
        ("second", SequenceMarker::Next),
        ("third", SequenceMarker::Next),
        ("fourth", SequenceMarker::Next),
        ("fifth", SequenceMarker::Next),
        ("sixth", SequenceMarker::Next),
        ("next", SequenceMarker::Next),
        ("then", SequenceMarker::Then),
    ];

    for (pattern, marker) in patterns {
        if let Some(remainder) = text.strip_prefix(pattern)
            && (remainder.is_empty() || remainder.starts_with(' '))
        {
            return Some((marker, pattern.len()));
        }
    }

    None
}

fn should_preserve_literal_sentence(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    is_tuple_like_payload(trimmed)
        || is_parenthesized_schema_payload(trimmed)
        || is_json_like_payload(trimmed)
        || is_log_like_payload(trimmed)
        || trimmed.contains("```")
        || normalize::is_equation_heavy_line(trimmed)
}

fn is_tuple_like_payload(text: &str) -> bool {
    text.starts_with('(')
        && text.ends_with(')')
        && text.contains(',')
        && (text.chars().any(|character| character.is_ascii_digit())
            || text.contains("->")
            || text.contains(':'))
}

fn is_parenthesized_schema_payload(text: &str) -> bool {
    if !(text.starts_with('(') && text.ends_with(')') && text.contains(',')) {
        return false;
    }

    let inner = &text[1..text.len() - 1];
    let cells = inner
        .split(',')
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>();

    cells.len() >= 3
        && !inner.chars().any(|character| character.is_ascii_digit())
        && cells.iter().all(|cell| {
            let words = cell.split_whitespace().collect::<Vec<_>>();
            !words.is_empty()
                && words.len() <= 3
                && words.iter().all(|word| {
                    word.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '/')
                    })
                })
        })
}

fn is_json_like_payload(text: &str) -> bool {
    ((text.starts_with('{') && text.ends_with('}'))
        || (text.starts_with('[') && text.ends_with(']')))
        && text.contains(':')
}

fn is_log_like_payload(text: &str) -> bool {
    let prefix = text
        .split_once(':')
        .map(|(head, _)| normalize::clean_input(head))
        .unwrap_or_default();
    let starts_with_log_marker = matches!(
        prefix.as_str(),
        "panic" | "error" | "warning" | "traceback" | "exception" | "fatal"
    );
    let cleaned = normalize::clean_input(text);
    let has_location_marker = cleaned.contains(" row ")
        || cleaned.contains(" line ")
        || text.contains(" at ")
        || text.contains("::")
        || text.contains('/')
        || text.contains('\\');

    starts_with_log_marker && has_location_marker
}

fn is_inside_protected_range(index: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= index && index < *end)
}

fn is_inside_literal_payload_line(input: &str, index: usize) -> bool {
    let line_start = input[..index]
        .rfind('\n')
        .map(|offset| offset + 1)
        .unwrap_or(0);
    let line_end = input[index..]
        .find('\n')
        .map(|offset| index + offset)
        .unwrap_or(input.len());
    let trimmed = input[line_start..line_end].trim();
    should_preserve_literal_sentence(trimmed)
}

fn push_trimmed(
    input: &str,
    start: usize,
    end: usize,
    marker: Option<SequenceMarker>,
    output: &mut Vec<ClauseSpan>,
) {
    push_trimmed_with_base(input, start, end, marker, output, 0);
}

fn push_trimmed_with_base(
    input: &str,
    start: usize,
    end: usize,
    marker: Option<SequenceMarker>,
    output: &mut Vec<ClauseSpan>,
    base_offset: usize,
) {
    if start >= end {
        return;
    }

    let slice = &input[start..end];
    let leading = slice.len() - slice.trim_start().len();
    let trailing = slice.len() - slice.trim_end().len();
    let trimmed = slice.trim();
    if trimmed.is_empty() {
        return;
    }

    let mut absolute_start = base_offset + start + leading;
    let absolute_end = base_offset + end - trailing;
    let mut text = trimmed.to_string();
    let mut detected_marker = marker;
    let mut is_list_item = false;
    let mut list_marker_kind = None;

    if let Some((trimmed_after_list_marker, consumed, marker_kind)) =
        strip_leading_list_marker(trimmed)
    {
        absolute_start += consumed;
        text = trimmed_after_list_marker.to_string();
        is_list_item = true;
        list_marker_kind = Some(marker_kind);
    }

    let cleaned = normalize::clean_input(&text);
    if let Some((sequence_marker, marker_len)) = detect_sequence_marker(&cleaned) {
        let raw_after_marker = text[marker_len..]
            .trim_start_matches([',', ':', ';', ' '])
            .trim();
        if !raw_after_marker.is_empty() {
            let consumed = text.len() - raw_after_marker.len();
            absolute_start += consumed;
            text = raw_after_marker.to_string();
            detected_marker = Some(sequence_marker);
        }
    }

    output.push(ClauseSpan::new(
        absolute_start,
        absolute_end,
        text,
        detected_marker,
        leading,
        is_list_item,
        list_marker_kind,
    ));
}

fn push_trimmed_with_inherited_metadata(
    input: &str,
    start: usize,
    end: usize,
    marker: Option<SequenceMarker>,
    output: &mut Vec<ClauseSpan>,
    base_offset: usize,
    inherited_indent: usize,
    inherited_list_item: bool,
    inherited_marker_kind: Option<ListMarkerKind>,
) {
    let original_len = output.len();
    push_trimmed_with_base(input, start, end, marker, output, base_offset);

    if let Some(clause) = output.get_mut(original_len) {
        clause.indent = inherited_indent;
        clause.is_list_item |= inherited_list_item;
        if clause.list_marker_kind.is_none() {
            clause.list_marker_kind = inherited_marker_kind;
        }
    }
}

fn is_numbered_list_period(input: &str, start: usize, index: usize) -> bool {
    let Some(remainder) = input.get(index + 1..) else {
        return false;
    };

    let trimmed = input[start..index].trim_start();
    !trimmed.is_empty()
        && trimmed.chars().all(|character| character.is_ascii_digit())
        && remainder
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace())
}

fn strip_leading_list_marker(text: &str) -> Option<(&str, usize, ListMarkerKind)> {
    for marker in ["- ", "* "] {
        if let Some(remainder) = text.strip_prefix(marker) {
            let trimmed = remainder.trim_start();
            if !trimmed.is_empty() {
                return Some((trimmed, text.len() - trimmed.len(), ListMarkerKind::Bullet));
            }
        }
    }

    let digits_len = text
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if digits_len == 0 {
        return None;
    }

    let remainder = &text[digits_len..];
    let Some(remainder) = remainder
        .strip_prefix('.')
        .or_else(|| remainder.strip_prefix(')'))
        .or_else(|| remainder.strip_prefix(':'))
    else {
        return None;
    };

    let trimmed = remainder.trim_start();
    if trimmed.is_empty() {
        None
    } else {
        Some((
            trimmed,
            text.len() - trimmed.len(),
            ListMarkerKind::Numbered,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_leading_marker_with_instruction() {
        let clauses = split_clauses(
            "First, explain neural networks. Then, summarize them.",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].marker, Some(SequenceMarker::First));
        assert_eq!(clauses[0].text, "explain neural networks");
    }

    #[test]
    fn does_not_split_on_descriptive_comma() {
        let clauses = split_clauses(
            "Explain neural networks, including their training process and limitations.",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 1);
    }

    #[test]
    fn does_not_split_tuple_like_example_row_on_commas() {
        let clauses = split_clauses("(10:04, D, MODIFY, 6 -> 9)", &SynonymTable::default_table());
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].text, "(10:04, D, MODIFY, 6 -> 9)");
    }

    #[test]
    fn does_not_split_decimal_tuple_like_example_row_on_periods() {
        let clauses = split_clauses("(Section-A, Budget, 4.2M)", &SynonymTable::default_table());
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].text, "(Section-A, Budget, 4.2M)");
    }

    #[test]
    fn keeps_parenthesized_schema_row_as_single_clause() {
        let clauses = split_clauses(
            "(time, service, signal, note)",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].text, "(time, service, signal, note)");
    }

    #[test]
    fn does_not_split_numbered_list_marker_from_instruction_text() {
        let clauses = split_clauses(
            "1. Detect anomaly.\n2. Then reconstruct it exactly.",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].text, "Detect anomaly");
        assert_eq!(clauses[1].text, "reconstruct it exactly");
        assert_eq!(clauses[1].marker, Some(SequenceMarker::Then));
    }

    #[test]
    fn keeps_short_investigate_tail_joined_to_prior_controller_clause() {
        let clauses = split_clauses(
            "- If alerts cluster around one region, investigate routing failure",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 1);
        assert_eq!(
            clauses[0].text,
            "If alerts cluster around one region, investigate routing failure"
        );
    }

    #[test]
    fn strips_leading_bullet_markers_from_clause_text() {
        let clauses = split_clauses(
            "- Optimize for latency\n* Minimize memory usage",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].text, "Optimize for latency");
        assert_eq!(clauses[1].text, "Minimize memory usage");
    }

    #[test]
    fn keeps_fenced_code_block_as_single_clause() {
        let clauses = split_clauses(
            "Explain the bug.\n```python\nfor i in range(3):\n    print(i)\n```\nThen summarize it.",
            &SynonymTable::default_table(),
        );

        assert_eq!(clauses.len(), 3);
        assert!(clauses[1].text.contains("```python"));
        assert!(clauses[1].text.contains("print(i)"));
    }

    #[test]
    fn does_not_split_equation_like_line_on_internal_commas() {
        let clauses = split_clauses("f(x, y) = x^2 + y^2", &SynonymTable::default_table());
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].text, "f(x, y) = x^2 + y^2");
    }

    #[test]
    fn keeps_log_like_line_as_single_clause() {
        let clauses = split_clauses(
            "panic: unexpected null pointer at row 44",
            &SynonymTable::default_table(),
        );
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].text, "panic: unexpected null pointer at row 44");
    }
}
