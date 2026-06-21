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

    let mut blocks = Vec::new();
    let mut current_block = TokelangBlock::new(BlockType::Default);
    let mut prefix_flags = ContextFlags::default();

    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if parse_prefix_line(line, &mut prefix_flags) {
            continue;
        }

        if let Some(block_type) = BlockType::from_marker(line) {
            if !current_block.items.is_empty() {
                blocks.push(current_block);
            }
            current_block = TokelangBlock::new(block_type);
            continue;
        }

        if parse_block_default_line(line, &mut current_block) {
            continue;
        }

        let mut item = parse_item_line(line)?;
        if item.flags.role.is_none() {
            item.flags.role = prefix_flags.role.clone();
        }
        if item.flags.audience.is_none() {
            item.flags.audience = prefix_flags.audience.clone();
        }
        if item.modifiers.is_empty()
            && let Some(default_modifier) = current_block.default_modifier
        {
            item.modifiers.push(default_modifier);
        }
        current_block.items.push(item);
    }

    if !current_block.items.is_empty() {
        blocks.push(current_block);
    }

    Ok(TokelangProgram { blocks })
}

fn parse_block_default_line(line: &str, block: &mut TokelangBlock) -> bool {
    let Some(rest) = line.strip_prefix("default ") else {
        return false;
    };
    let candidate = rest.trim();
    if candidate.split_whitespace().count() != 1 {
        return false;
    }
    let Some(modifier) = Modifier::from_mnemonic(candidate) else {
        return false;
    };
    block.default_modifier = Some(modifier);
    true
}

fn parse_prefix_line(line: &str, flags: &mut ContextFlags) -> bool {
    if let Some(value) = line.strip_prefix("role ") {
        flags.role = Some(canonical_phrase(value));
        return true;
    }

    if let Some(value) = line.strip_prefix("audience ") {
        flags.audience = Some(canonical_phrase(value));
        return true;
    }

    false
}

fn parse_item_line(line: &str) -> Result<TokelangIR, ParseError> {
    let token_storage = tokenize_compact_line(line);
    let tokens = token_storage.iter().map(String::as_str).collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(ParseError::InvalidLine(line.to_string()));
    }

    let mut index = 0usize;
    let mut sequence_id = None;

    if tokens[0].chars().all(|ch| ch.is_ascii_digit()) {
        sequence_id = Some(
            tokens[0]
                .parse::<usize>()
                .map_err(|_| ParseError::InvalidSequence(line.to_string()))?,
        );
        index += 1;
    }

    let instruction_token = tokens
        .get(index)
        .ok_or_else(|| ParseError::MissingInstruction(line.to_string()))?;
    let (instruction, mut tail, mut compact_override) =
        if let Some(instruction) = Instruction::from_mnemonic(instruction_token) {
            index += 1;
            (instruction, tokens[index..].to_vec(), None)
        } else {
            parse_control_line(tokens.as_slice(), index, line)?
        };

    if compact_override.is_none() {
        compact_override = Some(line.trim().to_string());
    }

    let modifiers = split_modifiers(&mut tail);
    let frame = parse_frame_tokens(&tail);

    Ok(TokelangIR {
        sequence_id,
        instruction,
        frame,
        modifiers,
        flags: ContextFlags::default(),
        source_span: None,
        recovered_from_coverage: false,
        compact_override,
    })
}

fn parse_control_line<'a>(
    tokens: &'a [&'a str],
    start_index: usize,
    line: &str,
) -> Result<(Instruction, Vec<&'a str>, Option<String>), ParseError> {
    let control = tokens
        .get(start_index)
        .copied()
        .ok_or_else(|| ParseError::MissingInstruction(line.to_string()))?;

    let instruction = match control {
        "if" => Instruction::Analyze,
        "else" => match tokens.get(start_index + 1).copied() {
            Some("route") => Instruction::Transform,
            Some("return") | Some("write") => Instruction::Generate,
            Some(token) => Instruction::from_mnemonic(token).unwrap_or(Instruction::Analyze),
            None => return Err(ParseError::InvalidLine(line.to_string())),
        },
        "keep" => Instruction::Transform,
        "route" => Instruction::Transform,
        "return" | "write" => Instruction::Generate,
        other => return Err(ParseError::UnknownInstruction(other.to_string())),
    };

    Ok((
        instruction,
        tokens[start_index..].to_vec(),
        Some(line.trim().to_string()),
    ))
}

fn split_modifiers(tokens: &mut Vec<&str>) -> Vec<Modifier> {
    let mut modifiers = Vec::new();

    while let Some(last) = tokens.last().copied() {
        let Some(modifier) = Modifier::from_mnemonic(last) else {
            break;
        };
        modifiers.push(modifier);
        tokens.pop();
    }

    modifiers.reverse();
    modifiers
}

fn parse_frame_tokens(tokens: &[&str]) -> SemanticFrame {
    let mut frame = SemanticFrame::default();
    let mut index = 0usize;

    while index < tokens.len() {
        let token = tokens[index];

        if let Some(literal) = unwrap_backtick_literal(token) {
            frame.literal_islands.push(literal.to_string());
            index += 1;
            continue;
        }

        if token.eq_ignore_ascii_case("shape")
            && let Some(label) = tokens.get(index + 1)
            && let Some(format) = OutputFormat::from_label(label)
        {
            let output_hint = frame.output_hint.get_or_insert(OutputHint {
                format: None,
                target: None,
            });
            if output_hint.target.is_none()
                && let Some(target) = frame.residual_terms.pop()
            {
                output_hint.target = Some(target);
            }
            output_hint.format = Some(format);
            index += 2;
            continue;
        }

        if let Some(relation_kind) = relation_kind_for(token)
            && let (Some(from), Some(to)) = (
                frame.residual_terms.last().cloned(),
                tokens.get(index + 1).map(|value| value.to_string()),
            )
        {
            push_entity(&mut frame, &from);
            push_entity(&mut frame, &to);
            frame.relations.push(Relation {
                from,
                kind: relation_kind,
                to: to.clone(),
            });
            frame.residual_terms.push(to);
            index += 2;
            continue;
        }

        frame.residual_terms.push(token.to_string());
        index += 1;
    }

    frame
}

fn tokenize_compact_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_backticks = false;

    for ch in line.chars() {
        match ch {
            '`' => {
                if in_backticks {
                    current.push(ch);
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    in_backticks = false;
                } else {
                    if !current.trim().is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    current.push(ch);
                    in_backticks = true;
                }
            }
            c if c.is_whitespace() && !in_backticks => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn unwrap_backtick_literal(token: &str) -> Option<&str> {
    token
        .strip_prefix('`')
        .and_then(|inner| inner.strip_suffix('`'))
        .filter(|inner| !inner.trim().is_empty())
}

fn relation_kind_for(token: &str) -> Option<RelationKind> {
    match token {
        "leads" => Some(RelationKind::LeadsTo),
        "causes" => Some(RelationKind::Causes),
        "requires" => Some(RelationKind::Requires),
        "enables" => Some(RelationKind::Enables),
        "then" => Some(RelationKind::Sequence),
        _ => None,
    }
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

fn canonical_phrase(value: &str) -> String {
    value
        .split_whitespace()
        .map(|part| {
            part.chars()
                .map(|character| {
                    if character.is_ascii_alphabetic() {
                        character.to_ascii_uppercase()
                    } else {
                        character
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("•")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_process_item() {
        let item = parse_item_line("1 explain qent simple").unwrap();
        assert_eq!(item.sequence_id, Some(1));
        assert_eq!(item.instruction, Instruction::Explain);
        assert_eq!(item.modifiers, vec![Modifier::Simple]);
        assert_eq!(item.frame.residual_terms[0], "qent");
    }

    #[test]
    fn parses_program_roundtrip_shape() {
        let compact = "role expert ai researcher\nprocess\n1 explain qent simple";
        let program = parse_program(compact).unwrap();
        assert_eq!(program.blocks.len(), 1);
        assert_eq!(
            program.blocks[0].items[0].flags.role.as_deref(),
            Some("EXPERT•AI•RESEARCHER")
        );
    }
}
