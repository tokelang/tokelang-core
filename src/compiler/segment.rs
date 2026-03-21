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

/// Span-aware clause emitted by the segmenter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClauseSpan {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub marker: Option<SequenceMarker>,
}

pub fn split_clauses(input: &str, synonyms: &SynonymTable) -> Vec<ClauseSpan> {
    let mut first_pass = Vec::new();
    let mut start = 0usize;

    for (index, character) in input.char_indices() {
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

        if should_split_on_tail(tail, synonyms) {
            let marker = if local_start == sentence.start {
                sentence.marker
            } else {
                None
            };
            push_trimmed_with_base(
                sentence.text.as_str(),
                local_start - sentence.start,
                relative_index,
                marker,
                output,
                sentence.start,
            );
            local_start = absolute_index + character.len_utf8();
        }
    }

    let marker = if local_start == sentence.start {
        sentence.marker
    } else {
        None
    };
    push_trimmed_with_base(
        sentence.text.as_str(),
        local_start - sentence.start,
        sentence.text.len(),
        marker,
        output,
        sentence.start,
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

    let cleaned = normalize::clean_input(trimmed);
    if let Some((sequence_marker, marker_len)) = detect_sequence_marker(&cleaned) {
        let raw_after_marker = trimmed[marker_len..]
            .trim_start_matches([',', ':', ';', ' '])
            .trim();
        if !raw_after_marker.is_empty() {
            let consumed = trimmed.len() - raw_after_marker.len();
            absolute_start += consumed;
            text = raw_after_marker.to_string();
            detected_marker = Some(sequence_marker);
        }
    }

    output.push(ClauseSpan {
        start: absolute_start,
        end: absolute_end,
        text,
        marker: detected_marker,
    });
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
}
