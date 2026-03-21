use crate::ir::{SourceSpan, TokelangProgram};
use crate::symbols::Modifier;

/// Coverage categories tracked across compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageKind {
    Topic,
    Example,
    Limitation,
    OutputRequirement,
}

/// Named coverage unit extracted from the source prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageItem {
    pub label: String,
    pub source_span: SourceSpan,
    pub kind: CoverageKind,
}

pub fn extract_coverage_items(input: &str) -> Vec<CoverageItem> {
    let patterns = [
        (
            "mathematical structure",
            "MATHEMATICAL STRUCTURE",
            CoverageKind::Topic,
        ),
        ("training process", "TRAINING PROCESS", CoverageKind::Topic),
        ("limitations", "LIMITATIONS", CoverageKind::Limitation),
        ("limitation", "LIMITATION", CoverageKind::Limitation),
        ("example", "EXAMPLE", CoverageKind::Example),
        ("conclusion", "CONCLUSION", CoverageKind::OutputRequirement),
        ("conclude", "CONCLUSION", CoverageKind::OutputRequirement),
    ];

    let lowered = input.to_ascii_lowercase();
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (needle, label, kind) in patterns {
        for (start, _) in lowered.match_indices(needle) {
            if seen.insert((label, start)) {
                items.push(CoverageItem {
                    label: label.to_string(),
                    source_span: SourceSpan {
                        start,
                        end: start + needle.len(),
                    },
                    kind: kind.clone(),
                });
            }
        }
    }

    items
}

pub fn reconcile_program(program: &mut TokelangProgram, coverage_items: &[CoverageItem]) {
    for coverage_item in coverage_items {
        if program
            .blocks
            .iter()
            .flat_map(|block| block.items.iter())
            .any(|item| item.covers_label(&coverage_item.label))
        {
            continue;
        }

        if let Some((block_index, item_index)) = nearest_item(program, coverage_item.source_span) {
            let item = &mut program.blocks[block_index].items[item_index];
            item.recovered_from_coverage = true;
            match coverage_item.kind {
                CoverageKind::Example => {
                    if !item.modifiers.contains(&Modifier::WithExamples) {
                        item.modifiers.push(Modifier::WithExamples);
                    }
                }
                CoverageKind::Limitation | CoverageKind::Topic => {
                    if !item.frame.residual_terms.contains(&coverage_item.label) {
                        item.frame.residual_terms.push(coverage_item.label.clone());
                    }
                }
                CoverageKind::OutputRequirement => {}
            }
        }
    }
}

fn nearest_item(program: &TokelangProgram, target: SourceSpan) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, usize)> = None;

    for (block_index, block) in program.blocks.iter().enumerate() {
        for (item_index, item) in block.items.iter().enumerate() {
            let Some(span) = item.source_span else {
                continue;
            };

            let item_mid = (span.start + span.end) / 2;
            let target_mid = (target.start + target.end) / 2;
            let distance = item_mid.abs_diff(target_mid);

            match best {
                Some((_, _, best_distance)) if best_distance <= distance => {}
                _ => best = Some((block_index, item_index, distance)),
            }
        }
    }

    best.map(|(block_index, item_index, _)| (block_index, item_index))
}
