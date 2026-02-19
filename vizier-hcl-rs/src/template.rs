use std::collections::BTreeMap;

use unicode_normalization::UnicodeNormalization;

use crate::ast::{Expression, TemplateDirective, TemplateExpr, TemplateKind, TemplateSegment};
use crate::diagnostics::Span;
use crate::eval::Evaluator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopDirective {
    Else,
    EndIf,
    EndFor,
}

#[derive(Debug, Clone, Copy)]
struct RenderResult {
    next_index: usize,
    stop: Option<StopDirective>,
    stop_span: Option<Span>,
}

#[derive(Debug, Clone, Copy)]
struct SegmentView<'a> {
    segments: &'a [TemplateSegment],
    stripped_literals: &'a [Option<String>],
}

#[derive(Debug)]
struct RenderState {
    output: String,
    flush_indent: usize,
    line_start: bool,
    remaining_indent: usize,
}

#[derive(Debug, Clone, Copy)]
struct ForDirective<'a> {
    key_var: Option<&'a str>,
    value_var: &'a str,
    collection: &'a crate::ast::Expression,
    span: Span,
}

impl RenderState {
    fn new(flush_indent: usize) -> Self {
        Self {
            output: String::new(),
            flush_indent,
            line_start: true,
            remaining_indent: flush_indent,
        }
    }

    fn append_literal(&mut self, value: &str) {
        for character in value.chars() {
            if character == '\n' {
                self.output.push(character);
                self.line_start = true;
                self.remaining_indent = self.flush_indent;
                continue;
            }

            if self.flush_indent > 0
                && self.line_start
                && self.remaining_indent > 0
                && is_indent_character(character)
            {
                self.remaining_indent -= 1;
                continue;
            }

            self.output.push(character);
            self.line_start = false;
        }
    }

    fn append_interpolation(&mut self, value: &str) {
        self.line_start = false;
        for character in value.chars() {
            self.output.push(character);
            if character == '\n' {
                self.line_start = true;
                self.remaining_indent = self.flush_indent;
            } else {
                self.line_start = false;
            }
        }
    }
}

pub(crate) fn render_template(evaluator: &mut Evaluator<'_>, template: &TemplateExpr) -> String {
    let flush_indent = match template.kind {
        TemplateKind::Heredoc { flush: true, .. } => compute_flush_indent(&template.segments),
        _ => 0,
    };
    let stripped_literals = apply_strip_markers(&template.segments);
    let view = SegmentView {
        segments: &template.segments,
        stripped_literals: &stripped_literals,
    };

    let mut state = RenderState::new(flush_indent);
    let result = render_segments(evaluator, view, 0, &[], true, &mut state);
    if let Some(stop) = result.stop {
        let span = result.stop_span.unwrap_or(template.span);
        match stop {
            StopDirective::Else => {
                evaluator.error("unexpected template `else` directive", span);
            }
            StopDirective::EndIf => {
                evaluator.error("unexpected template `endif` directive", span);
            }
            StopDirective::EndFor => {
                evaluator.error("unexpected template `endfor` directive", span);
            }
        }
    }

    state.output.nfc().collect()
}

pub(crate) fn unwrap_candidate_expression(template: &TemplateExpr) -> Option<&Expression> {
    match template.segments.as_slice() {
        [TemplateSegment::Interpolation(segment)] => Some(segment.expression.as_ref()),
        _ => None,
    }
}

fn render_segments(
    evaluator: &mut Evaluator<'_>,
    view: SegmentView<'_>,
    start: usize,
    stop_on: &[StopDirective],
    execute: bool,
    state: &mut RenderState,
) -> RenderResult {
    let mut index = start;
    while index < view.segments.len() {
        match &view.segments[index] {
            TemplateSegment::Literal(segment) => {
                if execute {
                    let value = view.stripped_literals[index]
                        .as_deref()
                        .unwrap_or(segment.value.as_str());
                    state.append_literal(value);
                }
                index += 1;
            }
            TemplateSegment::Interpolation(segment) => {
                if execute {
                    let value = evaluator.evaluate_expression(&segment.expression);
                    let interpolation = evaluator.interpolation_to_string(value, segment.span);
                    state.append_interpolation(&interpolation);
                }
                index += 1;
            }
            TemplateSegment::Directive(segment) => {
                if execute {
                    // Preserve previous flush-indentation behavior when directives appear at line start.
                    state.line_start = false;
                }

                match &segment.directive {
                    TemplateDirective::If { condition } => {
                        index = render_if(
                            evaluator,
                            view,
                            index + 1,
                            condition,
                            segment.span,
                            execute,
                            state,
                        );
                    }
                    TemplateDirective::For {
                        key_var,
                        value_var,
                        collection,
                    } => {
                        index = render_for(
                            evaluator,
                            view,
                            index + 1,
                            ForDirective {
                                key_var: key_var.as_deref(),
                                value_var,
                                collection,
                                span: segment.span,
                            },
                            execute,
                            state,
                        );
                    }
                    TemplateDirective::Else => {
                        if stop_on.contains(&StopDirective::Else) {
                            return RenderResult {
                                next_index: index + 1,
                                stop: Some(StopDirective::Else),
                                stop_span: Some(segment.span),
                            };
                        }
                        if execute {
                            evaluator.error("unexpected template `else` directive", segment.span);
                        }
                        index += 1;
                    }
                    TemplateDirective::EndIf => {
                        if stop_on.contains(&StopDirective::EndIf) {
                            return RenderResult {
                                next_index: index + 1,
                                stop: Some(StopDirective::EndIf),
                                stop_span: Some(segment.span),
                            };
                        }
                        if execute {
                            evaluator.error("unexpected template `endif` directive", segment.span);
                        }
                        index += 1;
                    }
                    TemplateDirective::EndFor => {
                        if stop_on.contains(&StopDirective::EndFor) {
                            return RenderResult {
                                next_index: index + 1,
                                stop: Some(StopDirective::EndFor),
                                stop_span: Some(segment.span),
                            };
                        }
                        if execute {
                            evaluator.error("unexpected template `endfor` directive", segment.span);
                        }
                        index += 1;
                    }
                    TemplateDirective::Unknown { keyword, .. } => {
                        if execute {
                            evaluator.error(
                                format!("unsupported template directive `{keyword}` in evaluation"),
                                segment.span,
                            );
                        }
                        index += 1;
                    }
                }
            }
        }
    }

    RenderResult {
        next_index: index,
        stop: None,
        stop_span: None,
    }
}

fn render_if(
    evaluator: &mut Evaluator<'_>,
    view: SegmentView<'_>,
    body_start: usize,
    condition: &crate::ast::Expression,
    span: Span,
    execute: bool,
    state: &mut RenderState,
) -> usize {
    let condition_value = if execute {
        let value = evaluator.evaluate_expression(condition);
        evaluator.expect_bool(value, condition.span(), "template `if` condition")
    } else {
        Some(false)
    };
    let render_then = execute && condition_value == Some(true);
    let render_else = execute && condition_value == Some(false);

    let then_result = render_segments(
        evaluator,
        view,
        body_start,
        &[StopDirective::Else, StopDirective::EndIf],
        render_then,
        state,
    );

    match then_result.stop {
        Some(StopDirective::EndIf) => then_result.next_index,
        Some(StopDirective::Else) => {
            let else_result = render_segments(
                evaluator,
                view,
                then_result.next_index,
                &[StopDirective::EndIf],
                render_else,
                state,
            );
            if matches!(else_result.stop, Some(StopDirective::EndIf)) {
                else_result.next_index
            } else {
                evaluator.error("template `if` directive is missing `%{ endif }`", span);
                else_result.next_index
            }
        }
        _ => {
            evaluator.error("template `if` directive is missing `%{ endif }`", span);
            then_result.next_index
        }
    }
}

fn render_for(
    evaluator: &mut Evaluator<'_>,
    view: SegmentView<'_>,
    body_start: usize,
    directive: ForDirective<'_>,
    execute: bool,
    state: &mut RenderState,
) -> usize {
    let mut end_index = None;

    if execute {
        let collection_value = evaluator.evaluate_expression(directive.collection);
        if let Some(iter_items) = evaluator.evaluate_for_collection(
            collection_value,
            directive.collection.span(),
            "template `for` directive",
        ) {
            for (iter_key, iter_value) in iter_items {
                let mut bindings = BTreeMap::new();
                if let Some(key_var) = directive.key_var {
                    bindings.insert(key_var.to_owned(), iter_key);
                }
                bindings.insert(directive.value_var.to_owned(), iter_value);

                let result = evaluator.with_scope(bindings, |evaluator| {
                    render_segments(
                        evaluator,
                        view,
                        body_start,
                        &[StopDirective::EndFor],
                        true,
                        state,
                    )
                });

                if !matches!(result.stop, Some(StopDirective::EndFor)) {
                    evaluator.error(
                        "template `for` directive is missing `%{ endfor }`",
                        directive.span,
                    );
                    return result.next_index;
                }

                if end_index.is_none() {
                    end_index = Some(result.next_index);
                }
            }
        }
    }

    if let Some(end_index) = end_index {
        return end_index;
    }

    let skipped = render_segments(
        evaluator,
        view,
        body_start,
        &[StopDirective::EndFor],
        false,
        state,
    );
    if !matches!(skipped.stop, Some(StopDirective::EndFor)) {
        evaluator.error(
            "template `for` directive is missing `%{ endfor }`",
            directive.span,
        );
    }
    skipped.next_index
}

fn apply_strip_markers(segments: &[TemplateSegment]) -> Vec<Option<String>> {
    let mut stripped_literals = segments
        .iter()
        .map(|segment| match segment {
            TemplateSegment::Literal(segment) => Some(segment.value.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();

    for (index, segment) in segments.iter().enumerate() {
        let (strip_left, strip_right) = match segment {
            TemplateSegment::Interpolation(segment) => (segment.strip_left, segment.strip_right),
            TemplateSegment::Directive(segment) => (segment.strip_left, segment.strip_right),
            TemplateSegment::Literal(_) => (false, false),
        };

        if strip_left
            && index > 0
            && let Some(previous_literal) = stripped_literals[index - 1].as_mut()
        {
            trim_trailing_whitespace(previous_literal);
        }

        if strip_right
            && index + 1 < segments.len()
            && let Some(next_literal) = stripped_literals[index + 1].as_mut()
        {
            trim_leading_whitespace(next_literal);
        }
    }

    stripped_literals
}

fn trim_leading_whitespace(value: &mut String) {
    let trimmed = value
        .trim_start_matches(|character: char| character.is_whitespace())
        .to_owned();
    value.clear();
    value.push_str(&trimmed);
}

fn trim_trailing_whitespace(value: &mut String) {
    while let Some(last) = value.chars().last() {
        if !last.is_whitespace() {
            break;
        }
        value.pop();
    }
}

fn compute_flush_indent(segments: &[TemplateSegment]) -> usize {
    let mut minimum_indent = None::<usize>;
    let mut current_indent = 0usize;
    let mut line_has_content = false;
    let mut line_start = true;

    for segment in segments {
        match segment {
            TemplateSegment::Literal(segment) => {
                for character in segment.value.chars() {
                    if character == '\n' {
                        if line_has_content {
                            minimum_indent = Some(match minimum_indent {
                                Some(current_minimum) => current_minimum.min(current_indent),
                                None => current_indent,
                            });
                        }
                        line_start = true;
                        current_indent = 0;
                        line_has_content = false;
                        continue;
                    }

                    if line_start && is_indent_character(character) {
                        current_indent += 1;
                    } else {
                        line_start = false;
                        line_has_content = true;
                    }
                }
            }
            TemplateSegment::Interpolation(_) | TemplateSegment::Directive(_) => {
                line_start = false;
                line_has_content = true;
            }
        }
    }

    if line_has_content {
        minimum_indent = Some(match minimum_indent {
            Some(current_minimum) => current_minimum.min(current_indent),
            None => current_indent,
        });
    }

    minimum_indent.unwrap_or(0)
}

fn is_indent_character(character: char) -> bool {
    character != '\n' && character != '\r' && character.is_whitespace()
}
