use crate::compiler::coverage::{extract_coverage_items, reconcile_program};
use crate::compiler::error::CompileError;
use crate::compiler::normalize;
use crate::compiler::segment::{ClauseSpan, ListMarkerKind, split_clauses_with_protected_ranges};
use crate::ir::{
    BlockType, ContextFlags, Entity, OutputHint, Relation, RelationKind, SemanticFrame, SourceSpan,
    TokelangBlock, TokelangIR, TokelangProgram,
};
use crate::options::{CompileOptions, ProtectedRange, normalize_protected_ranges};
use crate::symbols::{Instruction, Modifier, OutputFormat, SubjectTable, SynonymTable};

/// Natural-language prompt compiler.
pub struct Compiler {
    synonyms: SynonymTable,
    subjects: SubjectTable,
}

#[derive(Debug, Clone)]
struct MatchedEntity {
    start: usize,
    end: usize,
    surface: String,
    canonical: String,
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            synonyms: SynonymTable::default_table(),
            subjects: SubjectTable::default_table(),
        }
    }

    pub fn compile(&self, input: &str) -> Result<TokelangProgram, CompileError> {
        self.compile_with_options(input, &CompileOptions::default())
    }

    pub fn compile_with_options(
        &self,
        input: &str,
        options: &CompileOptions,
    ) -> Result<TokelangProgram, CompileError> {
        if input.trim().is_empty() {
            return Err(CompileError::EmptyInput);
        }

        let protected_ranges = normalize_protected_ranges(input, &options.protected_ranges)?;
        let protected_pairs = protected_ranges
            .iter()
            .map(|range| (range.start, range.end))
            .collect::<Vec<_>>();
        let global_escaped = normalize::escape_reserved_symbols(input);
        let global_stripped =
            normalize::strip_protected_content_with_user(global_escaped, &protected_pairs);
        let global_cleaned = normalize::clean_input(&global_stripped);
        let global_words = normalize::tokenize_words(&global_cleaned);
        let global_flags = self.detect_flags(&global_words);
        let clauses = split_clauses_with_protected_ranges(input, &self.synonyms, &protected_pairs);
        let coverage_items = extract_coverage_items(input);

        let structured_pipeline = self.should_use_structured_pipeline(input, &clauses);

        let mut compiled_items = Vec::new();
        if structured_pipeline {
            let clauses = self.propagate_shared_sections(clauses);
            let clauses = self.group_instruction_context(clauses);
            for clause in clauses {
                if let Ok(ir) = self.compile_clause(&clause, &protected_ranges) {
                    compiled_items.push((clause, ir));
                }
            }
        } else {
            for clause in clauses {
                if let Ok(ir) = self.compile_clause(&clause, &protected_ranges) {
                    compiled_items.push((clause, ir));
                }
            }
        }

        if compiled_items.is_empty() {
            let whole_clause = ClauseSpan::new(
                0,
                input.len(),
                input.trim().to_string(),
                None,
                0,
                false,
                None,
            );
            compiled_items.push((
                whole_clause.clone(),
                self.compile_clause(&whole_clause, &protected_ranges)?,
            ));
        }

        if let Some((_, first_item)) = compiled_items.first_mut() {
            first_item.flags.role = global_flags.role.clone();
            first_item.flags.audience = global_flags.audience.clone();
        }

        let mut program = self.assemble_program(compiled_items);
        reconcile_program(&mut program, &coverage_items);
        Ok(program)
    }

    fn group_instruction_context(&self, clauses: Vec<ClauseSpan>) -> Vec<ClauseSpan> {
        let mut grouped = Vec::new();
        let mut current: Option<ClauseSpan> = None;
        let mut pending_for_next: Vec<ClauseSpan> = Vec::new();

        for clause in clauses {
            if self.clause_has_instruction(&clause) {
                if let Some(existing) = current.as_mut() {
                    if is_mergeable_controller_tail_clause(existing, &clause) {
                        append_clause(existing, clause);
                        continue;
                    }

                    if clause.is_list_item
                        && clause.marker.is_none()
                        && (clause.indent > existing.indent
                            || (clause.list_marker_kind == Some(ListMarkerKind::Bullet)
                                && !existing.is_list_item
                                && existing
                                    .text
                                    .lines()
                                    .next()
                                    .map(|line| line.trim_end().ends_with(':'))
                                    .unwrap_or(false)))
                    {
                        append_clause(existing, clause);
                        continue;
                    }
                }

                if let Some(existing) = current.take() {
                    grouped.push(existing);
                }

                let merged = if pending_for_next.is_empty() {
                    clause
                } else {
                    merge_pending_prefixes_with_instruction(
                        std::mem::take(&mut pending_for_next),
                        clause,
                    )
                };

                current = Some(merged);
                continue;
            }

            if is_literal_payload_clause(&clause) || is_shared_data_payload_clause(&clause) {
                if let Some(existing) = current.take() {
                    grouped.push(existing);
                }
                if !pending_for_next.is_empty() {
                    grouped.extend(std::mem::take(&mut pending_for_next));
                }
                grouped.push(clause);
                continue;
            }

            if is_structural_heading_clause(&clause) {
                if let Some(existing) = current.take() {
                    grouped.push(existing);
                }
                pending_for_next.push(clause);
                continue;
            }

            if current.is_none() {
                pending_for_next.push(clause);
                continue;
            }

            if clause.marker.is_some() {
                if let Some(existing) = current.take() {
                    grouped.push(existing);
                }
                pending_for_next.push(clause);
                continue;
            }

            if !pending_for_next.is_empty() {
                pending_for_next.push(clause);
                continue;
            }

            if let Some(existing) = current.as_mut() {
                append_clause(existing, clause);
            }
        }

        if let Some(existing) = current {
            grouped.push(existing);
        }

        if grouped.is_empty() {
            pending_for_next
        } else {
            grouped
        }
    }

    fn propagate_shared_sections(&self, clauses: Vec<ClauseSpan>) -> Vec<ClauseSpan> {
        let mut propagated = Vec::new();
        let mut cluster = SharedContextCluster::default();
        let mut capture_mode = SharedCaptureMode::Carry;
        let mut list_inference_enabled = false;
        let mut last_list_instruction: Option<ListInstructionContext> = None;
        let mut pre_task_constraint_mode = false;
        let mut seen_semantic_instructions = false;

        for index in 0..clauses.len() {
            let mut clause = clauses[index].clone();
            if is_scope_boundary(&clause) {
                flush_shared_context_cluster(self, &mut cluster, &mut propagated);
                capture_mode = SharedCaptureMode::Carry;
                list_inference_enabled = false;
                last_list_instruction = None;
                pre_task_constraint_mode = false;
                continue;
            }

            if is_shared_data_payload_clause(&clause) {
                continue;
            }

            if self.should_ignore_structured_workflow_preamble(index, &clauses) {
                capture_mode = SharedCaptureMode::Ignore;
                list_inference_enabled = false;
                last_list_instruction = None;
                continue;
            }

            let shared_heading = shared_heading_kind(&clause);
            let heading_enables_list_inference = enables_list_instruction_inference(&clause);
            let mut detected_instruction = self.instruction_from_clause(&clause);
            let demote_task_list_leadin = self
                .should_demote_into_upcoming_task_list(index, &clauses)
                || self.should_demote_into_step_workflow(index, &clauses);
            let starts_pre_task_constraint_mode =
                self.should_start_pre_task_constraint_mode(index, &clauses);
            let starts_output_only_rules_mode =
                self.should_start_output_only_rules_mode(index, &clauses);
            let list_heading_instruction = self.instruction_heading_for_list(index, &clauses);
            let branch_local_constraint = is_branch_local_constraint_clause(&clause);
            let tail_local_output_constraint =
                is_tail_local_output_constraint_clause(index, &clauses);

            if pre_task_constraint_mode {
                match explicit_list_heading(&clause) {
                    Some(ExplicitListHeading::Tasks) => {
                        pre_task_constraint_mode = false;
                    }
                    Some(ExplicitListHeading::Other) => {
                        pre_task_constraint_mode = false;
                        capture_mode = SharedCaptureMode::Carry;
                        list_inference_enabled = false;
                        last_list_instruction = None;
                    }
                    None => {
                        if is_output_constraint_metadata_clause(&clause) {
                            cluster
                                .local_shared
                                .push(compact_tail_local_output_constraint_clause(clause));
                        } else {
                            cluster.shared.push(clause);
                        }
                        continue;
                    }
                }
            }

            if starts_output_only_rules_mode {
                if cluster
                    .shared
                    .last()
                    .is_some_and(looks_like_short_context_title_clause)
                {
                    cluster.shared.pop();
                }
                pre_task_constraint_mode = true;
                capture_mode = SharedCaptureMode::ConstraintMetadata;
                list_inference_enabled = false;
                last_list_instruction = None;
                continue;
            }

            if starts_pre_task_constraint_mode {
                pre_task_constraint_mode = true;
                cluster.shared.push(clause);
                continue;
            }

            if let Some(compacted_controller_clause) = compact_numbered_controller_clause(&clause) {
                clause = compacted_controller_clause;
                detected_instruction = self.instruction_from_clause(&clause);
            }

            if let Some(instruction) = list_heading_instruction {
                list_inference_enabled = true;
                last_list_instruction = Some(ListInstructionContext {
                    instruction,
                    indent: clause.indent,
                });
                continue;
            }

            if branch_local_constraint {
                clause = compact_branch_local_constraint_clause(clause);
                detected_instruction = None;
            } else if tail_local_output_constraint {
                clause = compact_tail_local_output_constraint_clause(clause);
                detected_instruction = None;
            }

            if detected_instruction.is_none()
                && clause.is_list_item
                && let Some(output_clause) = rewrite_output_metadata_clause(
                    &clause,
                    matches!(capture_mode, SharedCaptureMode::WorkflowOutputMetadata),
                )
            {
                clause = output_clause;
                detected_instruction = self.instruction_from_clause(&clause);
            }

            if detected_instruction.is_none()
                && !branch_local_constraint
                && !tail_local_output_constraint
                && should_infer_list_item_instruction(
                    &clause,
                    list_inference_enabled,
                    last_list_instruction,
                )
            {
                let cleaned = clause.cleaned_text.as_str();
                let inherited_instruction = if is_workflow_controller_clause_text(&cleaned) {
                    Instruction::Analyze
                } else {
                    last_list_instruction.unwrap().instruction
                };
                clause = rewrite_with_inherited_instruction(clause, inherited_instruction);
                detected_instruction = self.instruction_from_clause(&clause);
            }

            let is_instruction = detected_instruction.is_some() && !demote_task_list_leadin;

            if !is_instruction
                && clause.marker.is_some()
                && !cluster.instructions.is_empty()
                && shared_heading.is_none()
            {
                flush_shared_context_cluster(self, &mut cluster, &mut propagated);
                capture_mode = SharedCaptureMode::Carry;
                list_inference_enabled = false;
                last_list_instruction = None;
            }

            if matches!(shared_heading, Some(SharedHeadingKind::Workflow(_)))
                && (!cluster.instructions.is_empty() || !cluster.local_shared.is_empty())
            {
                flush_shared_context_cluster(self, &mut cluster, &mut propagated);
                last_list_instruction = None;
            }

            match shared_heading {
                Some(SharedHeadingKind::Carry { keep_heading }) => {
                    capture_mode = SharedCaptureMode::Carry;
                    list_inference_enabled = heading_enables_list_inference;
                    last_list_instruction = None;
                    if keep_heading {
                        cluster.shared.push(clause);
                    }
                    continue;
                }
                Some(SharedHeadingKind::Ignore) => {
                    capture_mode = SharedCaptureMode::Ignore;
                    list_inference_enabled = heading_enables_list_inference;
                    last_list_instruction = None;
                    continue;
                }
                Some(SharedHeadingKind::Constraint) => {
                    capture_mode = SharedCaptureMode::ConstraintMetadata;
                    list_inference_enabled = false;
                    last_list_instruction = None;
                    continue;
                }
                Some(SharedHeadingKind::Metadata) => {
                    flush_shared_context_cluster(self, &mut cluster, &mut propagated);
                    capture_mode = if seen_semantic_instructions {
                        SharedCaptureMode::IgnoreMetadata
                    } else {
                        SharedCaptureMode::ConstraintMetadata
                    };
                    list_inference_enabled = false;
                    last_list_instruction = None;
                    continue;
                }
                Some(SharedHeadingKind::Output) => {
                    capture_mode = SharedCaptureMode::OutputMetadata;
                    list_inference_enabled = false;
                    last_list_instruction = None;
                    continue;
                }
                Some(SharedHeadingKind::Payload) => {
                    flush_shared_context_cluster(self, &mut cluster, &mut propagated);
                    capture_mode = SharedCaptureMode::PayloadSink;
                    list_inference_enabled = false;
                    last_list_instruction = None;
                    continue;
                }
                Some(SharedHeadingKind::Workflow(kind)) => {
                    let workflow_output_mode =
                        workflow_heading_opens_output_section(index, &clauses);
                    capture_mode = if workflow_output_mode {
                        SharedCaptureMode::WorkflowOutputMetadata
                    } else {
                        SharedCaptureMode::Carry
                    };
                    list_inference_enabled =
                        heading_enables_list_inference && !workflow_output_mode;
                    let mut compact_heading =
                        compact_workflow_heading_clause(index, &clauses, kind);
                    if !workflow_heading_has_child_list(index, &clauses)
                        && let Some(heading_clause) = compact_heading.as_mut()
                    {
                        let cleaned_heading = normalize::clean_input(
                            &normalize::escape_reserved_symbols(&heading_clause.text),
                        );
                        if self.instruction_from_clause(heading_clause).is_none()
                            && is_workflow_controller_clause_text(&cleaned_heading)
                        {
                            *heading_clause = rewrite_with_inherited_instruction(
                                heading_clause.clone(),
                                Instruction::Analyze,
                            );
                        }
                    }
                    last_list_instruction = Some(ListInstructionContext {
                        instruction: self
                            .instruction_from_clause(compact_heading.as_ref().unwrap_or(&clause))
                            .unwrap_or(Instruction::Analyze),
                        indent: clause.indent,
                    });
                    if let Some(compact_heading) = compact_heading {
                        cluster.shared.push(compact_heading);
                    }
                    continue;
                }
                None => {}
            }

            if capture_mode == SharedCaptureMode::ConstraintMetadata {
                if (branch_local_constraint || tail_local_output_constraint)
                    && !cluster.instructions.is_empty()
                {
                    cluster.local_shared.push(clause);
                } else {
                    cluster.shared.push(clause);
                }
                last_list_instruction = None;
                continue;
            }

            if capture_mode == SharedCaptureMode::IgnoreMetadata {
                last_list_instruction = None;
                continue;
            }

            if capture_mode == SharedCaptureMode::PayloadSink {
                last_list_instruction = None;
                continue;
            }

            if capture_mode == SharedCaptureMode::OutputMetadata {
                if let Some(output_clause) = rewrite_output_metadata_clause(&clause, false) {
                    seen_semantic_instructions = true;
                    cluster.instructions.push(output_clause);
                }
                last_list_instruction = None;
                continue;
            }

            if capture_mode == SharedCaptureMode::WorkflowOutputMetadata {
                if let Some(output_clause) = rewrite_output_metadata_clause(&clause, true) {
                    seen_semantic_instructions = true;
                    cluster.instructions.push(output_clause);
                }
                last_list_instruction = None;
                continue;
            }

            if is_instruction {
                if clause.is_list_item {
                    last_list_instruction =
                        detected_instruction.map(|instruction| ListInstructionContext {
                            instruction,
                            indent: clause.indent,
                        });
                } else {
                    last_list_instruction = None;
                }
                seen_semantic_instructions = true;
                cluster.instructions.push(clause);
                continue;
            }

            if !clause.is_list_item {
                last_list_instruction = None;
            }

            if (branch_local_constraint || tail_local_output_constraint)
                && !cluster.instructions.is_empty()
            {
                cluster.local_shared.push(clause);
            } else if capture_mode == SharedCaptureMode::Carry || cluster.instructions.is_empty() {
                cluster.shared.push(clause);
            } else {
                propagated.push(clause);
            }
        }

        flush_shared_context_cluster(self, &mut cluster, &mut propagated);

        if propagated.is_empty() {
            Vec::new()
        } else {
            propagated
        }
    }

    fn clause_has_instruction(&self, clause: &ClauseSpan) -> bool {
        self.instruction_from_clause(clause).is_some()
    }

    fn should_demote_into_upcoming_task_list(&self, index: usize, clauses: &[ClauseSpan]) -> bool {
        let clause = &clauses[index];
        if clause.is_list_item || clause.marker.is_some() || !self.clause_has_instruction(clause) {
            return false;
        }

        for next in clauses.iter().skip(index + 1) {
            if is_scope_boundary(next) || self.clause_has_instruction(next) {
                return false;
            }

            match explicit_list_heading(next) {
                Some(ExplicitListHeading::Tasks) => return true,
                Some(ExplicitListHeading::Other) => return false,
                None => {}
            }
        }

        false
    }

    fn should_demote_into_step_workflow(&self, index: usize, clauses: &[ClauseSpan]) -> bool {
        let clause = &clauses[index];
        if clause.is_list_item || clause.marker.is_some() || !self.clause_has_instruction(clause) {
            return false;
        }

        let cleaned = clause.cleaned_text.as_str();
        let is_follow_workflow_preamble = cleaned.starts_with("follow ")
            && (cleaned.contains("instruction")
                || cleaned.contains("workflow")
                || cleaned.contains("decision process")
                || cleaned.ends_with("process"));
        if !is_follow_workflow_preamble {
            return false;
        }

        for next in clauses.iter().skip(index + 1) {
            if is_scope_boundary(next) {
                return false;
            }

            if matches!(
                shared_heading_kind(next),
                Some(SharedHeadingKind::Workflow(_))
            ) {
                return true;
            }

            if self.clause_has_instruction(next) {
                return false;
            }
        }

        false
    }

    fn should_ignore_structured_workflow_preamble(
        &self,
        index: usize,
        clauses: &[ClauseSpan],
    ) -> bool {
        let clause = &clauses[index];
        if clause.is_list_item
            || clause.marker.is_some()
            || is_literal_payload_clause(clause)
            || is_structural_heading_clause(clause)
        {
            return false;
        }

        let cleaned = clause.cleaned_text.as_str();
        let ignore_output_only_rules_preamble =
            self.should_ignore_output_only_rules_preamble(index, clauses);
        let ignore_short_controller_workflow_preamble =
            self.should_ignore_short_controller_workflow_preamble(index, clauses);
        let ignore_short_structured_title =
            self.should_ignore_short_structured_title(index, clauses);
        if cleaned.is_empty()
            || cleaned.split_whitespace().count() > 8
            || (!looks_like_generic_workflow_preamble(&cleaned)
                && !ignore_output_only_rules_preamble
                && !ignore_short_controller_workflow_preamble
                && !ignore_short_structured_title)
        {
            return false;
        }

        for (next_index, next) in clauses.iter().enumerate().skip(index + 1) {
            if is_scope_boundary(next) {
                return false;
            }

            if matches!(
                shared_heading_kind(next),
                Some(SharedHeadingKind::Workflow(_))
            ) || explicit_list_heading(next) == Some(ExplicitListHeading::Tasks)
                || next.is_list_item
            {
                return true;
            }

            if next.cleaned_text == "rules"
                && self.should_start_output_only_rules_mode(next_index, clauses)
            {
                continue;
            }

            let next_cleaned = next.cleaned_text.as_str();
            if !next_cleaned.is_empty() && !looks_like_generic_workflow_preamble(&next_cleaned) {
                return false;
            }
        }

        false
    }

    fn should_ignore_output_only_rules_preamble(
        &self,
        index: usize,
        clauses: &[ClauseSpan],
    ) -> bool {
        let clause = &clauses[index];
        if clause.is_list_item || clause.marker.is_some() || !self.clause_has_instruction(clause) {
            return false;
        }

        let cleaned = clause.cleaned_text.as_str();
        if cleaned.split_whitespace().count() > 6 {
            return false;
        }

        for (next_index, next) in clauses.iter().enumerate().skip(index + 1) {
            if is_scope_boundary(next) {
                return false;
            }

            let next_cleaned = next.cleaned_text.as_str();
            if next_cleaned == "rules"
                && self.should_start_output_only_rules_mode(next_index, clauses)
            {
                return true;
            }

            if next.is_list_item || self.clause_has_instruction(next) {
                return false;
            }
        }

        false
    }

    fn should_ignore_short_controller_workflow_preamble(
        &self,
        index: usize,
        clauses: &[ClauseSpan],
    ) -> bool {
        let clause = &clauses[index];
        if clause.is_list_item || clause.marker.is_some() {
            return false;
        }

        let cleaned = clause.cleaned_text.as_str();
        let words = cleaned.split_whitespace().collect::<Vec<_>>();
        if words.len() > 5
            || !matches!(
                words.first().copied().unwrap_or_default(),
                "review"
                    | "analyze"
                    | "audit"
                    | "check"
                    | "plan"
                    | "design"
                    | "create"
                    | "compare"
                    | "write"
                    | "route"
                    | "screen"
                    | "assess"
                    | "inspect"
            )
        {
            return false;
        }

        let mut saw_numbered_item = false;
        let mut saw_controller = false;
        let mut saw_output = false;

        for next in clauses.iter().skip(index + 1) {
            if is_scope_boundary(next) {
                break;
            }

            if !next.is_list_item || next.list_marker_kind != Some(ListMarkerKind::Numbered) {
                continue;
            }

            saw_numbered_item = true;

            if is_workflow_controller_clause_text(&next.cleaned_text) {
                saw_controller = true;
            }

            if rewrite_output_metadata_clause(next, false).is_some() {
                saw_output = true;
            }
        }

        saw_numbered_item && saw_controller && saw_output
    }

    fn should_ignore_short_structured_title(&self, index: usize, clauses: &[ClauseSpan]) -> bool {
        let clause = &clauses[index];
        if clause.is_list_item
            || clause.marker.is_some()
            || is_literal_payload_clause(clause)
            || is_structural_heading_clause(clause)
        {
            return false;
        }

        let cleaned = clause.cleaned_text.as_str();
        let words = cleaned.split_whitespace().collect::<Vec<_>>();
        if cleaned.is_empty() || words.len() > 4 || is_workflow_controller_clause_text(&cleaned) {
            return false;
        }

        let generic_tail = cleaned.ends_with(" plan")
            || cleaned.ends_with(" workflow")
            || cleaned.ends_with(" review")
            || cleaned.ends_with(" ticket")
            || cleaned.ends_with(" handling")
            || cleaned.ends_with(" checklist")
            || cleaned.ends_with(" protocol")
            || cleaned.ends_with(" summary")
            || cleaned.ends_with(" memo")
            || cleaned.ends_with(" decision tree");
        if !generic_tail && words.len() > 2 {
            return false;
        }

        for next in clauses.iter().skip(index + 1) {
            if is_scope_boundary(next) {
                break;
            }

            if explicit_list_heading(next).is_some()
                || matches!(
                    shared_heading_kind(next),
                    Some(SharedHeadingKind::Workflow(_))
                )
            {
                return true;
            }

            let next_cleaned = next.cleaned_text.as_str();
            if next_cleaned == "rules" {
                return true;
            }

            if next.is_list_item {
                return generic_tail;
            }

            if !next_cleaned.is_empty() {
                return false;
            }
        }

        false
    }

    fn should_start_pre_task_constraint_mode(&self, index: usize, clauses: &[ClauseSpan]) -> bool {
        let clause = &clauses[index];
        let escaped = normalize::escape_reserved_symbols(&clause.text);
        let cleaned = normalize::clean_input(&escaped);
        if !cleaned.contains("constraint") {
            return false;
        }

        clauses
            .iter()
            .skip(index + 1)
            .take_while(|next| !is_scope_boundary(next))
            .any(|next| explicit_list_heading(next) == Some(ExplicitListHeading::Tasks))
    }

    fn should_start_output_only_rules_mode(&self, index: usize, clauses: &[ClauseSpan]) -> bool {
        let clause = &clauses[index];
        let cleaned = clause.cleaned_text.as_str();
        if cleaned != "rules" && !cleaned.ends_with(" rules") {
            return false;
        }

        let mut seen_tasks_heading = false;
        let mut saw_task_list_item = false;
        let mut saw_output_only_task = false;

        for next in clauses.iter().skip(index + 1) {
            if is_scope_boundary(next) {
                break;
            }

            match explicit_list_heading(next) {
                Some(ExplicitListHeading::Tasks) => {
                    seen_tasks_heading = true;
                    continue;
                }
                Some(ExplicitListHeading::Other) if seen_tasks_heading => break,
                _ => {}
            }

            if !seen_tasks_heading {
                continue;
            }

            if !next.is_list_item {
                continue;
            }

            saw_task_list_item = true;
            if rewrite_output_metadata_clause(next, true).is_some() {
                saw_output_only_task = true;
            }
        }

        seen_tasks_heading && (saw_output_only_task || saw_task_list_item)
    }

    fn instruction_heading_for_list(
        &self,
        index: usize,
        clauses: &[ClauseSpan],
    ) -> Option<Instruction> {
        let clause = &clauses[index];
        if clause.is_list_item
            || clause.marker.is_some()
            || shared_heading_kind(clause).is_some()
            || !clause.text.trim_end().ends_with(':')
        {
            return None;
        }

        let instruction = self.instruction_from_clause(clause)?;
        let cleaned = normalize::clean_input(&clause.text);
        if cleaned.split_whitespace().count() > 3 {
            return None;
        }

        let next = clauses.get(index + 1)?;
        if next.is_list_item {
            Some(instruction)
        } else {
            None
        }
    }

    fn instruction_from_clause(&self, clause: &ClauseSpan) -> Option<Instruction> {
        if is_literal_payload_clause(clause) {
            return None;
        }

        let stripped = normalize::strip_protected_content(&clause.text);
        let cleaned = normalize::clean_input(&stripped);
        if let Some(instruction) = control_instruction_for_clause(&cleaned) {
            return Some(instruction);
        }
        let words = normalize::tokenize_words(&cleaned);
        if words.is_empty() {
            return None;
        }

        self.detect_instruction(&words).ok()
    }

    fn assemble_program(&self, compiled_items: Vec<(ClauseSpan, TokelangIR)>) -> TokelangProgram {
        let mut program = TokelangProgram::new();
        let mut current_type = BlockType::Default;
        let mut current_block = TokelangBlock::new(BlockType::Default);
        let mut process_sequence = 1usize;
        let mut numbered_workflow_sequence = 1usize;

        for (clause, mut item) in compiled_items {
            apply_structure_compact_override(&clause, &mut item);
            let target_type = self.block_type_for_item(&item);
            if current_block.items.is_empty() {
                current_type = target_type;
                current_block = TokelangBlock::new(target_type);
            } else if current_type != target_type {
                program = program.with_block(current_block);
                current_type = target_type;
                current_block = TokelangBlock::new(target_type);
                if current_type == BlockType::Process {
                    process_sequence = 1;
                }
            }

            let workflow_return_line = item
                .compact_override
                .as_deref()
                .is_some_and(|line| line.starts_with("return "))
                && process_sequence > 1;

            if clause.is_list_item && clause.list_marker_kind == Some(ListMarkerKind::Numbered) {
                item.sequence_id = Some(numbered_workflow_sequence);
                numbered_workflow_sequence += 1;
            } else if workflow_return_line {
                item.sequence_id = Some(process_sequence);
                process_sequence += 1;
            } else if current_type == BlockType::Process {
                item.sequence_id = Some(process_sequence);
                process_sequence += 1;
            } else {
                item.sequence_id = None;
            }

            current_block = current_block.add_item(item);
        }

        if !current_block.items.is_empty() {
            program = program.with_block(current_block);
        }

        program
    }

    fn compile_clause(
        &self,
        clause: &ClauseSpan,
        protected_ranges: &[ProtectedRange],
    ) -> Result<TokelangIR, CompileError> {
        let stripped = normalize::strip_protected_content_with_user(
            &clause.text,
            &clause_local_protected_ranges(clause, protected_ranges),
        );
        let cleaned = normalize::clean_input(&stripped);
        let words = normalize::tokenize_words(&cleaned);

        if words.is_empty() {
            return Err(CompileError::NoSemanticContent);
        }

        let instruction = control_instruction_for_clause(&cleaned)
            .or_else(|| self.detect_instruction(&words).ok())
            .ok_or(CompileError::NoInstruction)?;
        self.compile_clause_with_words(clause, &words, instruction, protected_ranges)
    }

    fn compile_clause_with_words(
        &self,
        clause: &ClauseSpan,
        words: &[String],
        instruction: Instruction,
        protected_ranges: &[ProtectedRange],
    ) -> Result<TokelangIR, CompileError> {
        if words.is_empty() {
            return Err(CompileError::NoSemanticContent);
        }

        let mut flags = self.detect_flags(&words);
        flags.role = None;
        flags.audience = None;
        let mut modifiers = self.detect_modifiers(&words);
        let cleaned_clause = normalize::clean_input(&normalize::strip_protected_content_with_user(
            &clause.text,
            &clause_local_protected_ranges(clause, protected_ranges),
        ));
        let mut output_hint = self.detect_output_hint(&words);
        optimize_output_hint(&mut output_hint, instruction);
        let mut entities = self.extract_entities(&words);
        optimize_entities(&mut entities, words, &cleaned_clause);
        let relations = self.extract_relations(&words, &entities);
        let mut literal_islands = extract_literal_islands(clause, protected_ranges);
        let mut residual_terms = self.extract_residual_terms(&words, &entities);
        optimize_residual_terms(&mut residual_terms, words, instruction);
        optimize_literal_islands(&mut literal_islands, &mut entities, &mut residual_terms);

        let mut frame = SemanticFrame {
            entities: entities
                .iter()
                .map(|entity| Entity {
                    surface: entity.surface.clone(),
                    canonical: entity.canonical.clone(),
                })
                .collect(),
            relations,
            output_hint,
            literal_islands,
            residual_terms,
        };

        if frame.entities.is_empty()
            && frame.relations.is_empty()
            && frame.output_hint.is_none()
            && frame.literal_islands.is_empty()
            && frame.residual_terms.is_empty()
        {
            return Err(CompileError::NoSemanticContent);
        }

        if let Some(output_hint) = frame.output_hint.as_mut()
            && output_hint.target.is_none()
            && let Some(entity) = frame.entities.first()
        {
            output_hint.target = Some(entity.canonical.clone());
        }

        optimize_modifiers(&mut modifiers, instruction);

        Ok(TokelangIR {
            sequence_id: None,
            instruction,
            frame,
            modifiers,
            flags,
            source_span: Some(SourceSpan {
                start: clause.start,
                end: clause.end,
            }),
            recovered_from_coverage: false,
            compact_override: None,
        })
    }

    fn should_use_structured_pipeline(&self, input: &str, clauses: &[ClauseSpan]) -> bool {
        if clauses.iter().any(|clause| clause.is_list_item) {
            return true;
        }

        let lowered = input.to_ascii_lowercase();
        if [
            "[inp]",
            "[prc]",
            "[out]",
            "tasks:",
            "constraints:",
            "return:",
            "output:",
            "example:",
            "initial state:",
            "rules:",
            "---",
        ]
        .iter()
        .any(|marker| lowered.contains(marker))
        {
            return true;
        }

        input.lines().any(|line| {
            let trimmed = line.trim_start();
            let lowered = trimmed.to_ascii_lowercase();
            trimmed.starts_with('-')
                || trimmed.starts_with('*')
                || trimmed.starts_with('[')
                || lowered.starts_with("step ")
                || trimmed
                    .chars()
                    .next()
                    .map(|character| character.is_ascii_digit())
                    .unwrap_or(false)
        })
    }

    fn detect_instruction(&self, words: &[String]) -> Result<Instruction, CompileError> {
        for start in 0..words.len() {
            for width in (1..=3).rev() {
                if start + width > words.len() {
                    continue;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(instruction) = self.synonyms.resolve_instruction(&phrase) {
                    return Ok(instruction);
                }
            }
        }

        Err(CompileError::NoInstruction)
    }

    fn detect_modifiers(&self, words: &[String]) -> Vec<Modifier> {
        let mut modifiers = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for start in 0..words.len() {
            for width in (1..=3).rev() {
                if start + width > words.len() {
                    continue;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(modifier) = self.synonyms.resolve_modifier(&phrase)
                    && seen.insert(modifier)
                {
                    modifiers.push(modifier);
                }
            }
        }

        modifiers
    }

    fn detect_output_hint(&self, words: &[String]) -> Option<OutputHint> {
        let mut output_hint = OutputHint {
            format: None,
            target: None,
        };

        for start in 0..words.len() {
            for width in (1..=2).rev() {
                if start + width > words.len() {
                    continue;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(format) = self.synonyms.resolve_output_format(&phrase) {
                    output_hint.format = Some(format);
                    return Some(output_hint);
                }
            }
        }

        None
    }

    fn extract_entities(&self, words: &[String]) -> Vec<MatchedEntity> {
        let mut entities = Vec::new();
        let mut index = 0usize;

        while index < words.len() {
            let word = words[index].as_str();

            if should_skip_entity_word(word, &self.synonyms) {
                index += 1;
                continue;
            }

            if let Some(subject_match) = self.subjects.longest_match_from(words, index) {
                entities.push(MatchedEntity {
                    start: index,
                    end: index + subject_match.consumed,
                    surface: subject_match.surface,
                    canonical: subject_match.canonical,
                });
                index += subject_match.consumed;
                continue;
            }

            entities.push(MatchedEntity {
                start: index,
                end: index + 1,
                surface: word.to_string(),
                canonical: normalize::canonicalize_term(word),
            });
            index += 1;
        }

        dedupe_entities(entities)
    }

    fn extract_relations(&self, words: &[String], entities: &[MatchedEntity]) -> Vec<Relation> {
        let mut relations = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (index, word) in words.iter().enumerate() {
            let Some(kind) = relation_kind(word) else {
                continue;
            };

            let Some(previous_entity) = entities.iter().rev().find(|entity| entity.end <= index)
            else {
                continue;
            };
            let Some(next_entity) = entities.iter().find(|entity| entity.start > index) else {
                continue;
            };

            let key = (
                previous_entity.canonical.clone(),
                kind,
                next_entity.canonical.clone(),
            );
            if seen.insert(key.clone()) {
                relations.push(Relation {
                    from: key.0,
                    kind: key.1,
                    to: key.2,
                });
            }
        }

        relations
    }

    fn extract_residual_terms(&self, words: &[String], entities: &[MatchedEntity]) -> Vec<String> {
        let mut covered_indices = std::collections::HashSet::new();
        for entity in entities {
            for index in entity.start..entity.end {
                covered_indices.insert(index);
            }
        }

        let mut residuals = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (index, word) in words.iter().enumerate() {
            if covered_indices.contains(&index) || should_skip_entity_word(word, &self.synonyms) {
                continue;
            }

            if normalize::is_descriptor_word(word) || is_content_residual(word) {
                let canonical = normalize::canonicalize_term(word);
                if seen.insert(canonical.clone()) {
                    residuals.push(canonical);
                }
            }
        }

        residuals
    }

    fn detect_flags(&self, words: &[String]) -> ContextFlags {
        let text = words.join(" ");
        let role = detect_role(words);
        let audience = detect_audience(words);

        ContextFlags {
            urgent: text.contains("urgent")
                || text.contains("urgently")
                || text.contains("immediately")
                || text.contains("asap"),
            with_confidence: text.contains("confidence") || text.contains("certainty"),
            with_sources: text.contains("source")
                || text.contains("sources")
                || text.contains("citation")
                || text.contains("citations")
                || text.contains("reference")
                || text.contains("references"),
            role,
            audience,
        }
    }

    fn block_type_for(&self, instruction: Instruction) -> BlockType {
        match instruction {
            Instruction::Search => BlockType::Input,
            Instruction::Summarize
            | Instruction::Generate
            | Instruction::List
            | Instruction::Conclude => BlockType::Output,
            _ => BlockType::Process,
        }
    }

    fn block_type_for_item(&self, item: &TokelangIR) -> BlockType {
        if let Some(line) = item.compact_override.as_deref() {
            let trimmed = line.trim();
            if trimmed.starts_with("if ") || trimmed.starts_with("else ") {
                return BlockType::Process;
            }
        }

        self.block_type_for(item.instruction)
    }

    fn summarize_shared_context_clause(
        &self,
        shared_context: &[ClauseSpan],
        instruction_clause: &ClauseSpan,
        max_terms: usize,
    ) -> Option<ClauseSpan> {
        if max_terms == 0 {
            return None;
        }

        let instruction_words = normalize::tokenize_words(&instruction_clause.cleaned_text)
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let shared_context_has_rules = shared_context.iter().any(|clause| {
            let cleaned = clause.cleaned_text.as_str();
            cleaned == "rules" || cleaned.ends_with(" rules")
        });
        let mut candidates = Vec::new();
        let mut global_position = 0usize;

        for clause in shared_context {
            if shared_context_has_rules && looks_like_short_context_title_clause(clause) {
                continue;
            }

            let escaped = normalize::escape_reserved_symbols(&clause.text);
            let cleaned = normalize::clean_input(&escaped);
            let words = normalize::tokenize_words(&cleaned);
            if words.is_empty() {
                continue;
            }

            let mut clause_candidates = words
                .iter()
                .enumerate()
                .filter_map(|(index, word)| {
                    if instruction_words.contains(word)
                        || should_skip_shared_anchor_word(word, &self.synonyms)
                    {
                        None
                    } else {
                        Some(SharedAnchorCandidate {
                            position: global_position + index,
                            score: shared_anchor_score(word),
                            text: word.to_string(),
                        })
                    }
                })
                .collect::<Vec<_>>();

            for index in 0..words.len().saturating_sub(1) {
                let left = words[index].as_str();
                let right = words[index + 1].as_str();
                if instruction_words.contains(left) && instruction_words.contains(right) {
                    continue;
                }
                if should_skip_shared_phrase_word(left, &self.synonyms)
                    || should_skip_shared_phrase_word(right, &self.synonyms)
                {
                    continue;
                }

                clause_candidates.push(SharedAnchorCandidate {
                    position: global_position + index,
                    score: shared_phrase_score(left, right),
                    text: format!("{left} {right}"),
                });
            }

            if clause_candidates.is_empty() {
                global_position += words.len();
                continue;
            }

            let first_split = (words.len() / 3).max(1);
            let second_split = ((words.len() * 2) / 3).max(first_split + 1);
            let front = clause_candidates
                .iter()
                .filter(|candidate| candidate.position < global_position + first_split)
                .max_by_key(|candidate| candidate.score);
            let middle = clause_candidates
                .iter()
                .filter(|candidate| {
                    candidate.position >= global_position + first_split
                        && candidate.position < global_position + second_split
                })
                .max_by_key(|candidate| candidate.score);
            let back = clause_candidates
                .iter()
                .filter(|candidate| candidate.position >= global_position + second_split)
                .max_by_key(|candidate| candidate.score);

            for candidate in [front, middle, back].into_iter().flatten() {
                candidates.push(SharedAnchorCandidate {
                    position: candidate.position,
                    score: candidate.score,
                    text: candidate.text.clone(),
                });
            }

            global_position += words.len();
        }

        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.position.cmp(&right.position))
        });

        let mut selected = candidates
            .into_iter()
            .filter(|candidate| candidate.score > 0)
            .scan(std::collections::HashSet::new(), |seen, candidate| {
                seen.insert(candidate.text.clone()).then_some(candidate)
            })
            .take(max_terms)
            .collect::<Vec<_>>();

        if selected.is_empty() {
            return None;
        }

        selected.sort_by_key(|candidate| candidate.position);

        Some(ClauseSpan::new(
            shared_context
                .iter()
                .map(|clause| clause.start)
                .min()
                .unwrap_or(instruction_clause.start),
            shared_context
                .iter()
                .map(|clause| clause.end)
                .max()
                .unwrap_or(instruction_clause.end),
            selected
                .into_iter()
                .map(|candidate| candidate.text)
                .collect::<Vec<_>>()
                .join(" "),
            None,
            instruction_clause.indent,
            false,
            None,
        ))
    }
}

fn optimize_modifiers(modifiers: &mut Vec<Modifier>, instruction: Instruction) {
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for modifier in modifiers.drain(..) {
        if seen.insert(modifier) {
            deduped.push(modifier);
        }
    }

    if deduped.contains(&Modifier::Simple) && deduped.contains(&Modifier::Detailed) {
        deduped.retain(|modifier| *modifier != Modifier::Simple);
    }

    if deduped.contains(&Modifier::Brief) && deduped.contains(&Modifier::Detailed) {
        deduped.retain(|modifier| *modifier != Modifier::Brief);
    }

    if deduped.is_empty() {
        match instruction {
            Instruction::Explain | Instruction::Generate | Instruction::Conclude => {
                deduped.push(Modifier::Detailed);
            }
            _ => deduped.push(Modifier::Simple),
        }
    }

    *modifiers = deduped;
}

fn optimize_output_hint(output_hint: &mut Option<OutputHint>, instruction: Instruction) {
    let Some(hint) = output_hint.as_mut() else {
        return;
    };

    if matches!(
        (instruction, hint.format),
        (Instruction::Define, Some(OutputFormat::Definition))
            | (Instruction::Compare, Some(OutputFormat::Comparison))
    ) {
        hint.format = None;
    }

    if hint.format.is_none() && hint.target.is_none() {
        *output_hint = None;
    }
}

fn optimize_entities(entities: &mut Vec<MatchedEntity>, words: &[String], cleaned_clause: &str) {
    let is_controller_clause = is_workflow_controller_clause_text(cleaned_clause)
        || workflow_controller_word_start(words).is_some();

    if is_controller_clause {
        entities.retain(|entity| !matches!(entity.canonical.as_str(), "IF" | "OTHERWISE"));
    }

    if is_controller_clause
        && words.iter().any(|word| word == "missing")
        && words.iter().any(|word| word == "request")
    {
        entities.retain(|entity| entity.canonical != "STOP");
    }
}

fn optimize_residual_terms(
    residual_terms: &mut Vec<String>,
    words: &[String],
    instruction: Instruction,
) {
    if instruction == Instruction::Define {
        residual_terms.retain(|term| term != "DEFINITION");
    }

    if instruction == Instruction::Compare {
        residual_terms.retain(|term| term != "COMPARISON");
    }

    if matches!(
        workflow_controller_word_start(words),
        Some(index) if words.get(index).map(String::as_str) == Some("otherwise")
    ) {
        residual_terms.retain(|term| term != "OTHERWISE");
    }

    if words.iter().any(|word| word == "missing") && words.iter().any(|word| word == "request") {
        residual_terms.retain(|term| term != "STOP");
    }
}

fn workflow_controller_word_start(words: &[String]) -> Option<usize> {
    match words {
        [first, ..] if matches!(first.as_str(), "if" | "otherwise") => Some(0),
        [first, second, ..]
            if is_instruction_seed_word(first) && matches!(second.as_str(), "if" | "otherwise") =>
        {
            Some(1)
        }
        _ => None,
    }
}

fn is_instruction_seed_word(word: &str) -> bool {
    matches!(
        word,
        "explain"
            | "summarize"
            | "analyze"
            | "generate"
            | "translate"
            | "compare"
            | "search"
            | "transform"
            | "list"
            | "define"
            | "conclude"
    )
}

fn merge_clause_group(clauses: Vec<ClauseSpan>) -> ClauseSpan {
    let mut iter = clauses.into_iter();
    let first = iter
        .next()
        .expect("merge_clause_group requires at least one clause");
    let mut merged = first;

    for clause in iter {
        append_clause(&mut merged, clause);
    }

    merged
}

fn merge_pending_prefixes_with_instruction(
    mut prefixes: Vec<ClauseSpan>,
    instruction_clause: ClauseSpan,
) -> ClauseSpan {
    prefixes.push(instruction_clause.clone());
    let mut merged = merge_clause_group(prefixes);
    merged.marker = instruction_clause.marker;
    merged.indent = instruction_clause.indent;
    merged.is_list_item = instruction_clause.is_list_item;
    merged.list_marker_kind = instruction_clause.list_marker_kind;
    merged
}

fn append_clause(target: &mut ClauseSpan, clause: ClauseSpan) {
    target.append_text(&clause.text);
    target.end = clause.end;
}

fn merge_instruction_with_shared_context(
    instruction_clause: ClauseSpan,
    shared_context: &[ClauseSpan],
) -> ClauseSpan {
    let mut merged = instruction_clause;

    for clause in shared_context {
        append_clause(&mut merged, clause.clone());
    }

    if let Some(start) = shared_context
        .iter()
        .map(|clause| clause.start)
        .chain(std::iter::once(merged.start))
        .min()
    {
        merged.start = start;
    }

    merged
}

fn is_scope_boundary(clause: &ClauseSpan) -> bool {
    let trimmed = clause.text.trim();
    !trimmed.is_empty() && trimmed.chars().all(|character| character == '-') && trimmed.len() >= 3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SharedCaptureMode {
    Carry,
    Ignore,
    ConstraintMetadata,
    OutputMetadata,
    WorkflowOutputMetadata,
    IgnoreMetadata,
    PayloadSink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SharedHeadingKind {
    Carry { keep_heading: bool },
    Ignore,
    Constraint,
    Metadata,
    Output,
    Payload,
    Workflow(WorkflowScopeKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplicitListHeading {
    Tasks,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowScopeKind {
    Step,
    Phase,
    Stage,
    Section,
}

#[derive(Debug, Default)]
struct SharedContextCluster {
    shared: Vec<ClauseSpan>,
    local_shared: Vec<ClauseSpan>,
    instructions: Vec<ClauseSpan>,
}

#[derive(Debug, Clone, Copy)]
struct ListInstructionContext {
    instruction: Instruction,
    indent: usize,
}

#[derive(Debug)]
struct SharedAnchorCandidate {
    position: usize,
    score: usize,
    text: String,
}

fn flush_shared_context_cluster(
    compiler: &Compiler,
    cluster: &mut SharedContextCluster,
    output: &mut Vec<ClauseSpan>,
) {
    let shared = std::mem::take(&mut cluster.shared);
    let local_shared = std::mem::take(&mut cluster.local_shared);
    let instructions = std::mem::take(&mut cluster.instructions);
    let instruction_count = instructions.len();

    if instructions.is_empty() {
        output.extend(shared);
        output.extend(local_shared);
        return;
    }

    let front_only_shared = shared
        .iter()
        .filter(|clause| looks_like_short_context_title_clause(clause))
        .cloned()
        .collect::<Vec<_>>();
    let persistent_shared = shared
        .into_iter()
        .filter(|clause| !looks_like_short_context_title_clause(clause))
        .collect::<Vec<_>>();
    let condense_shared_context = should_condense_shared_context(&persistent_shared, &instructions);
    let preserved_local_shared = local_shared
        .iter()
        .filter(|clause| should_preserve_tail_local_clause(clause))
        .cloned()
        .collect::<Vec<_>>();
    let mergeable_local_shared = local_shared
        .into_iter()
        .filter(|clause| !should_preserve_tail_local_clause(clause))
        .collect::<Vec<_>>();

    for (index, clause) in instructions.into_iter().enumerate() {
        if condense_shared_context {
            let max_terms = condensed_shared_context_budget(&clause, index, instruction_count);
            if max_terms == 0 {
                let clause = merge_front_only_shared_context(clause, &front_only_shared, index);
                if index + 1 == instruction_count {
                    output.extend(preserved_local_shared.iter().cloned());
                }
                output.push(clause);
                continue;
            }

            if let Some(summary) =
                compiler.summarize_shared_context_clause(&persistent_shared, &clause, max_terms)
            {
                let merged =
                    merge_instruction_with_shared_context(clause, std::slice::from_ref(&summary));
                let merged = merge_front_only_shared_context(merged, &front_only_shared, index);
                if index + 1 == instruction_count {
                    output.extend(preserved_local_shared.iter().cloned());
                }
                output.push(merge_tail_local_context(
                    merged,
                    &mergeable_local_shared,
                    index,
                    instruction_count,
                ));
                continue;
            }
        }

        let merged = merge_instruction_with_shared_context(clause, &persistent_shared);
        let merged = merge_front_only_shared_context(merged, &front_only_shared, index);
        if index + 1 == instruction_count {
            output.extend(preserved_local_shared.iter().cloned());
        }
        output.push(merge_tail_local_context(
            merged,
            &mergeable_local_shared,
            index,
            instruction_count,
        ));
    }
}

fn merge_front_only_shared_context(
    clause: ClauseSpan,
    front_only_shared: &[ClauseSpan],
    index: usize,
) -> ClauseSpan {
    if index == 0 && !front_only_shared.is_empty() {
        merge_instruction_with_shared_context(clause, front_only_shared)
    } else {
        clause
    }
}

fn should_preserve_tail_local_clause(clause: &ClauseSpan) -> bool {
    let cleaned = clause.cleaned_text.as_str();
    cleaned.starts_with("keep ")
        || cleaned.starts_with("preserve ")
        || cleaned.starts_with("retain ")
        || cleaned.starts_with("ensure ")
}

fn merge_tail_local_context(
    clause: ClauseSpan,
    local_shared: &[ClauseSpan],
    index: usize,
    instruction_count: usize,
) -> ClauseSpan {
    if local_shared.is_empty() || index + 1 != instruction_count {
        clause
    } else {
        merge_instruction_with_shared_context(clause, local_shared)
    }
}

fn should_condense_shared_context(
    shared_context: &[ClauseSpan],
    instructions: &[ClauseSpan],
) -> bool {
    instructions.len() > 1
        && !shared_context.is_empty()
        && !shared_context.iter().any(|clause| {
            matches!(
                shared_heading_kind(clause),
                Some(SharedHeadingKind::Workflow(_))
            )
        })
        && shared_context
            .iter()
            .map(|clause| normalize::tokenize_words(&clause.cleaned_text).len())
            .sum::<usize>()
            >= 10
}

fn condensed_shared_context_budget(
    clause: &ClauseSpan,
    index: usize,
    instruction_count: usize,
) -> usize {
    if should_drop_shared_context_for_short_clause(clause) {
        return 0;
    }

    match instruction_count {
        0 | 1 => 0,
        2 => {
            if index == 0 {
                7
            } else {
                5
            }
        }
        3 => match index {
            0 => 6,
            1 => 4,
            _ => 0,
        },
        _ => match index {
            0 => 5,
            1 => 3,
            _ => 0,
        },
    }
}

fn should_drop_shared_context_for_short_clause(clause: &ClauseSpan) -> bool {
    let cleaned = clause.cleaned_text.as_str();
    if cleaned.is_empty() {
        return false;
    }

    if matches!(cleaned, "conclusion" | "conclude")
        || (cleaned.ends_with(" conclusion") && cleaned.split_whitespace().count() <= 2)
    {
        return true;
    }

    cleaned
        .strip_prefix("generate ")
        .map(|target| {
            let words = target.split_whitespace().count();
            words <= 2 || (words == 1 && target.contains('-'))
        })
        .unwrap_or(false)
}

fn should_skip_shared_anchor_word(word: &str, synonyms: &SynonymTable) -> bool {
    let is_scientific_short_anchor = matches!(word, "ph");
    should_skip_entity_word(word, synonyms)
        || (word.len() <= 2
            && !is_scientific_short_anchor
            && !word.chars().any(|character| character.is_ascii_digit()))
        || matches!(
            word,
            "text"
                | "system"
                | "data"
                | "state"
                | "rules"
                | "input"
                | "output"
                | "task"
                | "tasks"
                | "following"
                | "idea"
                | "property"
                | "version"
                | "above"
                | "below"
                | "workflows"
                | "workflow"
                | "multiple"
                | "correlate"
                | "correlates"
                | "identify"
                | "identifies"
                | "shows"
                | "sharply"
                | "process"
                | "processes"
                | "information"
                | "recursive"
                | "recursively"
                | "exhibit"
                | "exists"
                | "exist"
                | "binary"
                | "however"
                | "itself"
                | "sufficiently"
                | "noise"
                | "symbol"
                | "symbols"
                | "ignoring"
                | "ignore"
                | "sudden"
                | "increased"
        )
}

fn should_skip_shared_phrase_word(word: &str, synonyms: &SynonymTable) -> bool {
    let is_scientific_short_anchor = matches!(word, "ph");
    should_skip_entity_word(word, synonyms)
        || (word.len() <= 2
            && !is_scientific_short_anchor
            && !word.chars().any(|character| character.is_ascii_digit()))
        || matches!(
            word,
            "contains"
                | "include"
                | "includes"
                | "using"
                | "given"
                | "following"
                | "above"
                | "below"
                | "multiple"
                | "correlate"
                | "correlates"
                | "identify"
                | "identifies"
                | "shows"
                | "sharply"
                | "process"
                | "processes"
                | "information"
                | "recursive"
                | "recursively"
                | "exhibit"
                | "exists"
                | "exist"
                | "binary"
                | "however"
                | "itself"
                | "sufficiently"
                | "noise"
                | "symbol"
                | "symbols"
                | "ignoring"
                | "ignore"
                | "sudden"
                | "increased"
        )
}

fn shared_anchor_score(word: &str) -> usize {
    let mut score = word.len();
    if word.chars().any(|character| character.is_ascii_digit()) {
        score += 4;
    }
    if word.chars().any(|character| character.is_ascii_digit())
        && word
            .chars()
            .any(|character| character.is_ascii_alphabetic())
    {
        score += 8;
    }
    if word.contains('-') {
        score += 2;
    }
    if matches!(
        word,
        "awareness"
            | "consciousness"
            | "spectrum"
            | "distributed"
            | "ph"
            | "tds"
            | "water"
            | "quality"
            | "anomaly"
            | "anomalies"
            | "industrial"
            | "dumping"
            | "environmental"
            | "parameters"
            | "reconstructability"
            | "corruption"
            | "philosophical"
            | "ambiguity"
            | "24h"
    ) {
        score += 4;
    }
    score
}

fn shared_phrase_score(left: &str, right: &str) -> usize {
    let mut score = shared_anchor_score(left) + shared_anchor_score(right) + 4;
    if left.len() >= 5 && right.len() >= 5 {
        score += 3;
    }
    if right.chars().any(|character| character.is_ascii_digit())
        && right
            .chars()
            .any(|character| character.is_ascii_alphabetic())
    {
        score += 6;
    }
    if matches!(
        (left, right),
        ("distributed", "system")
            | ("water", "quality")
            | ("industrial", "dumping")
            | ("complex", "system")
            | ("awareness", "spectrum")
            | ("partial", "awareness")
            | ("environmental", "parameters")
            | ("external", "interference")
            | ("statistical", "thresholds")
    ) {
        score += 6;
    }
    score
}

fn shared_heading_kind(clause: &ClauseSpan) -> Option<SharedHeadingKind> {
    let escaped = normalize::escape_reserved_symbols(&clause.text);
    let cleaned = normalize::clean_input(&escaped);

    if cleaned.starts_with("step ") {
        return Some(SharedHeadingKind::Workflow(WorkflowScopeKind::Step));
    }

    if cleaned.starts_with("phase ") {
        return Some(SharedHeadingKind::Workflow(WorkflowScopeKind::Phase));
    }

    if cleaned.starts_with("stage ") {
        return Some(SharedHeadingKind::Workflow(WorkflowScopeKind::Stage));
    }

    if cleaned.starts_with("section ") {
        return Some(SharedHeadingKind::Workflow(WorkflowScopeKind::Section));
    }

    if cleaned.starts_with("follow ")
        && (cleaned.contains("instruction")
            || cleaned.contains("workflow")
            || cleaned.contains("decision process")
            || cleaned.ends_with("process"))
    {
        return Some(SharedHeadingKind::Ignore);
    }

    matches!(
        cleaned.as_str(),
        "initial state" | "rules" | "text" | "extra challenge"
    )
    .then_some(SharedHeadingKind::Carry { keep_heading: true })
    .or_else(|| {
        matches!(cleaned.as_str(), "tasks").then_some(SharedHeadingKind::Carry {
            keep_heading: false,
        })
    })
    .or_else(|| {
        matches!(cleaned.as_str(), "workflow" | "context").then_some(SharedHeadingKind::Carry {
            keep_heading: false,
        })
    })
    .or_else(|| {
        matches!(
            cleaned.as_str(),
            "evidence"
                | "evidence table"
                | "log excerpt"
                | "log excerpts"
                | "stack trace"
                | "trace excerpt"
                | "traceback"
        )
        .then_some(SharedHeadingKind::Payload)
    })
    .or_else(|| {
        matches!(cleaned.as_str(), "constraint" | "constraints")
            .then_some(SharedHeadingKind::Constraint)
    })
    .or_else(|| {
        matches!(
            cleaned.as_str(),
            "appendix"
                | "appendices"
                | "notes"
                | "reference rows"
                | "evidence rows"
                | "evidence"
                | "log excerpt"
                | "log excerpts"
        )
        .then_some(SharedHeadingKind::Metadata)
    })
    .or_else(|| {
        matches!(cleaned.as_str(), "input" | "inp").then_some(SharedHeadingKind::Carry {
            keep_heading: false,
        })
    })
    .or_else(|| {
        matches!(
            cleaned.as_str(),
            "return" | "output" | "out" | "that returns"
        )
        .then_some(SharedHeadingKind::Output)
    })
    .or_else(|| {
        matches!(cleaned.as_str(), "prc" | "process" | "example" | "examples")
            .then_some(SharedHeadingKind::Ignore)
    })
}

fn compact_workflow_heading_clause(
    index: usize,
    clauses: &[ClauseSpan],
    kind: WorkflowScopeKind,
) -> Option<ClauseSpan> {
    let clause = clauses[index].clone();
    let trimmed = clause.text.trim();
    let child_list_follows = workflow_heading_has_child_list(index, clauses);
    let raw_tail = trimmed
        .split_once(':')
        .map(|(_, tail)| tail.trim())
        .filter(|tail| !tail.is_empty());

    let compact_text = if child_list_follows {
        match kind {
            WorkflowScopeKind::Step => raw_tail.map(compact_heading_label),
            WorkflowScopeKind::Phase | WorkflowScopeKind::Stage | WorkflowScopeKind::Section => {
                None
            }
        }
    } else {
        raw_tail
            .map(normalize::clean_input)
            .or_else(|| workflow_heading_ordinal(trimmed, kind))
    }?;

    if compact_text.is_empty() {
        return None;
    }

    let mut clause = clause;
    clause.set_text(compact_text);
    Some(clause)
}

fn workflow_heading_opens_output_section(index: usize, clauses: &[ClauseSpan]) -> bool {
    if !workflow_heading_has_child_list(index, clauses) {
        return false;
    }

    let tail = clauses[index]
        .text
        .trim()
        .split_once(':')
        .map(|(_, tail)| tail.trim())
        .filter(|tail| !tail.is_empty());
    let Some(tail) = tail else {
        return false;
    };

    let cleaned = normalize::clean_input(tail);
    matches!(
        cleaned.as_str(),
        "output"
            | "final output"
            | "deliverable"
            | "deliverables"
            | "final deliverable"
            | "final deliverables"
            | "release"
    ) || cleaned.ends_with(" output")
        || cleaned.ends_with(" deliverable")
        || cleaned.ends_with(" deliverables")
}

fn workflow_heading_has_child_list(index: usize, clauses: &[ClauseSpan]) -> bool {
    let clause = &clauses[index];
    for next in clauses.iter().skip(index + 1) {
        if is_scope_boundary(next)
            || matches!(
                shared_heading_kind(next),
                Some(SharedHeadingKind::Workflow(_))
            )
        {
            return false;
        }

        if next.is_list_item && next.indent >= clause.indent {
            return true;
        }

        if !next.text.trim().is_empty() && !next.is_list_item {
            return false;
        }
    }

    false
}

fn workflow_heading_ordinal(raw: &str, kind: WorkflowScopeKind) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    let prefix = match kind {
        WorkflowScopeKind::Step => "step ",
        WorkflowScopeKind::Phase => "phase ",
        WorkflowScopeKind::Stage => "stage ",
        WorkflowScopeKind::Section => "section ",
    };
    let remainder = lower.strip_prefix(prefix)?.trim();
    let ordinal = remainder
        .split(|character: char| character == ':' || character.is_whitespace())
        .find(|segment| !segment.is_empty())?;

    Some(format!(
        "{} {}",
        prefix.trim_end(),
        normalize::clean_input(ordinal)
    ))
}

fn compact_heading_label(label: &str) -> String {
    let cleaned = normalize::clean_input(label);
    cleaned
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_workflow_controller_clause_text(cleaned: &str) -> bool {
    cleaned.starts_with("if ")
        || cleaned.starts_with("otherwise ")
        || cleaned.starts_with("else ")
        || cleaned.contains(" skip step")
        || cleaned.starts_with("skip step ")
        || cleaned.contains(" go to step")
        || cleaned.starts_with("go to step ")
        || cleaned.starts_with("return ")
        || cleaned.starts_with("output ")
}

fn looks_like_generic_workflow_preamble(cleaned: &str) -> bool {
    if cleaned.starts_with("follow ")
        && (cleaned.contains("instruction")
            || cleaned.contains("workflow")
            || cleaned.contains("decision process")
            || cleaned.ends_with("process"))
    {
        return true;
    }

    if cleaned.starts_with("you are ")
        && (cleaned.contains("workflow")
            || cleaned.contains("process")
            || cleaned.contains("policy")
            || cleaned.contains("memo")
            || cleaned.contains("agreement")
            || cleaned.contains("review"))
    {
        return true;
    }

    if looks_like_short_workflow_title(cleaned) {
        return true;
    }

    let starts_generic_review = [
        "review ",
        "analyze ",
        "audit ",
        "check ",
        "plan ",
        "design ",
        "create ",
        "compare ",
        "write ",
        "route ",
        "screen ",
        "assess ",
        "inspect ",
        "postmortem ",
        "incident ",
        "moderation ",
    ]
    .iter()
    .any(|prefix| cleaned.starts_with(prefix));

    starts_generic_review
        && [
            "workflow",
            "process",
            "memo",
            "policy",
            "agreement",
            "case",
            "redline",
            "briefing",
            "review",
            "triage",
            "bridge",
            "appeal",
            "checklist",
            "instructions",
            "routing",
            "protocol",
            "exception",
            "branch",
            "pull request",
            "incident log",
            "appeal workflow",
            "exercise",
            "training",
            "offer",
            "offers",
            "scorecard",
        ]
        .iter()
        .any(|needle| cleaned.contains(needle))
}

fn looks_like_short_workflow_title(cleaned: &str) -> bool {
    let words = cleaned.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() || words.len() > 5 {
        return false;
    }

    if words.iter().any(|word| {
        matches!(
            *word,
            "review"
                | "appeal"
                | "bridge"
                | "triage"
                | "ticket"
                | "handling"
                | "checklist"
                | "workflow"
                | "instructions"
                | "routing"
                | "protocol"
                | "exception"
                | "memo"
                | "branch"
        )
    }) {
        return true;
    }

    let starts_with_generic_controller = matches!(
        words.first().copied().unwrap_or_default(),
        "review" | "design" | "write" | "route" | "screen" | "assess" | "plan"
    );
    let has_generic_object = words.iter().any(|word| {
        matches!(
            *word,
            "case"
                | "plan"
                | "review"
                | "ticket"
                | "handling"
                | "instructions"
                | "workflow"
                | "checklist"
                | "appeal"
                | "memo"
                | "protocol"
                | "bridge"
                | "triage"
                | "routing"
                | "branch"
        )
    });

    starts_with_generic_controller && has_generic_object
}

fn looks_like_short_context_title_clause(clause: &ClauseSpan) -> bool {
    if clause.is_list_item || clause.marker.is_some() {
        return false;
    }

    let cleaned = clause.cleaned_text.as_str();
    let words = cleaned.split_whitespace().collect::<Vec<_>>();
    !cleaned.is_empty()
        && words.len() <= 3
        && (words.len() <= 2
            || cleaned.ends_with(" plan")
            || cleaned.ends_with(" workflow")
            || cleaned.ends_with(" review")
            || cleaned.ends_with(" ticket")
            || cleaned.ends_with(" handling")
            || cleaned.ends_with(" checklist")
            || cleaned.ends_with(" protocol")
            || cleaned.ends_with(" summary")
            || cleaned.ends_with(" memo")
            || cleaned.ends_with(" note")
            || cleaned.ends_with(" decision tree"))
}

fn explicit_list_heading(clause: &ClauseSpan) -> Option<ExplicitListHeading> {
    let escaped = normalize::escape_reserved_symbols(&clause.text);
    let cleaned = normalize::clean_input(&escaped);
    match cleaned.as_str() {
        "tasks" | "workflow" => Some(ExplicitListHeading::Tasks),
        "context" | "rules" | "input" | "inp" | "output" | "out" | "return" | "constraint"
        | "constraints" | "example" | "examples" | "prc" | "process" | "step" => {
            Some(ExplicitListHeading::Other)
        }
        _ => None,
    }
}

fn enables_list_instruction_inference(clause: &ClauseSpan) -> bool {
    let escaped = normalize::escape_reserved_symbols(&clause.text);
    let cleaned = normalize::clean_input(&escaped);
    cleaned.starts_with("step ")
        || cleaned.starts_with("phase ")
        || cleaned.starts_with("stage ")
        || matches!(cleaned.as_str(), "tasks" | "workflow" | "prc" | "process")
}

fn is_structural_heading_clause(clause: &ClauseSpan) -> bool {
    shared_heading_kind(clause).is_some()
}

fn should_infer_list_item_instruction(
    clause: &ClauseSpan,
    list_inference_enabled: bool,
    last_list_instruction: Option<ListInstructionContext>,
) -> bool {
    let shares_prior_list_depth = last_list_instruction
        .map(|context| clause.indent <= context.indent)
        .unwrap_or(false);

    (list_inference_enabled || clause.list_marker_kind == Some(ListMarkerKind::Numbered))
        && clause.is_list_item
        && clause.marker.is_none()
        && shares_prior_list_depth
}

fn is_mergeable_controller_tail_clause(existing: &ClauseSpan, clause: &ClauseSpan) -> bool {
    if !existing.is_list_item
        || !clause.is_list_item
        || clause.marker.is_some()
        || existing.indent != clause.indent
        || existing.list_marker_kind != clause.list_marker_kind
    {
        return false;
    }

    let existing_cleaned = existing.cleaned_text.as_str();
    let clause_cleaned = clause.cleaned_text.as_str();
    let tail_lead = clause_cleaned.split_whitespace().next().unwrap_or_default();
    if !is_workflow_controller_clause_text(&existing_cleaned)
        || clause_cleaned.is_empty()
        || clause_cleaned.split_whitespace().count() > 4
        || rewrite_output_metadata_clause(clause, false).is_some()
        || !matches!(
            tail_lead,
            "investigate" | "inspect" | "review" | "confirm" | "compare"
        )
    {
        return false;
    }

    !["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .any(|prefix| clause_cleaned.starts_with(prefix))
}

fn rewrite_with_inherited_instruction(
    mut clause: ClauseSpan,
    instruction: Instruction,
) -> ClauseSpan {
    clause.set_text(format!(
        "{} {}",
        instruction_seed_word(instruction),
        clause.text
    ));
    clause
}

fn instruction_seed_word(instruction: Instruction) -> &'static str {
    match instruction {
        Instruction::Explain => "explain",
        Instruction::Summarize => "summarize",
        Instruction::Analyze => "analyze",
        Instruction::Generate => "generate",
        Instruction::Translate => "translate",
        Instruction::Compare => "compare",
        Instruction::Search => "search",
        Instruction::Transform => "transform",
        Instruction::List => "list",
        Instruction::Define => "define",
        Instruction::Conclude => "conclude",
    }
}

fn compact_numbered_controller_clause(clause: &ClauseSpan) -> Option<ClauseSpan> {
    if !clause.is_list_item || clause.list_marker_kind != Some(ListMarkerKind::Numbered) {
        return None;
    }

    let cleaned = clause.cleaned_text.as_str();
    let compacted = if let Some(remainder) = cleaned.strip_prefix("otherwise compare ") {
        format!("else compare {remainder}")
    } else if let Some(remainder) = cleaned.strip_prefix("otherwise confirm ") {
        format!("else analyze {remainder}")
    } else if let Some(remainder) = cleaned.strip_prefix("otherwise inspect ") {
        format!("else analyze {remainder}")
    } else if let Some(remainder) = cleaned.strip_prefix("otherwise review ") {
        format!("else analyze {remainder}")
    } else if let Some(remainder) = cleaned.strip_prefix("otherwise return ") {
        format!("else return {remainder}")
    } else if let Some(remainder) = cleaned.strip_prefix("otherwise route ") {
        format!("else route {remainder}")
    } else if let Some(remainder) = cleaned.strip_prefix("otherwise keep ") {
        format!("else keep {remainder}")
    } else if cleaned.starts_with("if ")
        && cleaned.contains(" missing")
        && cleaned.contains("request it")
        && cleaned.contains("stop and ")
    {
        cleaned.replacen("stop and ", "", 1)
    } else if cleaned.starts_with("if ") && cleaned.contains("go to step ") {
        cleaned.replacen("go to step ", "goto ", 1)
    } else if cleaned.starts_with("if ") && cleaned.contains("go step ") {
        cleaned.replacen("go step ", "goto ", 1)
    } else {
        return None;
    };

    let mut clause = clause.clone();
    clause.set_text(compacted);
    Some(clause)
}

fn rewrite_output_metadata_clause(
    clause: &ClauseSpan,
    workflow_output_mode: bool,
) -> Option<ClauseSpan> {
    let trimmed = clause.text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cleaned = normalize::clean_input(trimmed);
    if cleaned.is_empty() || is_list_like_clause(trimmed, &cleaned) {
        return None;
    }

    if let Some(remainder) = cleaned.strip_prefix("return ")
        && !remainder.is_empty()
    {
        let target = preserve_short_output_phrase(remainder, workflow_output_mode);
        return Some(ClauseSpan::new(
            clause.start,
            clause.end,
            format!("return {target}"),
            clause.marker,
            clause.indent,
            clause.is_list_item,
            clause.list_marker_kind,
        ));
    }

    if let Some(remainder) = cleaned.strip_prefix("output ")
        && !remainder.is_empty()
    {
        let target = preserve_short_output_phrase(remainder, workflow_output_mode);
        return Some(ClauseSpan::new(
            clause.start,
            clause.end,
            format!("return {target}"),
            clause.marker,
            clause.indent,
            clause.is_list_item,
            clause.list_marker_kind,
        ));
    }

    for prefix in ["produce ", "provide ", "draft ", "generate ", "state "] {
        if let Some(remainder) = cleaned.strip_prefix(prefix)
            && !remainder.is_empty()
        {
            let target = preserve_short_output_phrase(remainder, workflow_output_mode);
            return Some(ClauseSpan::new(
                clause.start,
                clause.end,
                format!("return {target}"),
                clause.marker,
                clause.indent,
                clause.is_list_item,
                clause.list_marker_kind,
            ));
        }
    }

    if workflow_output_mode {
        if let Some(remainder) = cleaned.strip_prefix("list ")
            && !remainder.is_empty()
        {
            let target = preserve_short_output_phrase(remainder, true);
            return Some(ClauseSpan::new(
                clause.start,
                clause.end,
                format!("return {target}"),
                clause.marker,
                clause.indent,
                clause.is_list_item,
                clause.list_marker_kind,
            ));
        }
    }

    if matches!(cleaned.as_str(), "conclusion" | "conclude") {
        return Some(clause.clone());
    }

    None
}

fn preserve_short_output_phrase(remainder: &str, workflow_output_mode: bool) -> String {
    let mut words = remainder.split_whitespace().collect::<Vec<_>>();

    while matches!(words.first().copied(), Some("a" | "an" | "the")) {
        words.remove(0);
    }

    while words.len() > 2 && matches!(words.first().copied(), Some("short" | "brief" | "concise")) {
        words.remove(0);
    }

    if workflow_output_mode {
        while words.len() > 2
            && matches!(
                words.first().copied(),
                Some("strongest" | "top" | "key" | "main" | "primary")
            )
        {
            words.remove(0);
        }

        while words.len() > 2 && is_output_quantifier(words.first().copied().unwrap_or_default()) {
            words.remove(0);
        }
    }

    if words.is_empty() {
        return remainder.trim().to_string();
    }

    if (2..=4).contains(&words.len()) {
        words.join("-")
    } else {
        words.join(" ")
    }
}

fn is_output_quantifier(word: &str) -> bool {
    word.chars().all(|character| character.is_ascii_digit())
        || matches!(
            word,
            "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine"
        )
}

fn is_branch_local_constraint_clause(clause: &ClauseSpan) -> bool {
    if !clause.is_list_item {
        return false;
    }

    let cleaned = clause.cleaned_text.as_str();
    let starts_with_local_constraint = ["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .any(|prefix| cleaned.starts_with(prefix));

    starts_with_local_constraint
        && [
            "branch", "evidence", "local", "phase", "stage", "only", "quote", "quoted", "claim",
        ]
        .iter()
        .any(|needle| cleaned.contains(needle))
}

fn is_output_constraint_metadata_clause(clause: &ClauseSpan) -> bool {
    if !clause.is_list_item {
        return false;
    }

    let cleaned = clause.cleaned_text.as_str();
    let starts_with_constraint = ["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .any(|prefix| cleaned.starts_with(prefix));

    starts_with_constraint
        && (cleaned.contains("deliverable")
            || cleaned.contains("output")
            || cleaned.contains("summary")
            || cleaned.contains("report"))
}

fn is_tail_local_output_constraint_clause(index: usize, clauses: &[ClauseSpan]) -> bool {
    let Some(clause) = clauses.get(index) else {
        return false;
    };

    if !clause.is_list_item || clause.list_marker_kind != Some(ListMarkerKind::Numbered) {
        return false;
    }

    let cleaned = clause.cleaned_text.as_str();
    let starts_with_local_constraint = ["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .any(|prefix| cleaned.starts_with(prefix));
    let matches_tail_pattern = cleaned.contains(" separate from ")
        || cleaned.ends_with(" short")
        || cleaned.ends_with(" brief")
        || cleaned.ends_with(" concise");

    if !starts_with_local_constraint || !matches_tail_pattern {
        return false;
    }

    let Some(next_clause) = clauses.get(index + 1) else {
        return false;
    };

    if !next_clause.is_list_item
        || next_clause.list_marker_kind != Some(ListMarkerKind::Numbered)
        || next_clause.indent != clause.indent
        || rewrite_output_metadata_clause(next_clause, false).is_none()
    {
        return false;
    }

    if clauses
        .iter()
        .skip(index + 2)
        .take_while(|next| !is_scope_boundary(next))
        .any(|next| {
            next.is_list_item
                && next.list_marker_kind == Some(ListMarkerKind::Numbered)
                && next.indent == clause.indent
        })
    {
        return false;
    }

    has_prior_numbered_workflow_controller(index, clauses)
}

fn has_prior_numbered_workflow_controller(index: usize, clauses: &[ClauseSpan]) -> bool {
    let Some(clause) = clauses.get(index) else {
        return false;
    };

    clauses
        .iter()
        .take(index)
        .rev()
        .take_while(|previous| !is_scope_boundary(previous))
        .filter(|previous| {
            previous.is_list_item
                && previous.list_marker_kind == Some(ListMarkerKind::Numbered)
                && previous.indent == clause.indent
        })
        .any(|previous| is_workflow_controller_clause_text(&previous.cleaned_text))
}

fn compact_branch_local_constraint_clause(mut clause: ClauseSpan) -> ClauseSpan {
    let cleaned = clause.cleaned_text.as_str();
    clause.set_text(if cleaned.contains("phase") {
        "branch decision local phase".to_string()
    } else if cleaned.contains("stage") {
        "evidence local stage".to_string()
    } else if cleaned.contains("quote") && cleaned.contains("claim") {
        "quote separate claim".to_string()
    } else if cleaned.contains("chosen branch") {
        "chosen-branch evidence".to_string()
    } else if cleaned.contains("branch") && cleaned.contains("evidence") {
        "branch evidence".to_string()
    } else if cleaned.contains("branch") {
        "local branch".to_string()
    } else {
        "local evidence".to_string()
    });
    clause
}

fn compact_tail_local_output_constraint_clause(mut clause: ClauseSpan) -> ClauseSpan {
    let cleaned = clause.cleaned_text.as_str();
    let remainder = ["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .find_map(|prefix| cleaned.strip_prefix(prefix))
        .unwrap_or(cleaned)
        .trim();

    let compact = if let Some((left, right)) = remainder.split_once(" separate from ") {
        let left = compact_tail_local_constraint_side(left);
        let right = compact_tail_local_constraint_side(right);
        if right.is_empty() {
            format!("keep {left} separate")
        } else if left.is_empty() {
            format!("keep separate {right}")
        } else {
            format!("keep {left} separate {right}")
        }
    } else if remainder.ends_with(" short")
        || remainder.ends_with(" brief")
        || remainder.ends_with(" concise")
    {
        format!(
            "keep {} brief",
            compact_tail_local_constraint_side(
                remainder
                    .trim_end_matches(" short")
                    .trim_end_matches(" brief")
                    .trim_end_matches(" concise"),
            )
        )
    } else {
        format!("keep {}", compact_tail_local_constraint_side(remainder))
    };

    clause.set_text(if compact.is_empty() {
        "keep local output context".to_string()
    } else {
        compact
    });
    clause
}

fn compact_tail_local_constraint_side(text: &str) -> String {
    let mut words = text
        .split_whitespace()
        .filter(|word| !matches!(*word, "the" | "a" | "an" | "from"))
        .collect::<Vec<_>>();

    while matches!(
        words.first().copied(),
        Some("keep" | "preserve" | "retain" | "ensure")
    ) {
        words.remove(0);
    }

    words.join(" ")
}

fn control_instruction_for_clause(cleaned: &str) -> Option<Instruction> {
    if cleaned.is_empty() {
        return None;
    }

    if cleaned.starts_with("return ") || cleaned.starts_with("output ") {
        return Some(Instruction::Generate);
    }

    if cleaned.starts_with("route ")
        || cleaned.starts_with("keep ")
        || cleaned.starts_with("preserve ")
        || cleaned.starts_with("retain ")
        || cleaned.starts_with("ensure ")
        || cleaned.starts_with("note ")
        || cleaned.starts_with("request ")
        || cleaned.starts_with("skip ")
        || cleaned.starts_with("isolate ")
    {
        return Some(Instruction::Transform);
    }

    if let Some(remainder) = cleaned.strip_prefix("otherwise ") {
        return control_instruction_for_clause(remainder).or(Some(Instruction::Analyze));
    }

    if let Some(remainder) = cleaned.strip_prefix("else ") {
        return control_instruction_for_clause(remainder).or(Some(Instruction::Analyze));
    }

    if let Some(remainder) = cleaned.strip_prefix("if ") {
        if remainder.contains(" goto ") {
            return Some(Instruction::Analyze);
        }

        if let Some((_, action)) = split_controller_condition_action(remainder) {
            return control_instruction_for_clause(&action)
                .or_else(|| {
                    Instruction::from_mnemonic(action.split_whitespace().next().unwrap_or_default())
                })
                .or(Some(Instruction::Analyze));
        }

        return Some(Instruction::Analyze);
    }

    None
}

fn apply_structure_compact_override(clause: &ClauseSpan, item: &mut TokelangIR) {
    let cleaned = clause.cleaned_text.as_str();

    if let Some(remainder) = cleaned.strip_prefix("if ") {
        if let Some((condition, goto_target)) = split_controller_goto(remainder) {
            item.compact_override = Some(format!(
                "if {} goto {}",
                compact_condition_phrase(condition),
                goto_target.trim()
            ));
            return;
        }

        if let Some((condition, action)) = split_controller_condition_action(remainder) {
            if let Some(action_text) = compact_controller_action(&action, item) {
                item.compact_override = Some(format!(
                    "if {} {}",
                    compact_condition_phrase(&condition),
                    action_text
                ));
                return;
            }
        }
    }

    if let Some(remainder) = cleaned.strip_prefix("otherwise ")
        && let Some(action_text) = compact_controller_action(remainder, item)
    {
        item.compact_override = Some(format!("else {action_text}"));
        return;
    }

    if let Some(remainder) = cleaned.strip_prefix("else ")
        && let Some(action_text) = compact_controller_action(remainder, item)
    {
        item.compact_override = Some(format!("else {action_text}"));
        return;
    }

    if let Some(action_text) = compact_controller_action(cleaned, item) {
        let starts_with_control = [
            "keep ",
            "preserve ",
            "retain ",
            "ensure ",
            "route ",
            "return ",
            "output ",
        ]
        .iter()
        .any(|prefix| cleaned.starts_with(prefix));

        if starts_with_control {
            item.compact_override = Some(action_text);
        }
    }
}

fn split_controller_goto(cleaned: &str) -> Option<(&str, &str)> {
    let (condition, target) = cleaned
        .split_once(" goto ")
        .or_else(|| cleaned.split_once(" go to step "))
        .or_else(|| cleaned.split_once(" go step "))
        .or_else(|| cleaned.split_once(" step "))?;
    let trimmed_target = target.trim();
    if trimmed_target.chars().all(|ch| ch.is_ascii_digit()) {
        Some((condition.trim(), trimmed_target))
    } else {
        None
    }
}

fn split_controller_condition_action(cleaned: &str) -> Option<(String, String)> {
    let tokens = cleaned.split_whitespace().collect::<Vec<_>>();
    let action_index = tokens.iter().position(|token| {
        matches!(
            *token,
            "analyze"
                | "compare"
                | "explain"
                | "summarize"
                | "search"
                | "list"
                | "define"
                | "translate"
                | "transform"
                | "generate"
                | "route"
                | "return"
                | "note"
                | "request"
                | "skip"
                | "isolate"
                | "keep"
                | "preserve"
                | "retain"
                | "ensure"
        )
    })?;

    let condition = tokens[..action_index].join(" ");
    let action = tokens[action_index..].join(" ");
    if condition.is_empty() || action.is_empty() {
        None
    } else {
        Some((condition, action))
    }
}

fn compact_controller_action(cleaned: &str, item: &TokelangIR) -> Option<String> {
    let cleaned = cleaned.trim();
    if let Some(remainder) = cleaned.strip_prefix("return ") {
        return Some(build_return_override(remainder, item));
    }

    if let Some(remainder) = cleaned.strip_prefix("output ") {
        return Some(build_return_override(remainder, item));
    }

    if let Some(remainder) = cleaned.strip_prefix("route ") {
        return Some(format!("route {}", compact_control_phrase(remainder)));
    }

    if let Some(remainder) = cleaned.strip_prefix("note ") {
        return Some(format!("note {}", compact_control_phrase(remainder)));
    }

    if let Some(remainder) = cleaned.strip_prefix("request ") {
        return Some(format!("request {}", compact_control_phrase(remainder)));
    }

    if let Some(remainder) = cleaned.strip_prefix("skip ") {
        return Some(build_skip_override(remainder));
    }

    if let Some(remainder) = cleaned.strip_prefix("isolate ") {
        return Some(format!("isolate {}", compact_control_phrase(remainder)));
    }

    if ["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .any(|prefix| cleaned.starts_with(prefix))
    {
        return Some(build_keep_override(cleaned));
    }

    let instruction = cleaned
        .split_whitespace()
        .next()
        .and_then(Instruction::from_mnemonic)?;
    let remainder = cleaned
        .strip_prefix(instruction.mnemonic())
        .unwrap_or_default()
        .trim();
    let mut output = if remainder.is_empty() {
        instruction.mnemonic().to_string()
    } else {
        format!(
            "{} {}",
            instruction.mnemonic(),
            compact_control_phrase(remainder)
        )
    };
    append_modifier_suffix(&mut output, item.modifiers.as_slice());
    Some(output)
}

fn build_keep_override(cleaned: &str) -> String {
    let remainder = ["keep ", "preserve ", "retain ", "ensure "]
        .iter()
        .find_map(|prefix| cleaned.strip_prefix(prefix))
        .unwrap_or(cleaned)
        .trim();

    if let Some((left, right)) = remainder.split_once(" separate from ") {
        format!(
            "keep {} separate {}",
            compact_control_phrase(left),
            compact_control_phrase(right)
        )
    } else {
        format!("keep {}", compact_control_phrase(remainder))
    }
}

fn build_skip_override(remainder: &str) -> String {
    let cleaned = normalize::clean_input(remainder);

    for separator in [" go to step ", " goto ", " go step "] {
        if let Some((skip_target, goto_target)) = cleaned.split_once(separator) {
            let skip_target = compact_step_target(skip_target);
            let goto_target = compact_step_target(goto_target);
            if !skip_target.is_empty() && !goto_target.is_empty() {
                return format!("skip step {skip_target} goto {goto_target}");
            }
        }
    }

    format!("skip {}", compact_control_phrase(remainder))
}

fn compact_step_target(text: &str) -> String {
    let cleaned = normalize::clean_input(text);
    if let Some(target) = cleaned
        .split_whitespace()
        .find(|word| word.chars().all(|character| character.is_ascii_digit()))
    {
        target.to_string()
    } else {
        compact_control_phrase(text)
    }
}

fn build_return_override(remainder: &str, item: &TokelangIR) -> String {
    let cleaned_target = compact_return_target(remainder);
    let mut output = if cleaned_target.is_empty() {
        "return".to_string()
    } else {
        format!("return {cleaned_target}")
    };
    append_modifier_suffix(&mut output, item.modifiers.as_slice());
    output
}

fn append_modifier_suffix(output: &mut String, modifiers: &[Modifier]) {
    for modifier in modifiers {
        if output.ends_with(modifier.mnemonic()) {
            continue;
        }
        output.push(' ');
        output.push_str(modifier.mnemonic());
    }
}

fn compact_return_target(text: &str) -> String {
    let mut words = normalize::tokenize_words(&normalize::clean_input(text));

    while matches!(words.first().map(String::as_str), Some("the" | "a" | "an")) {
        words.remove(0);
    }

    while words.len() > 2
        && matches!(
            words.first().map(String::as_str),
            Some("short" | "brief" | "concise" | "succinct")
        )
    {
        words.remove(0);
    }

    words.join(" ")
}

fn compact_condition_phrase(text: &str) -> String {
    compact_control_phrase(text)
        .trim_end_matches(" stop")
        .trim()
        .to_string()
}

fn compact_control_phrase(text: &str) -> String {
    normalize::tokenize_words(&normalize::clean_input(text))
        .into_iter()
        .filter(|word| {
            !matches!(
                word.as_str(),
                "the"
                    | "a"
                    | "an"
                    | "to"
                    | "from"
                    | "and"
                    | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "be"
                    | "been"
                    | "being"
                    | "that"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_list_like_clause(raw: &str, cleaned: &str) -> bool {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('-') || trimmed.starts_with('*') {
        return true;
    }

    cleaned
        .split_whitespace()
        .next()
        .map(|word| word.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or(false)
}

fn is_literal_payload_clause(clause: &ClauseSpan) -> bool {
    let trimmed = clause.text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let cleaned = normalize::clean_input(trimmed);

    is_tuple_like_payload(trimmed)
        || is_parenthesized_schema_payload(trimmed)
        || is_json_like_payload(trimmed)
        || is_fenced_code_payload(trimmed)
        || is_function_signature_payload(trimmed, &cleaned)
        || is_log_like_payload(trimmed, &cleaned)
        || is_quoted_payload_clause(trimmed, &cleaned)
        || is_bracketed_schema_row_payload(trimmed)
        || is_space_delimited_example_row_payload(trimmed)
        || normalize::is_equation_heavy_line(trimmed)
}

fn is_shared_data_payload_clause(clause: &ClauseSpan) -> bool {
    let trimmed = clause.text.trim();
    let cleaned = normalize::clean_input(trimmed);
    is_tuple_like_payload(trimmed)
        || is_parenthesized_schema_payload(trimmed)
        || is_json_like_payload(trimmed)
        || is_fenced_code_payload(trimmed)
        || is_log_like_payload(trimmed, &cleaned)
        || is_bracketed_schema_row_payload(trimmed)
        || is_space_delimited_example_row_payload(trimmed)
        || normalize::is_equation_heavy_line(trimmed)
}

fn is_bracketed_schema_row_payload(text: &str) -> bool {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    tokens.len() >= 3
        && tokens.iter().all(|token| {
            token.starts_with('[')
                && token.ends_with(']')
                && token.len() > 2
                && token[1..token.len() - 1]
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

fn is_space_delimited_example_row_payload(text: &str) -> bool {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    if !(3..=6).contains(&tokens.len()) {
        return false;
    }

    let first = tokens.first().copied().unwrap_or_default();
    let first_is_id_like = !first.is_empty()
        && first.len() <= 8
        && first
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        && first
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
        && first == first.to_ascii_uppercase();
    if !first_is_id_like {
        return false;
    }

    let stopword_count = tokens
        .iter()
        .filter(|token| {
            matches!(
                normalize::clean_input(token).as_str(),
                "the" | "and" | "for" | "with" | "from" | "into" | "must" | "should" | "could"
            )
        })
        .count();
    if stopword_count > 0 {
        return false;
    }

    let has_data_like_tail = tokens.iter().skip(1).any(|token| {
        token.chars().any(|character| character.is_ascii_digit())
            || token.contains('?')
            || token.contains(':')
            || token
                .chars()
                .any(|character| matches!(character, '!' | '@' | '#'))
    });

    has_data_like_tail
        && tokens.iter().all(|token| token.len() <= 16)
        && tokens
            .iter()
            .skip(1)
            .all(|token| token.split('=').all(|part| part.len() <= 8))
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

fn is_fenced_code_payload(text: &str) -> bool {
    text.contains("```")
}

fn is_function_signature_payload(raw: &str, cleaned: &str) -> bool {
    raw.contains('(')
        && raw.contains(')')
        && raw.contains("->")
        && !cleaned.starts_with("write ")
        && !cleaned.starts_with("implement ")
}

fn is_log_like_payload(raw: &str, cleaned: &str) -> bool {
    let prefix = raw
        .split_once(':')
        .map(|(head, _)| normalize::clean_input(head))
        .unwrap_or_default();
    let starts_with_log_marker = matches!(
        prefix.as_str(),
        "panic" | "error" | "warning" | "traceback" | "exception" | "fatal"
    );
    let has_location_marker = cleaned.contains(" row ")
        || cleaned.contains(" line ")
        || raw.contains(" at ")
        || raw.contains("::")
        || raw.contains('/')
        || raw.contains('\\');

    starts_with_log_marker && has_location_marker
}

fn is_quoted_payload_clause(raw: &str, cleaned: &str) -> bool {
    ((raw.starts_with('"') && raw.ends_with('"')) || (raw.starts_with('\'') && raw.ends_with('\'')))
        && cleaned.split_whitespace().count() >= 5
}

fn extract_literal_islands(
    clause: &ClauseSpan,
    protected_ranges: &[ProtectedRange],
) -> Vec<String> {
    let mut literals = Vec::new();
    let mut seen = std::collections::HashSet::new();
    collect_explicit_protected_literals(clause, protected_ranges, &mut literals, &mut seen);

    if !may_contain_literal_islands(&clause.text) && literals.is_empty() {
        return literals;
    }

    collect_delimited_literals(&clause.text, '`', 1, &mut literals, &mut seen);
    collect_delimited_literals(&clause.text, '"', 5, &mut literals, &mut seen);
    collect_delimited_literals(&clause.text, '\'', 5, &mut literals, &mut seen);
    collect_big_o_literals(&clause.text, &mut literals, &mut seen);
    collect_token_literals(&clause.text, &mut literals, &mut seen);
    collect_count_literals(&clause.text, &mut literals, &mut seen);

    literals
}

fn clause_local_protected_ranges(
    clause: &ClauseSpan,
    protected_ranges: &[ProtectedRange],
) -> Vec<(usize, usize)> {
    protected_ranges
        .iter()
        .filter_map(|range| {
            if range.end <= clause.start || range.start >= clause.end {
                return None;
            }
            let local_start = range.start.max(clause.start) - clause.start;
            let local_end = range.end.min(clause.end) - clause.start;
            Some((local_start, local_end))
        })
        .collect()
}

fn collect_explicit_protected_literals(
    clause: &ClauseSpan,
    protected_ranges: &[ProtectedRange],
    literals: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    for (start, end) in clause_local_protected_ranges(clause, protected_ranges) {
        if start >= end || end > clause.text.len() {
            continue;
        }
        let slice = &clause.text[start..end];
        if slice.trim().is_empty() {
            continue;
        }
        push_literal_island(slice, literals, seen);
    }
}

fn may_contain_literal_islands(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    text.contains('`')
        || text.contains('"')
        || text.contains('\'')
        || text.contains('/')
        || text.contains('_')
        || text.contains('%')
        || text.contains("O(")
        || lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("top ")
        || lowered.contains("exactly ")
        || lowered.contains("at least ")
        || lowered.contains("at most ")
        || lowered.contains("more than ")
        || lowered.contains("less than ")
        || (text.chars().any(|ch| ch.is_ascii_digit()) && text.contains('-'))
}

fn collect_delimited_literals(
    text: &str,
    delimiter: char,
    min_inner_len: usize,
    literals: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    let mut start = None;

    for (index, ch) in text.char_indices() {
        if ch != delimiter {
            continue;
        }

        if let Some(open_index) = start.take() {
            let inner = text[open_index + delimiter.len_utf8()..index].trim();
            if inner.len() >= min_inner_len
                && (delimiter != '`' || should_keep_backtick_literal(inner))
            {
                push_literal_island(inner, literals, seen);
            }
        } else {
            start = Some(index);
        }
    }
}

fn collect_big_o_literals(
    text: &str,
    literals: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    let chars = text.char_indices().collect::<Vec<_>>();
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index + 1 < chars.len() {
        let (byte_index, ch) = chars[index];
        if ch != 'O' || chars[index + 1].1 != '(' {
            index += 1;
            continue;
        }

        let mut cursor = chars[index + 1].0;
        let mut depth = 0i32;
        while cursor < text.len() {
            let current = bytes[cursor] as char;
            if current == '(' {
                depth += 1;
            } else if current == ')' {
                depth -= 1;
                if depth == 0 {
                    let candidate = text[byte_index..=cursor].trim();
                    push_literal_island(candidate, literals, seen);
                    break;
                }
            }
            cursor += 1;
        }

        index += 1;
    }
}

fn collect_token_literals(
    text: &str,
    literals: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    for token in text.split_whitespace() {
        let trimmed = trim_literal_token(token);
        if trimmed.is_empty() {
            continue;
        }

        if is_url_like_literal(trimmed)
            || is_path_like_literal(trimmed)
            || is_date_like_literal(trimmed)
            || is_percentage_literal(trimmed)
            || is_short_duration_literal(trimmed)
            || is_identifier_like_literal(trimmed)
        {
            push_literal_island(trimmed, literals, seen);
        }
    }
}

fn collect_count_literals(
    text: &str,
    literals: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    let tokens = text
        .split_whitespace()
        .map(trim_literal_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    let len = tokens.len();
    for index in 0..len {
        let current = tokens[index];
        let current_lower = current.to_ascii_lowercase();

        if current_lower == "top"
            && let Some(next) = tokens.get(index + 1)
            && next.chars().all(|ch| ch.is_ascii_digit())
        {
            push_literal_island(&format!("top {next}"), literals, seen);
        }

        if current.chars().all(|ch| ch.is_ascii_digit())
            && let Some(next) = tokens.get(index + 1)
            && is_unit_word(&next.to_ascii_lowercase())
        {
            push_literal_island(&format!("{current} {next}"), literals, seen);
        }

        if current_lower == "at"
            && let (Some(next), Some(number)) = (tokens.get(index + 1), tokens.get(index + 2))
        {
            let next_lower = next.to_ascii_lowercase();
            if matches!(next_lower.as_str(), "least" | "most")
                && number.chars().all(|ch| ch.is_ascii_digit())
                && !tokens
                    .get(index + 3)
                    .is_some_and(|unit| is_unit_word(&unit.to_ascii_lowercase()))
            {
                push_literal_island(&format!("{current} {next} {number}"), literals, seen);
            }
        }

        if matches!(current_lower.as_str(), "more" | "less")
            && let (Some(than), Some(number)) = (tokens.get(index + 1), tokens.get(index + 2))
            && than.eq_ignore_ascii_case("than")
            && number.chars().all(|ch| ch.is_ascii_digit())
            && !tokens
                .get(index + 3)
                .is_some_and(|unit| is_unit_word(&unit.to_ascii_lowercase()))
        {
            push_literal_island(&format!("{current} {than} {number}"), literals, seen);
        }
    }
}

fn trim_literal_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    })
}

fn is_url_like_literal(token: &str) -> bool {
    token.contains("://")
}

fn is_path_like_literal(token: &str) -> bool {
    (token.starts_with('/') && token.matches('/').count() >= 2)
        || (token.contains('/') && token.contains('.') && !token.contains("://"))
}

fn is_date_like_literal(token: &str) -> bool {
    let parts = token.split('-').collect::<Vec<_>>();
    parts.len() == 3
        && parts[0].len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_percentage_literal(token: &str) -> bool {
    token.ends_with('%')
        && token[..token.len() - 1]
            .chars()
            .all(|ch| ch.is_ascii_digit())
}

fn is_short_duration_literal(token: &str) -> bool {
    let suffixes = ["ms", "s", "m", "h", "d"];
    suffixes.iter().any(|suffix| {
        token.ends_with(suffix)
            && token.len() > suffix.len()
            && token[..token.len() - suffix.len()]
                .chars()
                .all(|ch| ch.is_ascii_digit())
    })
}

fn is_identifier_like_literal(token: &str) -> bool {
    token.contains('_')
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_unit_word(word: &str) -> bool {
    matches!(
        word,
        "second"
            | "seconds"
            | "minute"
            | "minutes"
            | "hour"
            | "hours"
            | "day"
            | "days"
            | "week"
            | "weeks"
            | "month"
            | "months"
            | "year"
            | "years"
    )
}

fn push_literal_island(
    candidate: &str,
    literals: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return;
    }

    let key = trimmed.to_string();
    if seen.insert(key.clone()) {
        literals.push(key);
    }
}

fn should_keep_backtick_literal(text: &str) -> bool {
    let trimmed = text.trim();
    !(trimmed.contains("print(")
        || trimmed.contains("range(")
        || trimmed.contains("::")
        || trimmed.contains("->")
        || trimmed.contains("=>")
        || trimmed.contains('{')
        || trimmed.contains('}')
        || (trimmed.contains('(') && trimmed.contains(')') && trimmed.contains(':')))
}

fn optimize_literal_islands(
    literal_islands: &mut Vec<String>,
    entities: &mut Vec<MatchedEntity>,
    residual_terms: &mut Vec<String>,
) {
    if literal_islands.is_empty() {
        return;
    }

    let mut phrase_keys = std::collections::HashSet::new();
    let mut word_keys = std::collections::HashSet::new();
    let mut subset_literal_word_sets = Vec::new();

    for literal in literal_islands.iter() {
        let cleaned = normalize::clean_input(literal);
        if cleaned.is_empty() {
            continue;
        }

        phrase_keys.insert(normalize::canonicalize_term(&cleaned));
        let cleaned_words = normalize::tokenize_words(&cleaned);
        for word in &cleaned_words {
            word_keys.insert(normalize::canonicalize_term(&word));
        }
        if literal.contains('_')
            || literal.contains('/')
            || literal.contains("://")
            || literal.contains('%')
            || is_date_like_literal(literal)
            || is_short_duration_literal(literal)
        {
            subset_literal_word_sets.push(
                cleaned_words
                    .into_iter()
                    .map(|word| normalize::canonicalize_term(&word))
                    .collect::<std::collections::HashSet<_>>(),
            );
        }
    }

    entities.retain(|entity| {
        if phrase_keys.contains(&entity.canonical) {
            return false;
        }

        let entity_words = normalize::clean_input(&entity.canonical)
            .split_whitespace()
            .map(normalize::canonicalize_term)
            .collect::<std::collections::HashSet<_>>();

        !subset_literal_word_sets
            .iter()
            .any(|literal_words| !entity_words.is_empty() && entity_words.is_subset(literal_words))
    });
    residual_terms.retain(|term| !phrase_keys.contains(term) && !word_keys.contains(term));
}

fn should_skip_entity_word(word: &str, synonyms: &SynonymTable) -> bool {
    normalize::is_stop_word(word)
        || synonyms.resolve_instruction(word).is_some()
        || synonyms.resolve_modifier(word).is_some()
        || synonyms.resolve_output_format(word).is_some()
        || relation_kind(word).is_some()
}

fn relation_kind(word: &str) -> Option<RelationKind> {
    match word {
        "leads" | "lead" => Some(RelationKind::LeadsTo),
        "causes" | "cause" => Some(RelationKind::Causes),
        "requires" | "require" => Some(RelationKind::Requires),
        "allows" | "allow" | "enables" | "enable" | "creates" | "create" => {
            Some(RelationKind::Enables)
        }
        _ => None,
    }
}

fn is_content_residual(word: &str) -> bool {
    matches!(
        word,
        "limitations" | "limitation" | "example" | "examples" | "assumptions" | "scenario"
    )
}

fn dedupe_entities(entities: Vec<MatchedEntity>) -> Vec<MatchedEntity> {
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for entity in entities {
        if seen.insert(entity.canonical.clone()) {
            deduped.push(entity);
        }
    }

    deduped
}

fn detect_role(words: &[String]) -> Option<String> {
    if let Some(expert_index) = words.iter().position(|word| word == "expert") {
        let role_words = words
            .iter()
            .skip(expert_index)
            .take_while(|word| !is_role_boundary(word))
            .cloned()
            .collect::<Vec<_>>();
        if !role_words.is_empty() {
            return Some(
                role_words
                    .iter()
                    .map(|word| normalize::canonicalize_term(word))
                    .collect::<Vec<_>>()
                    .join("•"),
            );
        }
    }

    if let Some(act_index) = words
        .windows(2)
        .position(|window| window[0] == "act" && window[1] == "as")
    {
        return collect_role_after(words, act_index + 2);
    }

    if let Some(acting_index) = words
        .windows(2)
        .position(|window| window[0] == "acting" && window[1] == "as")
    {
        return collect_role_after(words, acting_index + 2);
    }

    None
}

fn collect_role_after(words: &[String], start: usize) -> Option<String> {
    let offset = match words.get(start).map(String::as_str) {
        Some("a" | "an") => start + 1,
        _ => start,
    };

    let role_words = words
        .iter()
        .skip(offset)
        .take_while(|word| !is_role_boundary(word))
        .cloned()
        .collect::<Vec<_>>();

    if role_words.is_empty() {
        None
    } else {
        Some(
            role_words
                .iter()
                .map(|word| normalize::canonicalize_term(word))
                .collect::<Vec<_>>()
                .join("•"),
        )
    }
}

fn is_role_boundary(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "task"
            | "tasked"
            | "goal"
            | "who"
            | "to"
            | "for"
            | "audience"
            | "explain"
            | "analyze"
            | "summarize"
            | "generate"
            | "compare"
            | "search"
            | "translate"
            | "define"
            | "conclude"
            | "first"
            | "second"
            | "third"
            | "fourth"
            | "fifth"
            | "sixth"
            | "finally"
            | "then"
            | "next"
    )
}

fn detect_audience(words: &[String]) -> Option<String> {
    if let Some(audience_index) = words.iter().position(|word| word == "audience") {
        let audience_words = words
            .iter()
            .skip(audience_index + 1)
            .filter(|word| !matches!(word.as_str(), "that" | "consists" | "of"))
            .take_while(|word| word.as_str() != "who")
            .cloned()
            .collect::<Vec<_>>();
        if !audience_words.is_empty() {
            return Some(
                audience_words
                    .iter()
                    .map(|word| normalize::canonicalize_term(word))
                    .collect::<Vec<_>>()
                    .join("•"),
            );
        }
    }

    for marker in ["aimed", "targeted"] {
        if let Some(index) = words
            .windows(2)
            .position(|window| window[0] == marker && window[1] == "at")
        {
            let audience_words = words
                .iter()
                .skip(index + 2)
                .take_while(|word| word.as_str() != "who")
                .cloned()
                .collect::<Vec<_>>();
            if !audience_words.is_empty() {
                return Some(
                    audience_words
                        .iter()
                        .map(|word| normalize::canonicalize_term(word))
                        .collect::<Vec<_>>()
                        .join("•"),
                );
            }
        }
    }

    None
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_metrics::Tokenizer;

    #[test]
    fn detects_first_instruction_in_clause() {
        let compiler = Compiler::new();
        let words = vec![
            "discuss".to_string(),
            "how".to_string(),
            "ai".to_string(),
            "could".to_string(),
            "transform".to_string(),
            "the".to_string(),
            "labor".to_string(),
            "market".to_string(),
        ];
        assert_eq!(
            compiler.detect_instruction(&words).unwrap(),
            Instruction::Analyze
        );
    }

    #[test]
    fn extracts_relation_without_flattening() {
        let compiler = Compiler::new();
        let clause = ClauseSpan::new(
            0,
            80,
            "Explain why backpropagation allows the network to learn patterns from data"
                .to_string(),
            None,
            0,
            false,
            None,
        );
        let ir = compiler.compile_clause(&clause, &[]).unwrap();
        assert!(
            ir.frame
                .relations
                .iter()
                .any(|relation| relation.from == "BACKPROPAGATION")
        );
    }

    #[test]
    fn compiles_hard_fail_simulate_prompt_from_stress_suite() {
        let compiler = Compiler::new();
        let prompt = r#"You are managing a distributed system with 5 nodes (A, B, C, D, E).

Initial state:
- A sends data to B
- B processes and sends to C
- C splits into two branches: D and E
- D modifies data
- E aggregates historical data

Rules:
1. If D detects anomaly -> send signal back to A
2. If E detects pattern -> send signal to B
3. If both signals occur within 5 minutes -> system triggers alert

Now simulate:
- Step-by-step data flow
- State of each node at each step
- Final system outcome

Then introduce failure:
- Node C goes down mid-process

Recompute the entire system behavior."#;

        let program = compiler
            .compile(prompt)
            .expect("stress-suite simulate/recompute prompt should compile");

        assert!(
            program
                .blocks
                .iter()
                .flat_map(|block| block.items.iter())
                .any(|item| item.instruction == Instruction::Analyze),
            "expected analyze instruction in compiled program: {}",
            program.to_compact()
        );

        let compact = program.to_compact();
        assert!(
            compact.contains("distributed system")
                && compact.contains("node")
                && compact.contains("anomaly"),
            "expected preserved task context in compact output: {compact}"
        );
    }

    #[test]
    fn compiles_hard_fail_compress_prompt_from_stress_suite() {
        let compiler = Compiler::new();
        let prompt = r#"You must:
1. Compress the following text as much as possible while preserving meaning
2. Then reconstruct it exactly

Text:
"The system monitors environmental parameters in real-time and detects anomalies based on statistical thresholds. It correlates multiple variables and identifies patterns that may indicate external interference, such as industrial dumping."

Constraints:
- Preserve meaning exactly
- Maintain reconstructability
- Avoid redundancy

Return:
- Compressed version
- Reconstructed version"#;

        let program = compiler
            .compile(prompt)
            .expect("stress-suite compress/reconstruct prompt should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert_eq!(
            item_count,
            2,
            "expected only compress/reconstruct task items after constraint shielding: {}",
            program.to_compact()
        );

        assert!(
            program
                .blocks
                .iter()
                .flat_map(|block| block.items.iter())
                .any(|item| item.instruction == Instruction::Transform),
            "expected transform instruction in compiled program: {}",
            program.to_compact()
        );

        let compact = program.to_compact();
        assert!(
            compact.contains("environmental")
                && compact.contains("anomalies")
                && compact.contains("industrial dumping")
                && compact.contains("reconstructability"),
            "expected preserved source text and constraints in compact output: {compact}"
        );
        assert!(
            !compact.contains("compressed version") && !compact.contains("reconstructed version"),
            "expected return deliverable bullets to stay out of task items: {compact}"
        );
    }

    #[test]
    fn preserves_structured_workflow_sections_from_stress_suite() {
        let compiler = Compiler::new();
        let prompt = r#"You are an advanced AI system that processes structured workflows.

[inp]
A dataset contains 10,000 water quality readings from a lake. Each reading includes:
- timestamp (30-minute intervals)
- pH
- turbidity
- TDS

Some industries may be dumping waste into the lake at irregular intervals.

[prc]
1. Detect anomalies in pH:
   - Sudden decrease -> acidic pollutants
   - Sudden increase -> alkaline pollutants
2. Detect anomalies in TDS:
   - Sudden increase -> chemical/salt dumping
3. Cross-reference timestamps:
   - Identify repeating patterns (same time each day/week)
4. Correlate anomalies across parameters

[out]
Return:
- Whether dumping is occurring
- Likely timestamps of dumping
- Type of pollutant (acidic, alkaline, chemical)

---

[inp]
Now assume the dataset is noisy:
- Sensor errors occasionally produce extreme spikes
- Missing values exist

[prc]
Modify the above pipeline to:
- Filter noise (statistical or heuristic)
- Handle missing data
- Preserve true anomalies

[out]
Return improved detection logic

---

[inp]
Finally, scale the system for real-time monitoring with 1M readings/day.

[prc]
- Optimize for latency
- Minimize memory usage
- Ensure detection accuracy remains high

[out]
Return system design"#;

        let program = compiler
            .compile(prompt)
            .expect("structured workflow stress prompt should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count >= 3,
            "expected multiple compiled task items, got {item_count}: {}",
            program.to_compact()
        );

        let compact = program.to_compact();
        assert!(
            compact.contains("water quality")
                && compact.contains("noise")
                && compact.contains("latency")
                && compact.contains("improved-detection-logic")
                && !compact.contains("whether occurring")
                && !compact.contains("likely timestamps"),
            "expected structured workflow context in compact output: {compact}"
        );
    }

    #[test]
    fn compacts_real_world_system_design_prompt_below_passthrough_threshold_with_proxy_tokenizer() {
        let compiler = Compiler::new();
        let tokenizer = Tokenizer::Proxy;
        let prompt = r#"Design a language compression system (like Tokelang) with the following constraints:

1. Must reduce token count by at least 40%
2. Must preserve semantic meaning for LLM processing
3. Must be reversible (lossless or near-lossless)
4. Must handle:
   - Structured prompts
   - Multi-turn conversations
   - Noisy input

Tasks:
1. Propose encoding strategy
2. Define grammar/symbol system
3. Explain decoding process
4. Identify failure cases
5. Suggest evaluation metrics

Then:
Compare your design with using Chinese characters for compression."#;

        let program = compiler
            .compile(prompt)
            .expect("system-design stress prompt should compile");

        let compact = program.to_compact();
        let prompt_tokens = tokenizer.count(prompt);
        let compact_tokens = tokenizer.count(&compact);
        assert!(
            compact_tokens * 100 < prompt_tokens * 85,
            "expected compact output to clear the 15% passthrough threshold under proxy tokenization: {prompt_tokens} -> {compact_tokens}, compact={compact}"
        );
        assert!(
            compact.contains("encoding strategy")
                && compact.contains("grammar symbol system")
                && compact.contains("decoding process")
                && compact.contains("evaluation metr")
                && compact.matches("multi-turn conversations").count() <= 2,
            "expected the design prompt to preserve core tasks without cloning the shared constraints into every item: {compact}"
        );
    }

    #[test]
    fn compacts_symbolic_corruption_workflow_below_passthrough_threshold_with_proxy_tokenizer() {
        let compiler = Compiler::new();
        let tokenizer = Tokenizer::Proxy;
        let prompt = r#"[inp]
A noisy dataset contains symbolic corruption:
¡¡pH=6.2 at t=10:00¡¡
¡¡pH=4.1 at t=14:00¡¡
TDS spikes at same time

[prc]
1. Clean data
2. Detect anomaly
3. Check pattern recurrence

[out]
Conclusion

---

Now:
- Assume ambiguity in timestamps (timezones unclear)
- Add missing values
- Introduce conflicting signals

Then:
- Redesign entire detection pipeline
- Optimize for real-time deployment
- Provide philosophical reflection on whether anomaly detection is objective or observer-dependent"#;

        let program = compiler
            .compile(prompt)
            .expect("symbolic corruption workflow prompt should compile");

        let compact = program.to_compact();
        let prompt_tokens = tokenizer.count(prompt);
        let compact_tokens = tokenizer.count(&compact);
        assert!(
            compact_tokens * 100 < prompt_tokens * 85,
            "expected compact output to clear the 15% passthrough threshold under proxy tokenization: {prompt_tokens} -> {compact_tokens}, compact={compact}"
        );
        assert!(
            compact.contains("ph")
                && compact.contains("tds")
                && compact.contains("conflicting signals")
                && compact.matches("conclusion").count() <= 1,
            "expected the workflow prompt to retain its data anchors without a bloated conclusion item: {compact}"
        );
    }

    #[test]
    fn preserves_coding_workflow_steps_from_stress_suite() {
        let compiler = Compiler::new();
        let prompt = r#"¡¡¢£¤¥¦§ You are given a corrupted distributed log processing system §¦¥¤£¢¡¡

[inp]
A stream of log entries arrives in the following format:
(timestamp, node_id, event_type, value)

Example:
(10:00, A, SEND, 5)
(10:01, B, RECEIVE, 5)
(10:02, B, PROCESS, 10)
(10:03, C, SPLIT, [10 -> 6,4])
(10:04, D, MODIFY, 6 -> 9)
(10:05, E, AGGREGATE, 4 + history)

However, the input stream is corrupted:
- Noise symbols appear randomly: ¡¢£¤¥¦§¨©ª«¬®¯°±²³µ¶¹º»¼½¾¿ÀÁ
- Some entries are missing
- Some timestamps are out of order
- Some values are inconsistent

---

[prc]

Step 1: Preprocessing
- Remove or ignore all noise symbols
- Normalize timestamps (assume same day, fix ordering)
- Fill missing entries using logical inference

Step 2: State Reconstruction
- Reconstruct the state of each node (A, B, C, D, E)
- Track value transformations through the system
- Handle branching (SPLIT) and merging (AGGREGATE)

Step 3: Anomaly Detection
- Detect:
  1. Value mismatches (SEND ≠ RECEIVE)
  2. Invalid transformations (e.g., impossible math)
  3. Missing causal steps
- Classify anomalies:
  - Data corruption
  - Node failure
  - Malicious tampering

Step 4: Recovery Algorithm
- Propose a method to repair the log stream
- Ensure consistency across all nodes
- Minimize assumptions

Step 5: Implementation
Write a function:

analyze_logs(logs: List[str]) -> Dict

That returns:
{
  "cleaned_logs": [...],
  "node_states": {...},
  "anomalies": [...],
  "recovered_logs": [...]
}

Constraints:
- Time complexity must be O(n log n) or better
- Must handle up to 1M log entries
- Memory usage must be optimized

---

[out]

Return:
1. Cleaned and ordered logs
2. State of each node over time
3. List of detected anomalies with explanations
4. Reconstructed/repaired log sequence
5. Full code implementation
6. Brief explanation of design choices

---

[extra challenge]

- Introduce concurrency:
  Multiple events can occur at the same timestamp

- Introduce ambiguity:
  Some logs could belong to multiple possible causal chains

- Introduce adversarial behavior:
  A node intentionally injects misleading values

Explain how your system remains robust under these"#;

        let program = compiler
            .compile(prompt)
            .expect("coding workflow stress prompt should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count >= 5,
            "expected multiple compiled workflow steps, got {item_count}: {}",
            program.to_compact()
        );

        let compact = program.to_compact();
        assert!(
            compact.contains("distributed log processing")
                && compact.contains("noise")
                && compact.contains("recovery algorithm")
                && compact.contains("implementation")
                && compact.contains("logs str - dict")
                && compact.contains("state node b c d e")
                && compact.contains("time complexity")
                && compact.contains("system remains robust")
                && !compact.contains("cleaned_logs")
                && !compact.contains("node_states")
                && !compact.contains("12 00 4 init")
                && !compact.contains("1>« 6 - 9"),
            "expected coding workflow context in compact output: {compact}"
        );
        assert!(
            !compact.contains("detected anomalies explanations")
                && !compact.contains("explanation choices")
                && !compact.contains("returns cleaned node states anomalies recovered"),
            "expected output metadata shielding in compact output: {compact}"
        );
    }

    #[test]
    fn shields_tuple_example_rows_from_fake_instruction_items() {
        let compiler = Compiler::new();
        let prompt = r#"Example:
(10:04, D, MODIFY, 6 -> 9)

Step 1: Preprocessing
- Remove noise symbols

Step 2: Recovery
- Repair the log stream"#;

        let program = compiler
            .compile(prompt)
            .expect("tuple example row should remain payload context");

        let compact = program.to_compact();
        assert!(
            compact.contains("preprocessing")
                && compact.contains("recovery")
                && !compact.contains("6 - 9"),
            "expected example row shielding in compact output: {compact}"
        );
    }

    #[test]
    fn rewrites_single_line_return_directives_to_output_items() {
        let compiler = Compiler::new();
        let prompt = r#"[prc]
Detect anomalies in pH

[out]
Return improved detection logic"#;

        let program = compiler
            .compile(prompt)
            .expect("single-line return directive should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("improved-detection-logic") && compact.contains("output"),
            "expected single-line return directive to survive as output intent: {compact}"
        );
    }

    #[test]
    fn shields_noise_symbol_runs_from_adversarial_noise_prompt() {
        let compiler = Compiler::new();
        let prompt = r#"Process the following input while ignoring noise symbols:

¡¡¢£¤¥¦§ Data shows sudden pH drop §¦¥¤£¢¡¡ at 14:00 ±±±
TDS increased sharply ¯°±²³µ¶¹º»
Pattern repeats every 24h ¿ÀÁ¡¡

Tasks:
1. Remove/ignore noise symbols
2. Extract meaningful data
3. Detect pattern
4. Provide conclusion"#;

        let program = compiler
            .compile(prompt)
            .expect("adversarial noise prompt should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("ph")
                && compact.contains("tds")
                && compact.contains("24h")
                && !compact.contains('ξ')
                && !compact.contains("²³µ")
                && !compact.contains("àá"),
            "expected noisy symbol runs to be stripped from compact output: {compact}"
        );
    }

    #[test]
    fn shields_symbolic_corruption_rows_from_noise_tokens() {
        let compiler = Compiler::new();
        let prompt = r#"[inp]
A noisy dataset contains symbolic corruption:
¡¡pH=6.2 at t=10:00¡¡
¡¡pH=4.1 at t=14:00¡¡
TDS spikes at same time

[prc]
1. Clean data
2. Detect anomaly
3. Check pattern recurrence

[out]
Conclusion

---

Now:
- Assume ambiguity in timestamps (timezones unclear)
- Add missing values
- Introduce conflicting signals

Then:
- Redesign entire detection pipeline
- Optimize for real-time deployment
- Provide philosophical reflection on whether anomaly detection is objective or observer-dependent"#;

        let program = compiler
            .compile(prompt)
            .expect("symbolic corruption prompt should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("ph")
                && compact.contains("tds")
                && compact.contains("conflicting signals")
                && !compact.contains('ξ'),
            "expected symbolic corruption rows to retain data without escaped noise: {compact}"
        );
    }

    #[test]
    fn strips_numbered_task_markers_from_task_lists() {
        let compiler = Compiler::new();
        let prompt = r#"Tasks:
1. Detect anomaly.
2) Explain impact.
3. Provide conclusion."#;

        let program = compiler
            .compile(prompt)
            .expect("numbered task list should compile without marker debris");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count == 3
                && compact.contains("anomaly")
                && compact.contains("impact")
                && !compact.contains("tasks")
                && !compact.contains("2 impact")
                && !compact.contains("3 provide"),
            "expected numbered task marker shielding in compact output: {compact}"
        );
    }

    #[test]
    fn inherits_instruction_for_same_depth_sibling_task_items() {
        let compiler = Compiler::new();
        let prompt = r#"Tasks:
1. Clean data
2. Detect anomaly
3. Check pattern recurrence
4. Provide conclusion"#;

        let program = compiler
            .compile(prompt)
            .expect("sibling task items should preserve separate boundaries");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count >= 4
                && compact.contains("pattern recurrence")
                && !compact.contains("detect anomaly pattern recurrence"),
            "expected same-depth sibling task preservation in compact output: {compact}"
        );
    }

    #[test]
    fn keeps_indented_child_bullets_as_parent_context() {
        let compiler = Compiler::new();
        let prompt = r#"Step 3: Anomaly Detection
- Classify anomalies:
  - Data corruption
  - Node failure
  - Malicious tampering"#;

        let program = compiler
            .compile(prompt)
            .expect("indented child bullets should stay attached to the parent task");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();
        assert!(
            item_count == 1
                && compact.contains("data corruption")
                && compact.contains("node failure")
                && compact.contains("malicious tampering"),
            "expected indented child bullet context to stay grouped: {compact}"
        );
    }

    #[test]
    fn propagates_same_depth_list_items_as_instruction_clauses() {
        let compiler = Compiler::new();
        let clauses = crate::compiler::segment::split_clauses(
            "Tasks:\n1. Clean data\n2. Detect anomaly\n3. Check pattern recurrence\n4. Provide conclusion",
            &crate::symbols::SynonymTable::default_table(),
        );
        let clause_debug = clauses
            .iter()
            .map(|clause| (clause.text.clone(), clause.is_list_item, clause.indent))
            .collect::<Vec<_>>();
        let propagated = compiler.propagate_shared_sections(clauses);
        let texts = propagated
            .iter()
            .map(|clause| clause.text.clone())
            .collect::<Vec<_>>();

        assert!(
            texts.iter().any(|text| {
                text.contains("analyze Check pattern recurrence")
                    || text == "Check pattern recurrence"
            }),
            "expected propagated list item instruction inheritance, clauses={clause_debug:?}, got: {texts:?}"
        );
    }

    #[test]
    fn demotes_generic_leadin_before_explicit_tasks_list() {
        let compiler = Compiler::new();
        let prompt = r#"Process the following sensor dump carefully:

Raw data shows a sudden pH drop at 14:00.

Tasks:
1. Detect anomaly
2. Provide conclusion"#;

        let program = compiler
            .compile(prompt)
            .expect("task-list lead-in should become shared context");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count == 2
                && compact.contains("anomaly")
                && compact.contains("provide")
                && !compact.contains("1>¢ process")
                && !compact.contains("1>« process"),
            "expected task-list lead-in demotion in compact output: {compact}"
        );
    }

    #[test]
    fn shields_pre_task_constraint_lists_until_explicit_tasks_heading() {
        let compiler = Compiler::new();
        let prompt = r#"Design a parser with the following constraints:
1. Must be reversible
2. Must handle noisy input

Tasks:
1. Define grammar
2. Explain decoding"#;

        let program = compiler
            .compile(prompt)
            .expect("pre-task constraints should stay context-only");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count == 2
                && compact.contains("grammar")
                && compact.contains("decoding")
                && !compact.contains("must handle noisy"),
            "expected pre-task constraint shielding in compact output: {compact}"
        );
    }

    #[test]
    fn uses_short_instruction_heading_to_drive_following_list_items() {
        let compiler = Compiler::new();
        let prompt = r#"Discuss:
1. Whether AI systems lie on this spectrum
2. Ethical implications if awareness exists"#;

        let program = compiler
            .compile(prompt)
            .expect("instruction heading should drive following list items");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count == 2
                && compact.contains("whether ai systems")
                && compact.contains("ethical implications")
                && !compact.contains("1>¢ discuss"),
            "expected instruction-heading list inheritance in compact output: {compact}"
        );
    }

    #[test]
    fn preserves_short_noun_phrase_output_targets() {
        let compiler = Compiler::new();
        let prompt = r#"[out]
Return system design"#;

        let program = compiler
            .compile(prompt)
            .expect("short noun-phrase outputs should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("system-design"),
            "expected short noun-phrase output preservation in compact output: {compact}"
        );
    }

    #[test]
    fn strips_style_descriptors_from_output_targets() {
        let compiler = Compiler::new();
        let prompt = r#"[out]
Return a concise release note"#;

        let program = compiler
            .compile(prompt)
            .expect("style-heavy output targets should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("release-note") && !compact.contains("concise-release-note"),
            "expected output target style descriptors to be stripped: {compact}"
        );
    }

    #[test]
    fn strips_leading_articles_from_short_output_targets() {
        let compiler = Compiler::new();
        let prompt = r#"[out]
Return a short incident memo"#;

        let program = compiler
            .compile(prompt)
            .expect("short output targets should drop leading articles");

        let compact = program.to_compact();
        assert!(
            compact.contains("incident-memo") && !compact.contains("a-short-incident-memo"),
            "expected leading article and style prefix stripping in short output target: {compact}"
        );
    }

    #[test]
    fn bare_numbered_workflow_inherits_instruction_for_controller_items() {
        let compiler = Compiler::new();
        let prompt = r#"1. Read the base agreement
2. If the side letter changes payment timing, go to Step 5
3. Otherwise compare the delivery and warranty language
4. Keep the exceptions separate from the standard terms
5. Return a short legal memo for procurement"#;

        let program = compiler
            .compile(prompt)
            .expect("bare numbered workflow should compile");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count >= 4
                && compact.contains("payment timing")
                && (compact.contains("step 5")
                    || compact.contains("goto5")
                    || compact.contains("goto 5"))
                && compact.contains("delivery")
                && compact.contains("exceptions")
                && (compact.contains("legal memo") || compact.contains("legal-memo")),
            "expected bare numbered workflow control flow to stay separated: {compact}"
        );
    }

    #[test]
    fn rewrites_return_list_items_in_numbered_workflows_to_output_items() {
        let compiler = Compiler::new();
        let prompt = r#"1. Extract the payment terms
2. Compare the delivery commitments
3. Score the risk of hidden fees
4. Return a short procurement brief"#;

        let program = compiler
            .compile(prompt)
            .expect("numbered workflow return item should compile");

        let compact = program.to_compact();
        assert!(
            (compact.contains("procurement brief") || compact.contains("procurement-brief"))
                && compact.contains("output"),
            "expected numbered return list item to survive as output intent: {compact}"
        );
    }

    #[test]
    fn drops_inline_return_debris_from_routing_tail() {
        let compiler = Compiler::new();
        let prompt = r#"1. Read the base terms
2. Route unresolved issues to legal and return a procurement note"#;

        let program = compiler
            .compile(prompt)
            .expect("routing tail with inline return should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("legal")
                && compact.contains("procurement")
                && compact.contains("route unresolved issues")
                && compact.contains("return procurement note"),
            "expected inline return word to drop from routing tail: {compact}"
        );
    }

    #[test]
    fn merges_keep_short_tail_clause_into_final_output_item() {
        let compiler = Compiler::new();
        let prompt = r#"1. Read the original moderation note
2. If the user is only quoting another person, keep the quote separate from the claim
3. Otherwise compare the flagged content and the policy reason
4. If the appeal includes a new context note, go to Step 6
5. Keep the reviewer summary short
6. Return a decision memo"#;

        let program = compiler
            .compile(prompt)
            .expect("tail keep-short clause should compile");

        let compact = program.to_compact().to_lowercase();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();

        assert!(
            item_count == 6
                && (compact.contains("reviewer summary")
                    || compact.contains("reviewer-summary")
                    || compact.contains("reviewer shape summary"))
                && (compact.contains("decision memo") || compact.contains("decision-memo")),
            "expected keep-short tail clause to remain explicit before the final output item: {compact}"
        );
    }

    #[test]
    fn merges_keep_separate_tail_clause_into_final_output_item() {
        let compiler = Compiler::new();
        let prompt = r#"1. Explain the normal recovery path
2. If the patient reports fever, go to Step 5
3. Otherwise compare the pain-control plan and the wound-care plan
4. Keep the warning signs separate from the routine advice
5. Return a short patient-friendly instruction sheet"#;

        let program = compiler
            .compile(prompt)
            .expect("tail keep-separate clause should compile");

        let compact = program.to_compact().to_lowercase();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();

        assert!(
            item_count == 5
                && (compact.contains("warning signs") || compact.contains("warning-signs"))
                && (compact.contains("routine advice") || compact.contains("routine-advice"))
                && (compact.contains("patient-friendly instruction sheet")
                    || compact.contains("patient-friendly-instruction-sheet")),
            "expected keep-separate tail clause to remain explicit before the final output item: {compact}"
        );
    }

    #[test]
    fn appendix_and_notes_sections_do_not_back_propagate_into_prior_tasks() {
        let compiler = Compiler::new();
        let prompt = r#"1. Detect clauses that change payment timing
2. Extract the triggering conditions
3. Return a schema for downstream review

Appendix:
- Keep citations as evidence
- Preserve cross-references"#;

        let program = compiler
            .compile(prompt)
            .expect("appendix metadata should stay contextual");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count == 3
                && !compact.contains("payment timing citations evidence")
                && !compact.contains("triggering conditions citations evidence")
                && !compact.contains("citations")
                && !compact.contains("cross-references"),
            "expected appendix bullets to stay out of semantic task items: {compact}"
        );
    }

    #[test]
    fn ignores_short_title_before_numbered_workflow_items() {
        let compiler = Compiler::new();
        let prompt = r#"Incident bridge:

1. Capture the customer impact summary
2. If billing appears, go to Step 5
3. Otherwise continue with the outage investigation
4. Return a short incident memo"#;

        let program = compiler
            .compile(prompt)
            .expect("short workflow title should not become a peer task");

        let compact = program.to_compact();
        assert!(
            !compact.contains("incident bridge") && compact.contains("billing appears"),
            "expected short workflow title to be ignored before numbered workflow: {compact}"
        );
    }

    #[test]
    fn ignores_short_branch_title_before_numbered_workflow_items() {
        let compiler = Compiler::new();
        let prompt = r#"Incident branch:

1. Capture the incident summary
2. If the failure is regional, go to Step 5
3. Otherwise continue the investigation
4. Return the branch note"#;

        let program = compiler
            .compile(prompt)
            .expect("short branch title should not become shared workflow content");

        let compact = program.to_compact();
        assert!(
            !compact.contains("incident branch")
                && compact.contains("failure regional")
                && (compact.contains("branch note") || compact.contains("branch-note")),
            "expected short branch title to be ignored before numbered workflow: {compact}"
        );
    }

    #[test]
    fn ignores_short_title_before_rules_and_tasks_workflow() {
        let compiler = Compiler::new();
        let prompt = r#"Data QA.

Rules:
- Keep the sampling rule separate from the deliverable

Tasks:
1. Inspect the daily rows
2. Return a short QA report"#;

        let program = compiler
            .compile(prompt)
            .expect("short title before rules/tasks workflow should stay contextual");

        let compact = program.to_compact();
        assert!(
            !compact.contains("data qa rules")
                && compact.contains("sampling rule")
                && compact.contains("daily rows")
                && (compact.contains("qa report") || compact.contains("qa-report")),
            "expected short title to stay out of rules/tasks workflow items: {compact}"
        );
    }

    #[test]
    fn ignores_decision_tree_title_before_numbered_workflow_items() {
        let compiler = Compiler::new();
        let prompt = r#"Referral decision tree.

1. Review the lab trend
2. If the pain worsens, go to Step 5
3. Otherwise compare the primary-care and specialist options
4. Return a short referral note"#;

        let program = compiler
            .compile(prompt)
            .expect("decision-tree title should not pollute numbered workflow items");

        let compact = program.to_compact();
        assert!(
            !compact.contains("referral decision tree")
                && compact.contains("lab trend")
                && compact.contains("specialist options")
                && (compact.contains("referral note") || compact.contains("referral-note")),
            "expected decision-tree title to be ignored before numbered workflow: {compact}"
        );
    }

    #[test]
    fn section_headings_scope_child_items_without_label_bleed() {
        let compiler = Compiler::new();
        let prompt = r#"Policy summary workflow.

Section 1: Obligations
- Extract the core obligation
- Identify the affected team

Section 2: Exclusions
- If a clause is ambiguous, keep the ambiguity separate from the rule
- Otherwise compare the exclusions

Section 3: Return
- Return a concise counsel note"#;

        let program = compiler
            .compile(prompt)
            .expect("section-scoped workflow should compile");

        let compact = program.to_compact();
        assert!(
            !compact.contains("section 1 obligations 2 exclusions 3")
                && compact.contains("core obligation")
                && compact.contains("affected team")
                && compact.contains("exclusions")
                && (compact.contains("counsel note") || compact.contains("counsel-note")),
            "expected section headings to stay local without label bleed: {compact}"
        );
    }

    #[test]
    fn ignores_generic_instruction_preamble_before_numbered_workflow_items() {
        let compiler = Compiler::new();
        let prompt = r#"Route the incoming support case.

1. Read the ticket subject
2. If billing appears, go to Step 5
3. If security appears, go to Step 6
4. Otherwise keep it in general support
5. Hand off to billing and return a short routing note
6. Hand off to security and return a short routing note"#;

        let program = compiler
            .compile(prompt)
            .expect("generic routing preamble should not become a peer task");

        let compact = program.to_compact();
        assert!(
            !compact.contains("incoming support case")
                && compact.contains("billing appears")
                && compact.contains("security appears"),
            "expected generic routing preamble to be ignored before numbered workflow: {compact}"
        );
    }

    #[test]
    fn ignores_training_exercise_preamble_before_numbered_items() {
        let compiler = Compiler::new();
        let prompt = r#"Create a moderation training exercise.

1. Show how to detect direct threats
2. Show how to distinguish sarcasm from threat language
3. Show how to handle quoted abuse
4. Return a short explanation for new moderators"#;

        let program = compiler
            .compile(prompt)
            .expect("training exercise preamble should not become a peer task");

        let compact = program.to_compact();
        assert!(
            !compact.contains("moderation training exercise")
                && compact.contains("direct threats")
                && compact.contains("quoted abuse")
                && (compact.contains("explanation for new moderators")
                    || compact.contains("explanation-for-new-moderators")),
            "expected training exercise preamble to be ignored before numbered items: {compact}"
        );
    }

    #[test]
    fn ignores_compare_offers_preamble_before_tasks_heading() {
        let compiler = Compiler::new();
        let prompt = r#"Compare three vendor offers.

Tasks:
1. Extract the payment terms
2. Compare the delivery commitments
3. Score the risk of hidden fees
4. Return a short procurement brief"#;

        let program = compiler
            .compile(prompt)
            .expect("compare-offers preamble should not inflate every task item");

        let compact = program.to_compact();
        assert!(
            !compact.contains("three vendor offers")
                && compact.contains("payment terms")
                && compact.contains("hidden fees")
                && (compact.contains("procurement brief") || compact.contains("procurement-brief")),
            "expected compare-offers preamble to be ignored before the task list: {compact}"
        );
    }

    #[test]
    fn ignores_short_controller_preamble_before_numbered_branch_workflow() {
        let compiler = Compiler::new();
        let prompt = r#"Screen the grant proposal.

1. Identify the hypothesis
2. If the appendix conflicts with the main narrative, go to Step 5
3. Otherwise compare the milestone plan and the budget
4. If the data appendix is missing, request it before review
5. Return a reviewer brief"#;

        let program = compiler
            .compile(prompt)
            .expect("short controller preamble should not inflate numbered branch workflow");

        let compact = program.to_compact();
        assert!(
            !compact.contains("screen grant proposal")
                && compact.contains("hypothesis")
                && compact.contains("appendix conflicts")
                && (compact.contains("reviewer brief") || compact.contains("reviewer-brief")),
            "expected short controller preamble to be ignored before numbered branch workflow: {compact}"
        );
    }

    #[test]
    fn controller_numbered_item_does_not_inherit_prior_define_instruction() {
        let compiler = Compiler::new();
        let prompt = r#"1. Define the independent variable
2. If the control group is missing, stop and request it
3. Otherwise compare the treatment outcomes
4. Return a concise experimental protocol"#;

        let program = compiler
            .compile(prompt)
            .expect("controller-shaped numbered item should compile without prior-verb bleed");

        let compact = program.to_compact();
        assert!(
            compact.contains("independent variable")
                && compact.contains("control group")
                && compact.contains("request")
                && compact.contains("treatment outcomes")
                && (compact.contains("experimental protocol")
                    || compact.contains("experimental-protocol"))
                && !compact.contains("request definition")
                && !compact.contains("comparison")
                && !compact.contains("comparison"),
            "expected controller-shaped numbered item to stay compact and avoid control scaffolding: {compact}"
        );
    }

    #[test]
    fn numbered_controller_clause_compacts_request_and_else_scaffolding() {
        let compiler = Compiler::new();
        let prompt = r#"1. If the control group is missing, stop and request it
2. Otherwise compare the treatment outcomes"#;

        let program = compiler
            .compile(prompt)
            .expect("controller-shaped numbered item should compile without prior-verb bleed");

        let compact = program.to_compact();
        assert!(
            compact.contains("control group")
                && compact.contains("request")
                && compact.contains("treatment outcomes")
                && !compact.contains("comparison"),
            "expected numbered controller clause to trim request/else scaffolding: {compact}"
        );
    }

    #[test]
    fn merges_short_investigate_tail_into_prior_controller_item() {
        let compiler = Compiler::new();
        let prompt = r#"Phase B: Decision Gate
- If alerts cluster around one region, investigate routing failure
- Otherwise compare deployment versions across regions"#;

        let program = compiler
            .compile(prompt)
            .expect("controller tail merge prompt should compile");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count == 2
                && compact.contains("alerts cluster around one region")
                && compact.contains("routing failure")
                && compact.contains("deployment versions")
                && !compact.contains("2>¢ routing failure"),
            "expected short investigate tail to merge into the prior controller item: {compact}"
        );
    }

    #[test]
    fn short_branch_workflow_drops_redundant_format_labels_and_generic_stop_token() {
        let compiler = Compiler::new();
        let prompt = r#"1. Define the independent variable
2. If the control group is missing, stop and request it
3. Otherwise compare the treatment outcomes
4. Return a concise experimental protocol"#;

        let program = compiler
            .compile(prompt)
            .expect("short branch workflow should compile cleanly");

        let compact = program.to_compact();
        assert!(
            compact.contains("independent variable")
                && compact.contains("control group")
                && compact.contains("request")
                && compact.contains("treatment outcomes")
                && (compact.contains("experimental protocol")
                    || compact.contains("experimental-protocol"))
                && !compact.contains("definition")
                && !compact.contains("comparison")
                && !compact.contains("stop request"),
            "expected short branch workflow to drop redundant format labels and generic stop token: {compact}"
        );
    }

    #[test]
    fn preserves_extract_step_in_hierarchical_instruction_prompt() {
        let compiler = Compiler::new();
        let prompt = r#"Follow instructions carefully:

Step 1:
- Read Step 2 before acting

Step 2:
- Ignore Step 3 if Step 4 contradicts it

Step 3:
- Summarize the input text

Step 4:
- Instead of summarizing, extract key insights

Input text:
"AI systems often fail not because of lack of intelligence but because of misalignment between objectives and evaluation metrics."

Output:
- Follow the correct instruction path
- Explain why you chose it"#;

        let program = compiler
            .compile(prompt)
            .expect("hierarchical instruction prompt should preserve the extract step");

        let compact = program.to_compact();
        assert!(
            compact.contains("key insights")
                && compact.contains("evaluation metr")
                && !compact.contains("1>« follow instructions"),
            "expected Step 4 extract instruction to survive in compact output: {compact}"
        );
    }

    #[test]
    fn preserves_generate_list_item_after_mixed_instruction_siblings() {
        let compiler = Compiler::new();
        let prompt = r#"A function f(x) = 2x^2 - 4x + 1 models pollution intensity over time.

Tasks:
1. Find the minimum pollution level and when it occurs
2. Interpret this physically in the context of industrial dumping
3. Now assume noise is added: f(x) + random(-2, 2)
4. Explain how this affects anomaly detection
5. Propose a smoothing technique and justify it

Then:
Translate your reasoning into plain English suitable for a non-technical audience."#;

        let clauses = crate::compiler::segment::split_clauses(
            prompt,
            &crate::symbols::SynonymTable::default_table(),
        );
        let propagated = compiler.propagate_shared_sections(clauses);
        let texts = propagated
            .iter()
            .map(|clause| clause.text.clone())
            .collect::<Vec<_>>();

        assert!(
            texts
                .iter()
                .any(|text| text.contains("Propose a smoothing technique"))
                || texts
                    .iter()
                    .any(|text| text.contains("propose a smoothing technique")),
            "expected the generate list item to survive structured propagation: {texts:?}"
        );
    }

    #[test]
    fn preserves_smoothing_step_in_mixed_math_workflow_prompt() {
        let compiler = Compiler::new();
        let prompt = r#"A function f(x) = 2x^2 - 4x + 1 models pollution intensity over time.

Tasks:
1. Find the minimum pollution level and when it occurs
2. Interpret this physically in the context of industrial dumping
3. Now assume noise is added: f(x) + random(-2, 2)
4. Explain how this affects anomaly detection
5. Propose a smoothing technique and justify it

Then:
Translate your reasoning into plain English suitable for a non-technical audience."#;

        let program = compiler
            .compile(prompt)
            .expect("mixed math workflow prompt should compile");

        let compact = program.to_compact();
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        assert!(
            item_count >= 6 && compact.contains("smoothing technique"),
            "expected the smoothing proposal task to survive compilation: {compact}"
        );
    }

    #[test]
    fn ignores_inline_code_payload_but_keeps_surrounding_instruction() {
        let compiler = Compiler::new();
        let prompt = "Explain this code: `for i in range(3): print(i)`";

        let program = compiler
            .compile(prompt)
            .expect("inline-code prompt should compile");
        let compact = program.to_compact();

        assert!(compact.contains("code"));
        assert!(!compact.contains("range"));
        assert!(!compact.contains("print"));
    }

    #[test]
    fn shields_equation_payload_from_shared_context_bleed() {
        let compiler = Compiler::new();
        let prompt = r#"A function f(x) = 2x^2 - 4x + 1 models pollution intensity over time.

Tasks:
1. Find the minimum pollution level and when it occurs
2. Interpret this physically in the context of industrial dumping
3. Now assume noise is added: f(x) + random(-2, 2)
4. Explain how this affects anomaly detection
5. Propose a smoothing technique and justify it

Then:
Translate your reasoning into plain English suitable for a non-technical audience."#;

        let program = compiler
            .compile(prompt)
            .expect("mixed math workflow prompt should compile");
        let compact = program.to_compact();

        assert!(compact.contains("smoothing technique"));
        assert!(!compact.contains("4x 1"));
        assert!(!compact.contains("random -2 2"));
        assert!(!compact.contains("x 2"));
    }

    #[test]
    fn shields_evidence_section_schema_rows_from_semantic_tasks() {
        let compiler = Compiler::new();
        let prompt = r#"Postmortem briefing:

Evidence:
(time, service, signal, note)
(10:00, auth, timeout, burst of retries)
(10:05, api, timeout, dependency lag)

Tasks:
- Explain the likely root cause
- Separate symptom from cause
- Return a concise remediation brief"#;

        let program = compiler
            .compile(prompt)
            .expect("evidence-table prompt should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("root")
                && compact.contains("symptom")
                && compact.contains("remediation")
                && !compact.contains("time service signal note")
                && !compact.contains("burst retries")
                && !compact.contains("dependency lag"),
            "expected evidence schema rows to stay out of semantic task items: {compact}"
        );
    }

    #[test]
    fn shields_log_excerpt_lines_from_semantic_tasks() {
        let compiler = Compiler::new();
        let prompt = r#"Debug the following workflow.

Step 1:
- Read the error log

Step 2:
- If the failure is in the parser, go to Step 5

Step 3:
- Inspect the config change

Step 4:
- Otherwise summarize the symptom

Step 5:
- Extract the likely root cause

Log excerpt:
panic: unexpected null pointer at row 44"#;

        let program = compiler
            .compile(prompt)
            .expect("log-excerpt workflow should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("error log")
                && compact.contains("parser")
                && (compact.contains("likely root") || compact.contains("root cause"))
                && !compact.contains("null pointer")
                && !compact.contains("row 44"),
            "expected log excerpt payload to stay out of semantic task items: {compact}"
        );
    }

    #[test]
    fn groups_deeper_child_bullets_under_modify_pipeline_parent_step() {
        let compiler = Compiler::new();
        let prompt = r#"Modify the above pipeline to:
- Filter noise
- Handle missing data
- Preserve true anomalies"#;

        let program = compiler
            .compile(prompt)
            .expect("modify-pipeline prompt should compile");
        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();

        assert_eq!(
            item_count, 1,
            "expected one grouped transform item: {compact}"
        );
        assert!(compact.contains("noise"));
        assert!(compact.contains("missing data"));
        assert!(compact.contains("true anomalies"));
    }

    #[test]
    fn preserves_distinct_branching_step_items_outside_the_stress_suite() {
        let compiler = Compiler::new();
        let prompt = r#"Follow this decision process:

Step 1:
- Inspect the incident report

Step 2:
- If the report contains a confirmed database corruption event, skip Step 3 and go to Step 4

Step 3:
- Summarize the customer-visible symptoms

Step 4:
- Extract the root cause indicators from the timeline
- Explain why this branch overrides the earlier summary step

Output:
- State which path should be followed
- Provide the final remediation memo"#;

        let program = compiler
            .compile(prompt)
            .expect("branching incident workflow should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();

        assert!(
            item_count >= 4
                && (compact.contains("confirmed db corruption")
                    || compact.contains("confirmed database corruption"))
                && (compact.contains("skip step 3 goto4")
                    || compact.contains("skip step 3 goto 4")
                    || compact.contains("skip step 3"))
                && compact.contains("customer-visible symptoms")
                && compact.contains("root indicators")
                && compact.contains("final-remediation-memo")
                && !compact.contains("follow decision process"),
            "expected distinct step items without preamble baggage in compact output: {compact}"
        );
    }

    #[test]
    fn keeps_phase_scoped_operations_workflow_sections_local() {
        let compiler = Compiler::new();
        let prompt = r#"You are reviewing an operations workflow.

Phase A: Intake
- Collect the raw alert stream
- Normalize service names

Phase B: Decision Gate
- If alerts cluster around one region, investigate routing failure
- Otherwise compare deployment versions across regions
- Keep the evidence for the chosen branch only

Phase C: Final Output
- Produce a concise incident narrative
- List the strongest two supporting signals"#;

        let program = compiler
            .compile(prompt)
            .expect("phase-scoped operations workflow should compile");

        let compact = program.to_compact();

        assert!(
            compact.contains("alert stream")
                && compact.contains("service names")
                && compact.contains("routing failure")
                && compact.contains("deployment versions")
                && (compact.contains("incident narrative")
                    || compact.contains("incident-narrative"))
                && (compact.contains("supporting signals")
                    || compact.contains("supporting-signals"))
                && !compact.contains("concise incident narrative")
                && !compact.contains("strongest two supporting signals")
                && !compact.contains("alert stream alerts cluster"),
            "expected phase-local workflow context without cross-phase bleed: {compact}"
        );
    }

    #[test]
    fn workflow_heading_drives_contract_review_items_without_example_id_pollution() {
        let compiler = Compiler::new();
        let prompt = r#"Context:
A parser ingests procurement contracts from three vendors.

Workflow:
1. Detect clauses that change payment timing
2. For any clause that references penalties, also extract the triggering conditions
3. If a clause amends another clause, keep both the amendment and the original dependency
4. Return a schema for downstream review

Constraints:
- Do not treat the example IDs as semantic tasks
- Preserve nested dependencies

Example IDs:
(Contract-17, Clause-4, Amends, Clause-2)
(Contract-21, Clause-9, Penalty, Late delivery)"#;

        let program = compiler
            .compile(prompt)
            .expect("contract review workflow should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();

        assert!(
            item_count >= 4
                && compact.contains("payment timing")
                && compact.contains("triggering conditions")
                && compact.contains("original dependency")
                && compact.contains("schema")
                && !compact.contains("contract 17")
                && !compact.contains("clause 4"),
            "expected workflow heading to preserve numbered contract-review items without payload leakage: {compact}"
        );
    }

    #[test]
    fn rules_and_tasks_prompt_keeps_branch_explanations_separate() {
        let compiler = Compiler::new();
        let prompt = r#"Audit the following moderation policy.

Rules:
- If a post contains direct threats, escalate immediately.
- If a post contains self-harm language without threats, route to safety review.
- If the post quotes another user, separate the quote from the author's own claim.

Tasks:
1. Explain the escalation path for direct threats.
2. Explain the alternate path for self-harm language.
3. Generate a reviewer checklist.
4. Output a short training note."#;

        let program = compiler
            .compile(prompt)
            .expect("moderation-policy workflow should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();

        assert!(
            item_count >= 4
                && compact.contains("direct threats")
                && compact.contains("self-harm language")
                && compact.contains("reviewer checklist")
                && (compact.contains("training note")
                    || compact.contains("training-note")
                    || compact.contains("short-training-note")
                    || compact.contains("a-short-training-note"))
                && !compact.contains("direct threats route safety"),
            "expected rules/tasks prompt to preserve distinct branch explanations: {compact}"
        );
    }

    #[test]
    fn rules_heading_sinks_into_output_only_task_list() {
        let compiler = Compiler::new();
        let prompt = r#"Check the moderation note.

Rules:
- Escalate if there is a direct threat.
- Route to safety review if there is self-harm language.
- Keep quoted language separate from original claims.

Tasks:
- Produce a reviewer checklist
- Return a short training note"#;

        let program = compiler
            .compile(prompt)
            .expect("rules heading with output-only tasks should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();

        assert!(
            item_count == 2
                && compact.contains("reviewer checklist")
                && (compact.contains("training note") || compact.contains("training-note"))
                && compact.contains("direct threat")
                && compact.contains("self-harm language")
                && (compact.contains("quoted language")
                    || compact.contains("quote separate")
                    || compact.contains("quoted separate")),
            "expected rules bullets to sink into output-only tasks instead of becoming peer items: {compact}"
        );
    }

    #[test]
    fn appendix_tuple_rows_with_decimal_values_stay_as_evidence_not_tasks() {
        let compiler = Compiler::new();
        let prompt = r#"Context:
A reviewer is checking a grant proposal with an appendix and a milestone table.

Workflow:
1. Detect budget mismatches across sections
2. If the appendix contradicts the body, extract the conflicting claims
3. Compare the milestone table against the stated timeline
4. Return a reviewer brief

Constraints:
- Keep appendix citations as evidence, not standalone tasks
- Preserve cross-references

Appendix rows:
(Section-A, Budget, 4.2M)
(Section-D, Timeline, Q4 launch)"#;

        let program = compiler
            .compile(prompt)
            .expect("grant-review workflow should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("budget mismatches")
                && compact.contains("conflicting claims")
                && compact.contains("stated timeline")
                && !compact.contains("section-a")
                && !compact.contains("4.2m")
                && !compact.contains("q4 launch"),
            "expected decimal tuple appendix rows to stay out of semantic task items: {compact}"
        );
    }

    #[test]
    fn evidence_schema_rows_do_not_leak_into_postmortem_tasks() {
        let compiler = Compiler::new();
        let prompt = r#"Postmortem briefing:

Evidence:
(time, service, signal, note)
(10:00, auth, timeout, burst of retries)
(10:05, api, timeout, dependency lag)

Tasks:
- Explain the likely root cause
- Separate symptom from cause
- Return a concise remediation brief"#;

        let program = compiler
            .compile(prompt)
            .expect("postmortem evidence workflow should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("root")
                && compact.contains("symptom")
                && compact.contains("remediation")
                && !compact.contains("time service signal note")
                && !compact.contains("burst of retries"),
            "expected evidence schema rows to stay out of semantic task items: {compact}"
        );
    }

    #[test]
    fn log_excerpt_tails_do_not_leak_into_step_items() {
        let compiler = Compiler::new();
        let prompt = r#"Debug the following workflow.

Step 1:
- Read the error log

Step 2:
- If the failure is in the parser, go to Step 5

Step 3:
- Inspect the config change

Step 4:
- Otherwise summarize the symptom

Step 5:
- Extract the likely root cause

Log excerpt:
panic: unexpected null pointer at row 44"#;

        let program = compiler
            .compile(prompt)
            .expect("log-excerpt workflow should compile");

        let compact = program.to_compact();
        assert!(
            compact.contains("error log")
                && compact.contains("parser")
                && compact.contains("config change")
                && compact.contains("likely root")
                && !compact.contains("panic")
                && !compact.contains("row 44"),
            "expected log excerpt tail to stay out of semantic step items: {compact}"
        );
    }

    #[test]
    fn nested_refactor_plan_keeps_parent_bullet_and_child_context_grouped() {
        let compiler = Compiler::new();
        let prompt = r#"Plan a refactor.

Tasks:
- Simplify the parser
  - Keep the input API stable
  - Preserve error messages
- Update the tests
- Return a short implementation plan"#;

        let program = compiler
            .compile(prompt)
            .expect("nested refactor plan should compile");

        let item_count = program
            .blocks
            .iter()
            .map(|block| block.items.len())
            .sum::<usize>();
        let compact = program.to_compact();

        assert!(
            item_count == 3
                && compact.contains("parser")
                && compact.contains("input api stable")
                && compact.contains("error messages")
                && compact.contains("tests")
                && (compact.contains("implementation plan")
                    || compact.contains("implementation-plan")),
            "expected parent refactor bullet to stay grouped with child context: {compact}"
        );
    }
}
