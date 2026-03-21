use crate::ir::{
    BlockType, ContextFlags, Entity, OutputHint, Relation, RelationKind, SemanticFrame,
    TokelangBlock, TokelangIR, TokelangProgram,
};
use crate::symbols::{Instruction, Modifier, OutputFormat};

use super::error::ParseError;

pub fn parse_program(input: &str) -> Result<TokelangProgram, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyInput);
    }

    let mut lines = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .peekable();

    let mut prefix_flags = ContextFlags::default();
    if let Some(first_line) = lines.peek().copied()
        && (first_line.starts_with('Φ') || first_line.starts_with('Ψ'))
    {
        parse_prefix_line(first_line, &mut prefix_flags);
        lines.next();
    }

    let mut blocks = Vec::new();
    let mut current_block = TokelangBlock::new(BlockType::Default);

    for line in lines {
        if let Some(block_type) = BlockType::from_marker(line) {
            if !current_block.items.is_empty() {
                blocks.push(current_block);
            }
            current_block = TokelangBlock::new(block_type);
            continue;
        }

        let item = parse_item_line(line)?;
        current_block.items.push(item);
    }

    if !current_block.items.is_empty() {
        blocks.push(current_block);
    }

    if let Some(first_block) = blocks.first_mut()
        && let Some(first_item) = first_block.items.first_mut()
    {
        if first_item.flags.role.is_none() {
            first_item.flags.role = prefix_flags.role;
        }
        if first_item.flags.audience.is_none() {
            first_item.flags.audience = prefix_flags.audience;
        }
    }

    Ok(TokelangProgram { blocks })
}

fn parse_prefix_line(line: &str, flags: &mut ContextFlags) {
    for part in line.split_whitespace() {
        if let Some(role) = part.strip_prefix('Φ') {
            flags.role = Some(role.to_string());
        }
        if let Some(audience) = part.strip_prefix('Ψ') {
            flags.audience = Some(audience.to_string());
        }
    }
}

fn parse_item_line(line: &str) -> Result<TokelangIR, ParseError> {
    let mut remainder = line;
    let mut sequence_id = None;

    if let Some(index) = remainder.find('>') {
        let prefix = &remainder[..index];
        if prefix.chars().all(|ch| ch.is_ascii_digit()) {
            sequence_id = Some(
                prefix
                    .parse::<usize>()
                    .map_err(|_| ParseError::InvalidSequence(line.to_string()))?,
            );
            remainder = &remainder[index + 1..];
        }
    }

    let mut chars = remainder.chars();
    let instruction_char = chars
        .next()
        .ok_or_else(|| ParseError::MissingInstruction(line.to_string()))?;
    let instruction = Instruction::from_mnemonic(&instruction_char.to_string())
        .ok_or_else(|| ParseError::UnknownInstruction(instruction_char.to_string()))?;
    let payload = chars.as_str();

    let (subject_payload, modifiers) = split_modifiers(payload);
    let frame = parse_frame(subject_payload);

    Ok(TokelangIR {
        sequence_id,
        instruction,
        frame,
        modifiers,
        flags: ContextFlags::default(),
        source_span: None,
        recovered_from_coverage: false,
    })
}

fn split_modifiers(payload: &str) -> (&str, Vec<Modifier>) {
    let chars = payload.char_indices().collect::<Vec<_>>();
    let mut modifiers = Vec::new();
    let mut cut = payload.len();
    let mut index = chars.len();

    while index > 0 {
        let (byte_index, character) = chars[index - 1];
        let escaped = index >= 2 && chars[index - 2].1 == 'Ξ';
        if escaped {
            break;
        }
        if let Some(modifier) = Modifier::from_mnemonic(&character.to_string()) {
            modifiers.push(modifier);
            cut = byte_index;
            index -= 1;
        } else {
            break;
        }
    }

    modifiers.reverse();
    (&payload[..cut], modifiers)
}

fn parse_frame(payload: &str) -> SemanticFrame {
    let mut frame = SemanticFrame::default();

    for chunk in payload.split('•').filter(|chunk| !chunk.is_empty()) {
        if chunk.contains('→') {
            let parts = chunk
                .split('→')
                .filter(|part| !part.is_empty())
                .map(|part| part.to_string())
                .collect::<Vec<_>>();

            for window in parts.windows(2) {
                let from = window[0].clone();
                let to = window[1].clone();
                push_entity(&mut frame, &from);
                push_entity(&mut frame, &to);
                frame.relations.push(Relation {
                    from,
                    kind: RelationKind::LeadsTo,
                    to,
                });
            }
            continue;
        }

        if let Some(format) = OutputFormat::from_label(chunk) {
            let output_hint = frame.output_hint.get_or_insert(OutputHint {
                format: None,
                target: None,
            });
            output_hint.format = Some(format);
            continue;
        }

        push_entity(&mut frame, chunk);
    }

    frame
}

fn push_entity(frame: &mut SemanticFrame, canonical: &str) {
    if frame
        .entities
        .iter()
        .any(|entity| entity.canonical == canonical)
    {
        return;
    }

    frame.entities.push(Entity {
        surface: canonical.to_string(),
        canonical: canonical.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_process_item() {
        let item = parse_item_line("1>¡QENTα").unwrap();
        assert_eq!(item.sequence_id, Some(1));
        assert_eq!(item.instruction, Instruction::Explain);
        assert_eq!(item.modifiers, vec![Modifier::Simple]);
        assert_eq!(item.frame.entities[0].canonical, "QENT");
    }

    #[test]
    fn parses_program_roundtrip_shape() {
        let compact = "ΦEXPERT•AI•RESEARCHER\n§\n1>¡QENTα";
        let program = parse_program(compact).unwrap();
        assert_eq!(program.blocks.len(), 1);
        assert_eq!(
            program.blocks[0].items[0].flags.role.as_deref(),
            Some("EXPERT•AI•RESEARCHER")
        );
    }
}
