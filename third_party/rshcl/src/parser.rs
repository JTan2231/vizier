use std::collections::{HashMap, hash_map::Entry};

use crate::ast::{
    Attribute, BinaryExpr, BinaryOperator, Block, BlockLabel, Body, BodyItem, ConditionalExpr,
    ConfigFile, Expression, ForExpr, ForExprKind, FunctionCallExpr, GetAttrOp, IndexOp,
    LegacyIndexOp, LiteralExpr, LiteralValue, ObjectExpr, ObjectItem, ObjectKey, OneLineBlock,
    TemplateDirective, TemplateDirectiveSegment, TemplateExpr, TemplateInterpolationSegment,
    TemplateKind, TemplateLiteralSegment, TemplateSegment, TraversalExpr, TraversalOperation,
    TupleExpr, UnaryExpr, UnaryOperator, VariableExpr,
};
use crate::diagnostics::{Diagnostic, Span};
use crate::lexer::{TemplateTokenSegment, Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    pub config: ConfigFile,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(tokens: &[Token]) -> ParseResult {
    Parser::new(tokens).parse()
}

#[derive(Debug, Clone, Copy, Default)]
struct ExprStop {
    newline: bool,
    rbrace: bool,
    rparen: bool,
    rbracket: bool,
    comma: bool,
    colon: bool,
    fat_arrow: bool,
}

impl ExprStop {
    fn attribute(stop_on_rbrace: bool) -> Self {
        Self {
            newline: true,
            rbrace: stop_on_rbrace,
            comma: true,
            ..Self::default()
        }
    }

    fn with_colon(mut self) -> Self {
        self.colon = true;
        self
    }

    fn with_fat_arrow(mut self) -> Self {
        self.fat_arrow = true;
        self
    }

    fn with_rparen(mut self) -> Self {
        self.rparen = true;
        self
    }

    fn with_rbracket(mut self) -> Self {
        self.rbracket = true;
        self
    }

    fn with_rbrace(mut self) -> Self {
        self.rbrace = true;
        self
    }

    fn with_comma(mut self) -> Self {
        self.comma = true;
        self
    }
}

struct ParsedForIntro {
    key_var: Option<String>,
    value_var: String,
    collection: Expression,
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            index: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse(mut self) -> ParseResult {
        let body = self.parse_body(false);
        self.skip_newlines();

        while !self.is_eof() {
            let span = self.current_span();
            self.error("unexpected trailing tokens", span);
            self.advance();
        }

        ParseResult {
            config: ConfigFile { body },
            diagnostics: self.diagnostics,
        }
    }

    fn parse_body(&mut self, stop_on_rbrace: bool) -> Body {
        let mut body = Body::default();
        let mut seen_attributes: HashMap<String, Span> = HashMap::new();

        loop {
            self.skip_newlines();

            if self.is_eof() {
                break;
            }

            if stop_on_rbrace && self.check_rbrace() {
                break;
            }

            let Some((name, name_span)) = self.consume_identifier() else {
                let span = self.current_span();
                self.error("expected attribute or block identifier", span);
                self.recover_to_line_end_or_rbrace(stop_on_rbrace);
                self.consume_newline();
                continue;
            };

            if self.consume_equal() {
                let attribute = self.parse_attribute(name.clone(), name_span);
                match seen_attributes.entry(name) {
                    Entry::Vacant(entry) => {
                        entry.insert(name_span);
                    }
                    Entry::Occupied(_) => {
                        self.error(
                            format!("duplicate attribute `{}` in body", attribute.name),
                            name_span,
                        );
                    }
                }
                body.items.push(BodyItem::Attribute(attribute));
                continue;
            }

            let block_item = self.parse_block(name, name_span);
            body.items.push(block_item);
        }

        body
    }

    fn parse_attribute(&mut self, name: String, name_span: Span) -> Attribute {
        let expression = self.parse_assignment_expression(
            name_span,
            ExprStop::attribute(false),
            Some("each attribute must be on its own line"),
        );

        if !self.consume_newline() {
            let span = self.current_or_eof_span();
            self.error("expected newline after attribute", span);
            self.recover_to_line_end_or_rbrace(false);
            self.consume_newline();
        }

        let span = if expression.span().is_empty() {
            name_span
        } else {
            name_span.merge(expression.span())
        };

        Attribute {
            name,
            expression,
            span,
        }
    }

    fn parse_assignment_expression(
        &mut self,
        name_span: Span,
        stop: ExprStop,
        comma_error: Option<&str>,
    ) -> Expression {
        let expression = match self.parse_expression(stop) {
            Some(expression) => expression,
            None => {
                let span = Span::new(name_span.end, name_span.end);
                self.error("expected expression after `=`", span);
                Expression::Invalid(span)
            }
        };

        if self.check_comma() {
            if let Some(message) = comma_error {
                self.error(message, self.current_span());
            }
            self.advance();
            self.skip_until_expression_terminator(stop.rbrace);
        }

        expression
    }

    fn parse_block(&mut self, block_type: String, type_span: Span) -> BodyItem {
        let mut labels = Vec::new();

        loop {
            match self.current_kind() {
                Some(TokenKind::Identifier(label)) => {
                    labels.push(BlockLabel::Identifier(label.clone()));
                    self.advance();
                }
                Some(TokenKind::StringLiteral(label)) => {
                    labels.push(BlockLabel::StringLiteral(label.clone()));
                    self.advance();
                }
                _ => break,
            }
        }

        let Some(open_span) = self.consume_lbrace() else {
            self.error(
                "expected `{` after block header",
                self.current_or_eof_span(),
            );
            self.recover_to_line_end_or_rbrace(false);
            self.consume_newline();
            return BodyItem::Block(Block {
                block_type,
                labels,
                body: Body::default(),
                span: type_span,
            });
        };

        if self.consume_newline() {
            let body = self.parse_body(true);
            let close_span = match self.consume_rbrace() {
                Some(span) => span,
                None => {
                    self.error("expected `}` to close block", open_span);
                    open_span
                }
            };

            if !self.consume_newline() && !self.is_eof() {
                self.error("expected newline after block", self.current_span());
                self.recover_to_line_end_or_rbrace(false);
                self.consume_newline();
            }

            return BodyItem::Block(Block {
                block_type,
                labels,
                body,
                span: type_span.merge(close_span),
            });
        }

        self.parse_one_line_block(block_type, type_span, labels, open_span)
    }

    fn parse_one_line_block(
        &mut self,
        block_type: String,
        type_span: Span,
        labels: Vec<BlockLabel>,
        open_span: Span,
    ) -> BodyItem {
        let mut attribute = None;

        if !self.check_rbrace() {
            if let Some((name, name_span)) = self.consume_identifier() {
                if self.consume_equal() {
                    let expression = self.parse_assignment_expression(
                        name_span,
                        ExprStop::attribute(true),
                        Some("only one argument is allowed in a single-line block definition"),
                    );
                    let span = if expression.span().is_empty() {
                        name_span
                    } else {
                        name_span.merge(expression.span())
                    };
                    attribute = Some(Attribute {
                        span,
                        name,
                        expression,
                    });
                } else {
                    let span = if self.check_lbrace() {
                        name_span.merge(self.current_span())
                    } else {
                        name_span
                    };
                    self.error(
                        "a single-line block definition cannot contain another block definition",
                        span,
                    );
                    self.skip_single_line_content();
                }
            } else {
                self.error(
                    "invalid single-line block content",
                    self.current_or_eof_span(),
                );
                self.skip_single_line_content();
            }
        }

        if self.check_newline() {
            let start = attribute
                .as_ref()
                .map_or(open_span.end, |parsed_attribute| parsed_attribute.span.end);
            let newline_span = self.current_span();
            self.error(
                "the closing brace for a single-line block definition must be on the same line",
                Span::new(start, newline_span.end),
            );
            self.consume_newline();

            while !self.is_eof() && !self.check_rbrace() {
                self.advance();
            }
        }

        let close_span = match self.consume_rbrace() {
            Some(span) => span,
            None => {
                self.error("expected `}` to close single-line block", open_span);
                open_span
            }
        };

        if !self.consume_newline() && !self.is_eof() {
            self.error("expected newline after block", self.current_span());
            self.recover_to_line_end_or_rbrace(false);
            self.consume_newline();
        }

        BodyItem::OneLineBlock(OneLineBlock {
            block_type,
            labels,
            attribute,
            span: type_span.merge(close_span),
        })
    }

    fn parse_expression(&mut self, stop: ExprStop) -> Option<Expression> {
        self.skip_expression_newlines(stop);
        if self.is_expression_terminator(stop) {
            return None;
        }
        self.parse_conditional(stop)
    }

    fn parse_conditional(&mut self, stop: ExprStop) -> Option<Expression> {
        let predicate = self.parse_binary(stop, 1)?;

        self.skip_expression_newlines(stop);
        if !self.check_question() {
            return Some(predicate);
        }

        let question_span = self.current_span();
        self.advance();

        let if_true = match self.parse_expression(stop.with_colon()) {
            Some(expression) => expression,
            None => {
                let span = Span::new(question_span.end, question_span.end);
                self.error(
                    "missing expression after `?` in conditional expression",
                    span,
                );
                Expression::Invalid(span)
            }
        };

        self.skip_expression_newlines(stop.with_colon());
        let colon_span = if self.consume_colon() {
            self.previous_span()
        } else {
            let span = self.current_or_eof_span();
            self.error("expected `:` in conditional expression", span);
            span
        };

        let if_false = match self.parse_conditional(stop) {
            Some(expression) => expression,
            None => {
                let span = Span::new(colon_span.end, colon_span.end);
                self.error(
                    "missing expression after `:` in conditional expression",
                    span,
                );
                Expression::Invalid(span)
            }
        };

        let span = predicate.span().merge(if_false.span());
        Some(Expression::Conditional(ConditionalExpr {
            predicate: Box::new(predicate),
            if_true: Box::new(if_true),
            if_false: Box::new(if_false),
            span,
        }))
    }

    fn parse_binary(&mut self, stop: ExprStop, min_precedence: u8) -> Option<Expression> {
        let mut left = self.parse_unary(stop)?;

        loop {
            self.skip_expression_newlines(stop);
            if self.is_expression_terminator(stop) {
                break;
            }

            let Some((operator, operator_span)) = self.current_binary_operator() else {
                break;
            };

            if operator.precedence() < min_precedence {
                break;
            }

            self.advance();
            self.skip_expression_newlines(stop);

            let right = match self.parse_binary(stop, operator.precedence() + 1) {
                Some(expression) => expression,
                None => {
                    self.error(
                        format!(
                            "missing operand after operator `{}`",
                            self.binary_operator_symbol(operator)
                        ),
                        operator_span,
                    );
                    let span = Span::new(operator_span.end, operator_span.end);
                    Expression::Invalid(span)
                }
            };

            let span = left.span().merge(right.span());
            left = Expression::Binary(BinaryExpr {
                left: Box::new(left),
                operator,
                right: Box::new(right),
                span,
            });
        }

        Some(left)
    }

    fn parse_unary(&mut self, stop: ExprStop) -> Option<Expression> {
        if self.check_minus() || self.check_bang() {
            let (operator, operator_span) = if self.check_minus() {
                (UnaryOperator::Negate, self.current_span())
            } else {
                (UnaryOperator::Not, self.current_span())
            };
            self.advance();
            self.skip_expression_newlines(stop);

            let expression = match self.parse_unary(stop) {
                Some(expression) => expression,
                None => {
                    self.error("missing operand after unary operator", operator_span);
                    let span = Span::new(operator_span.end, operator_span.end);
                    Expression::Invalid(span)
                }
            };

            let span = operator_span.merge(expression.span());
            return Some(Expression::Unary(UnaryExpr {
                operator,
                expression: Box::new(expression),
                span,
            }));
        }

        self.parse_postfix(stop)
    }

    fn parse_postfix(&mut self, stop: ExprStop) -> Option<Expression> {
        let mut expression = self.parse_primary(stop)?;

        loop {
            self.skip_expression_newlines(stop);
            if self.is_expression_terminator(stop) {
                break;
            }

            if self.check_dot() {
                let Some(operation) = self.parse_dot_operation() else {
                    break;
                };
                expression = self.with_traversal_operation(expression, operation);
                continue;
            }

            if self.check_lbracket() {
                let Some(operation) = self.parse_bracket_operation() else {
                    break;
                };
                expression = self.with_traversal_operation(expression, operation);
                continue;
            }

            if self.check_lparen() {
                self.error("invalid function call syntax", self.current_span());
                self.skip_parenthesized_tokens();
                continue;
            }

            break;
        }

        Some(expression)
    }

    fn parse_primary(&mut self, stop: ExprStop) -> Option<Expression> {
        self.skip_expression_newlines(stop);
        if self.is_expression_terminator(stop) {
            return None;
        }

        let token = self.current_token()?.clone();
        match token.kind {
            TokenKind::Number(number) => {
                self.advance();
                Some(Expression::Literal(LiteralExpr {
                    value: LiteralValue::Number(number),
                    span: token.span,
                }))
            }
            TokenKind::StringLiteral(value) => {
                self.advance();
                Some(Expression::Template(TemplateExpr {
                    segments: vec![TemplateSegment::Literal(TemplateLiteralSegment {
                        value,
                        span: token.span,
                    })],
                    kind: TemplateKind::Quoted,
                    span: token.span,
                }))
            }
            TokenKind::QuotedTemplate { segments } => {
                self.advance();
                Some(Expression::Template(self.parse_template_expression(
                    segments,
                    TemplateKind::Quoted,
                    token.span,
                )))
            }
            TokenKind::HeredocTemplate {
                segments,
                flush,
                marker,
            } => {
                self.advance();
                Some(Expression::Template(self.parse_template_expression(
                    segments,
                    TemplateKind::Heredoc { flush, marker },
                    token.span,
                )))
            }
            TokenKind::Identifier(identifier) => {
                self.advance();

                let expression = match identifier.as_str() {
                    "true" => Expression::Literal(LiteralExpr {
                        value: LiteralValue::Bool(true),
                        span: token.span,
                    }),
                    "false" => Expression::Literal(LiteralExpr {
                        value: LiteralValue::Bool(false),
                        span: token.span,
                    }),
                    "null" => Expression::Literal(LiteralExpr {
                        value: LiteralValue::Null,
                        span: token.span,
                    }),
                    _ => Expression::Variable(VariableExpr {
                        name: identifier.clone(),
                        span: token.span,
                    }),
                };

                if self.check_lparen() {
                    return Some(self.parse_function_call(identifier, token.span));
                }

                Some(expression)
            }
            TokenKind::LParen => self.parse_parenthesized_expression(),
            TokenKind::LBracket => self.parse_tuple_expression(),
            TokenKind::LBrace => self.parse_object_expression(),
            _ => {
                self.error("expected expression term", token.span);
                self.advance();
                Some(Expression::Invalid(token.span))
            }
        }
    }

    fn parse_template_expression(
        &mut self,
        segments: Vec<TemplateTokenSegment>,
        kind: TemplateKind,
        span: Span,
    ) -> TemplateExpr {
        let mut parsed_segments = Vec::new();

        for segment in segments {
            match segment {
                TemplateTokenSegment::Literal { value, span } => {
                    parsed_segments.push(TemplateSegment::Literal(TemplateLiteralSegment {
                        value,
                        span,
                    }));
                }
                TemplateTokenSegment::Interpolation {
                    expression_tokens,
                    strip_left,
                    strip_right,
                    span,
                } => {
                    let expression = self.parse_embedded_expression(
                        &expression_tokens,
                        span,
                        "expected expression in template interpolation",
                    );
                    parsed_segments.push(TemplateSegment::Interpolation(
                        TemplateInterpolationSegment {
                            expression: Box::new(expression),
                            strip_left,
                            strip_right,
                            span,
                        },
                    ));
                }
                TemplateTokenSegment::Directive {
                    directive_tokens,
                    strip_left,
                    strip_right,
                    span,
                } => {
                    let directive = self.parse_template_directive(&directive_tokens, span);
                    parsed_segments.push(TemplateSegment::Directive(TemplateDirectiveSegment {
                        directive,
                        strip_left,
                        strip_right,
                        span,
                    }));
                }
            }
        }

        TemplateExpr {
            segments: parsed_segments,
            kind,
            span,
        }
    }

    fn parse_template_directive(&mut self, tokens: &[Token], span: Span) -> TemplateDirective {
        let tokens = self.trim_newline_tokens(tokens);
        let Some(first) = tokens.first() else {
            self.error("expected template directive keyword", span);
            return TemplateDirective::Unknown {
                keyword: String::new(),
                expression: None,
            };
        };

        let TokenKind::Identifier(keyword) = &first.kind else {
            self.error("expected template directive keyword", first.span);
            return TemplateDirective::Unknown {
                keyword: String::new(),
                expression: None,
            };
        };

        let remainder = self.trim_newline_tokens(&tokens[1..]);
        match keyword.as_str() {
            "if" => {
                let condition = self.parse_embedded_expression(
                    remainder,
                    span,
                    "expected condition expression in template `if` directive",
                );
                TemplateDirective::If {
                    condition: Box::new(condition),
                }
            }
            "else" => {
                if !remainder.is_empty() {
                    self.error(
                        "unexpected tokens after template `else` directive",
                        remainder[0].span,
                    );
                }
                TemplateDirective::Else
            }
            "endif" => {
                if !remainder.is_empty() {
                    self.error(
                        "unexpected tokens after template `endif` directive",
                        remainder[0].span,
                    );
                }
                TemplateDirective::EndIf
            }
            "for" => self.parse_template_for_directive(remainder, span),
            "endfor" => {
                if !remainder.is_empty() {
                    self.error(
                        "unexpected tokens after template `endfor` directive",
                        remainder[0].span,
                    );
                }
                TemplateDirective::EndFor
            }
            _ => {
                self.error(
                    format!("unsupported template directive keyword `{keyword}`"),
                    first.span,
                );
                let expression = if remainder.is_empty() {
                    None
                } else {
                    Some(Box::new(self.parse_embedded_expression(
                        remainder,
                        span,
                        "expected expression after template directive keyword",
                    )))
                };
                TemplateDirective::Unknown {
                    keyword: keyword.clone(),
                    expression,
                }
            }
        }
    }

    fn parse_template_for_directive(&mut self, tokens: &[Token], span: Span) -> TemplateDirective {
        let tokens = self.trim_newline_tokens(tokens);
        let Some((first_name, _)) = self.consume_embedded_identifier(tokens, 0) else {
            self.error(
                "expected iterator variable in template `for` directive",
                span,
            );
            return TemplateDirective::For {
                key_var: None,
                value_var: String::new(),
                collection: Box::new(Expression::Invalid(span)),
            };
        };

        let mut index = 1usize;
        let mut key_var = None;
        let mut value_var = first_name;

        if matches!(
            tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Comma)
        ) {
            key_var = Some(value_var);
            index += 1;
            if let Some((name, _)) = self.consume_embedded_identifier(tokens, index) {
                value_var = name;
                index += 1;
            } else {
                self.error(
                    "expected value variable after `,` in template `for` directive",
                    tokens.get(index).map_or(span, |token| token.span),
                );
                value_var = String::new();
            }
        }

        if matches!(
            tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Identifier(keyword)) if keyword == "in"
        ) {
            index += 1;
        } else {
            self.error(
                "expected `in` in template `for` directive",
                tokens.get(index).map_or(span, |token| token.span),
            );
        }

        let collection_tokens = self.trim_newline_tokens(&tokens[index..]);
        let collection = self.parse_embedded_expression(
            collection_tokens,
            span,
            "expected source expression in template `for` directive",
        );

        TemplateDirective::For {
            key_var,
            value_var,
            collection: Box::new(collection),
        }
    }

    fn parse_embedded_expression(
        &mut self,
        tokens: &[Token],
        span: Span,
        missing_message: &str,
    ) -> Expression {
        let tokens = self.trim_newline_tokens(tokens);
        if tokens.is_empty() {
            self.error(missing_message, span);
            return Expression::Invalid(span);
        }

        let mut embedded_tokens = tokens.to_vec();
        let eof_offset = embedded_tokens
            .last()
            .map_or(span.end, |token| token.span.end);
        embedded_tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(eof_offset, eof_offset),
        });

        let mut embedded_parser = Parser::new(&embedded_tokens);
        let expression = match embedded_parser.parse_expression(ExprStop::default()) {
            Some(expression) => expression,
            None => {
                embedded_parser.error(missing_message, span);
                Expression::Invalid(span)
            }
        };

        embedded_parser.skip_expression_newlines(ExprStop::default());
        if !embedded_parser.is_eof() {
            embedded_parser.error(
                "unexpected trailing tokens in template sequence",
                embedded_parser.current_span(),
            );
            while !embedded_parser.is_eof() {
                embedded_parser.advance();
            }
        }

        self.diagnostics.extend(embedded_parser.diagnostics);
        expression
    }

    fn trim_newline_tokens<'b>(&self, tokens: &'b [Token]) -> &'b [Token] {
        let mut start = 0usize;
        let mut end = tokens.len();

        while start < end && matches!(tokens[start].kind, TokenKind::Newline) {
            start += 1;
        }
        while start < end && matches!(tokens[end - 1].kind, TokenKind::Newline) {
            end -= 1;
        }

        &tokens[start..end]
    }

    fn consume_embedded_identifier(
        &self,
        tokens: &[Token],
        index: usize,
    ) -> Option<(String, Span)> {
        match tokens.get(index) {
            Some(Token {
                kind: TokenKind::Identifier(name),
                span,
            }) => Some((name.clone(), *span)),
            _ => None,
        }
    }

    fn parse_function_call(&mut self, name: String, name_span: Span) -> Expression {
        let open_span = self.current_span();
        self.advance();

        let mut arguments = Vec::new();
        let mut expand_final = false;

        self.skip_expression_newlines(ExprStop::default());
        if self.check_rparen() {
            let close_span = self.current_span();
            self.advance();
            return Expression::FunctionCall(FunctionCallExpr {
                name,
                arguments,
                expand_final,
                span: name_span.merge(close_span),
            });
        }

        loop {
            let stop = ExprStop::default().with_rparen().with_comma();
            let Some(argument) = self.parse_expression(stop) else {
                self.error(
                    "missing operand in function call",
                    self.current_or_eof_span(),
                );
                break;
            };
            arguments.push(argument);

            self.skip_expression_newlines(stop);
            if self.check_ellipsis() {
                expand_final = true;
                let ellipsis_span = self.current_span();
                self.advance();
                self.skip_expression_newlines(stop);

                if self.check_comma() {
                    self.error(
                        "ellipsis expansion must be the final argument in a function call",
                        ellipsis_span,
                    );
                    self.advance();
                    self.skip_expression_newlines(stop);
                    continue;
                }
            }

            if self.check_comma() {
                self.advance();
                self.skip_expression_newlines(stop);
                if self.check_rparen() {
                    break;
                }
                continue;
            }

            if self.check_rparen() {
                break;
            }

            self.error("invalid function call syntax", self.current_or_eof_span());
            self.recover_to_rparen_or_newline();
            if self.check_rparen() || self.check_newline() || self.is_eof() {
                break;
            }
        }

        let close_span = if self.check_rparen() {
            let span = self.current_span();
            self.advance();
            span
        } else {
            self.error(
                "expected `)` to close function call",
                self.current_or_eof_span(),
            );
            self.current_or_eof_span()
        };

        Expression::FunctionCall(FunctionCallExpr {
            name,
            arguments,
            expand_final,
            span: name_span.merge(close_span.merge(open_span)),
        })
    }

    fn parse_parenthesized_expression(&mut self) -> Option<Expression> {
        let open_span = self.current_span();
        self.advance();

        let stop = ExprStop::default().with_rparen();
        let expression = match self.parse_expression(stop) {
            Some(expression) => expression,
            None => {
                let span = Span::new(open_span.end, open_span.end);
                self.error("expected expression after `(`", span);
                Expression::Invalid(span)
            }
        };

        self.skip_expression_newlines(stop);
        if !self.check_rparen() {
            self.error(
                "expected `)` to close expression",
                self.current_or_eof_span(),
            );
            return Some(expression);
        }

        self.advance();
        Some(expression)
    }

    fn parse_for_tuple_expression(&mut self, open_span: Span) -> Expression {
        let intro = self.parse_for_intro();
        let value_stop = ExprStop::default().with_rbracket();
        let value = match self.parse_expression(value_stop) {
            Some(expression) => expression,
            None => {
                self.error(
                    "expected value expression in tuple `for` expression",
                    self.current_or_eof_span(),
                );
                Expression::Invalid(self.current_or_eof_span())
            }
        };

        self.skip_expression_newlines(value_stop);
        let mut condition = None;
        if self.check_identifier("if") {
            let if_span = self.current_span();
            self.advance();
            let condition_stop = ExprStop::default().with_rbracket();
            let parsed_condition = match self.parse_expression(condition_stop) {
                Some(expression) => expression,
                None => {
                    let span = Span::new(if_span.end, if_span.end);
                    self.error("expected condition expression after `if`", span);
                    Expression::Invalid(span)
                }
            };
            condition = Some(Box::new(parsed_condition));
            self.skip_expression_newlines(condition_stop);
        }

        let close_span = if self.check_rbracket() {
            let span = self.current_span();
            self.advance();
            span
        } else {
            self.error(
                "expected `]` to close tuple `for` expression",
                self.current_or_eof_span(),
            );
            self.current_or_eof_span()
        };

        Expression::For(ForExpr {
            key_var: intro.key_var,
            value_var: intro.value_var,
            collection: Box::new(intro.collection),
            kind: ForExprKind::Tuple {
                value: Box::new(value),
            },
            condition,
            span: open_span.merge(close_span),
        })
    }

    fn parse_for_object_expression(&mut self, open_span: Span) -> Expression {
        let intro = self.parse_for_intro();
        let key_stop = ExprStop::default().with_fat_arrow();
        let key = match self.parse_expression(key_stop) {
            Some(expression) => expression,
            None => {
                self.error(
                    "expected key expression in object `for` expression",
                    self.current_or_eof_span(),
                );
                Expression::Invalid(self.current_or_eof_span())
            }
        };

        self.skip_expression_newlines(key_stop);
        if self.check_fat_arrow() {
            self.advance();
        } else {
            self.error(
                "expected `=>` in object `for` expression",
                self.current_or_eof_span(),
            );
        }

        let value_stop = ExprStop::default().with_rbrace();
        let value = match self.parse_expression(value_stop) {
            Some(expression) => expression,
            None => {
                self.error(
                    "expected value expression in object `for` expression",
                    self.current_or_eof_span(),
                );
                Expression::Invalid(self.current_or_eof_span())
            }
        };

        self.skip_expression_newlines(value_stop);
        let mut group = false;
        if self.check_ellipsis() {
            group = true;
            self.advance();
            self.skip_expression_newlines(value_stop);
        }

        let mut condition = None;
        if self.check_identifier("if") {
            let if_span = self.current_span();
            self.advance();
            let condition_stop = ExprStop::default().with_rbrace();
            let parsed_condition = match self.parse_expression(condition_stop) {
                Some(expression) => expression,
                None => {
                    let span = Span::new(if_span.end, if_span.end);
                    self.error("expected condition expression after `if`", span);
                    Expression::Invalid(span)
                }
            };
            condition = Some(Box::new(parsed_condition));
            self.skip_expression_newlines(condition_stop);
        }

        let close_span = if self.check_rbrace() {
            let span = self.current_span();
            self.advance();
            span
        } else {
            self.error(
                "expected `}` to close object `for` expression",
                self.current_or_eof_span(),
            );
            self.current_or_eof_span()
        };

        Expression::For(ForExpr {
            key_var: intro.key_var,
            value_var: intro.value_var,
            collection: Box::new(intro.collection),
            kind: ForExprKind::Object {
                key: Box::new(key),
                value: Box::new(value),
                group,
            },
            condition,
            span: open_span.merge(close_span),
        })
    }

    fn parse_for_intro(&mut self) -> ParsedForIntro {
        let start_span = self.current_or_eof_span();
        if !self.consume_keyword("for") {
            self.error("expected `for` in `for` expression", start_span);
            return ParsedForIntro {
                key_var: None,
                value_var: String::new(),
                collection: Expression::Invalid(start_span),
            };
        }

        self.skip_newlines();
        let Some((first_name, _)) = self.consume_identifier() else {
            self.error(
                "expected iterator variable after `for` in `for` expression",
                self.current_or_eof_span(),
            );
            return ParsedForIntro {
                key_var: None,
                value_var: String::new(),
                collection: Expression::Invalid(self.current_or_eof_span()),
            };
        };

        let mut key_var = None;
        let mut value_var = first_name;

        self.skip_newlines();
        if self.check_comma() {
            self.advance();
            self.skip_newlines();
            key_var = Some(value_var);
            if let Some((name, _)) = self.consume_identifier() {
                value_var = name;
            } else {
                self.error(
                    "expected value iterator variable after `,` in `for` expression",
                    self.current_or_eof_span(),
                );
                value_var = String::new();
            }
        }

        self.skip_newlines();
        if !self.consume_keyword("in") {
            self.error(
                "expected `in` in `for` expression",
                self.current_or_eof_span(),
            );
        }

        self.skip_newlines();
        let source_stop = ExprStop::default().with_colon();
        let collection = match self.parse_expression(source_stop) {
            Some(expression) => expression,
            None => {
                self.error(
                    "expected source expression in `for` expression",
                    self.current_or_eof_span(),
                );
                Expression::Invalid(self.current_or_eof_span())
            }
        };

        self.skip_expression_newlines(source_stop);
        if !self.consume_colon() {
            self.error(
                "expected `:` in `for` expression",
                self.current_or_eof_span(),
            );
        }

        ParsedForIntro {
            key_var,
            value_var,
            collection,
        }
    }

    fn parse_tuple_expression(&mut self) -> Option<Expression> {
        let open_span = self.current_span();
        self.advance();

        self.skip_newlines();
        if self.check_identifier("for") {
            return Some(self.parse_for_tuple_expression(open_span));
        }

        let mut elements = Vec::new();
        let stop = ExprStop {
            newline: true,
            ..ExprStop::default().with_rbracket().with_comma()
        };

        while !self.is_eof() && !self.check_rbracket() {
            let Some(element) = self.parse_expression(stop) else {
                self.error(
                    "expected tuple element expression",
                    self.current_or_eof_span(),
                );
                self.recover_inside_collection(TokenKind::RBracket);
                break;
            };
            elements.push(element);

            if self.check_comma() {
                self.advance();
                self.skip_newlines();
                if self.check_rbracket() {
                    break;
                }
                continue;
            }

            if self.check_newline() {
                self.skip_newlines();
                continue;
            }

            if self.check_rbracket() {
                break;
            }

            self.error(
                "expected `,` or newline between tuple elements",
                self.current_or_eof_span(),
            );
            self.recover_inside_collection(TokenKind::RBracket);
        }

        let close_span = if self.check_rbracket() {
            let span = self.current_span();
            self.advance();
            span
        } else {
            self.error(
                "expected `]` to close tuple expression",
                self.current_or_eof_span(),
            );
            self.current_or_eof_span()
        };

        Some(Expression::Tuple(TupleExpr {
            elements,
            span: open_span.merge(close_span),
        }))
    }

    fn parse_object_expression(&mut self) -> Option<Expression> {
        let open_span = self.current_span();
        self.advance();

        self.skip_newlines();
        if self.check_identifier("for") {
            return Some(self.parse_for_object_expression(open_span));
        }

        let mut items = Vec::new();
        let value_stop = ExprStop {
            newline: true,
            ..ExprStop::default().with_rbrace().with_comma()
        };

        while !self.is_eof() && !self.check_rbrace() {
            let key = if self.next_is_object_identifier_key() {
                match self.consume_identifier() {
                    Some((name, span)) => ObjectKey::Identifier { name, span },
                    None => {
                        self.error("expected object key", self.current_or_eof_span());
                        ObjectKey::Expression {
                            expression: Box::new(Expression::Invalid(self.current_or_eof_span())),
                        }
                    }
                }
            } else {
                let key_stop = ExprStop::default().with_rbrace().with_comma().with_colon();
                let key_expression = match self.parse_expression(key_stop) {
                    Some(expression) => expression,
                    None => {
                        self.error("expected object key expression", self.current_or_eof_span());
                        Expression::Invalid(self.current_or_eof_span())
                    }
                };
                ObjectKey::Expression {
                    expression: Box::new(key_expression),
                }
            };

            self.skip_newlines();
            if !self.consume_equal() && !self.consume_colon() {
                self.error(
                    "expected `=` or `:` after object key",
                    self.current_or_eof_span(),
                );
                self.recover_inside_collection(TokenKind::RBrace);
                if self.check_rbrace() {
                    break;
                }
            }

            let value = match self.parse_expression(value_stop) {
                Some(expression) => expression,
                None => {
                    self.error(
                        "expected expression after object key",
                        self.current_or_eof_span(),
                    );
                    Expression::Invalid(self.current_or_eof_span())
                }
            };

            let item_span = key.span().merge(value.span());
            items.push(ObjectItem {
                key,
                value,
                span: item_span,
            });

            if self.check_comma() {
                self.advance();
                self.skip_newlines();
                if self.check_rbrace() {
                    break;
                }
                continue;
            }

            if self.check_newline() {
                self.skip_newlines();
                continue;
            }

            if self.check_rbrace() {
                break;
            }

            self.error(
                "expected `,` or newline between object elements",
                self.current_or_eof_span(),
            );
            self.recover_inside_collection(TokenKind::RBrace);
        }

        let close_span = if self.check_rbrace() {
            let span = self.current_span();
            self.advance();
            span
        } else {
            self.error(
                "expected `}` to close object expression",
                self.current_or_eof_span(),
            );
            self.current_or_eof_span()
        };

        Some(Expression::Object(ObjectExpr {
            items,
            span: open_span.merge(close_span),
        }))
    }

    fn parse_dot_operation(&mut self) -> Option<TraversalOperation> {
        let dot_span = self.current_span();
        self.advance();

        match self.current_kind() {
            Some(TokenKind::Identifier(name)) => {
                let span = dot_span.merge(self.current_span());
                let name = name.clone();
                self.advance();
                Some(TraversalOperation::GetAttr(GetAttrOp { name, span }))
            }
            Some(TokenKind::Number(number)) => {
                if number.chars().all(|character| character.is_ascii_digit()) {
                    let span = dot_span.merge(self.current_span());
                    let index = number.clone();
                    self.advance();
                    Some(TraversalOperation::LegacyIndex(LegacyIndexOp {
                        index,
                        span,
                    }))
                } else {
                    let span = self.current_span();
                    self.error("invalid legacy index syntax after `.`", span);
                    self.advance();
                    None
                }
            }
            Some(TokenKind::Star) => {
                let span = dot_span.merge(self.current_span());
                self.advance();
                Some(TraversalOperation::AttrSplat { span })
            }
            Some(TokenKind::Newline)
            | Some(TokenKind::RBrace)
            | Some(TokenKind::RParen)
            | Some(TokenKind::RBracket)
            | Some(TokenKind::Comma)
            | Some(TokenKind::Eof) => {
                self.error("invalid traversal syntax after `.`", dot_span);
                None
            }
            Some(_) => {
                let span = self.current_span();
                self.error("invalid traversal syntax after `.`", span);
                self.advance();
                None
            }
            None => {
                self.error("invalid traversal syntax after `.`", dot_span);
                None
            }
        }
    }

    fn parse_bracket_operation(&mut self) -> Option<TraversalOperation> {
        let open_span = self.current_span();
        self.advance();

        if self.check_star() {
            let star_span = self.current_span();
            self.advance();
            if !self.check_rbracket() {
                self.error(
                    "expected `]` after full splat operator",
                    self.current_or_eof_span(),
                );
                return Some(TraversalOperation::FullSplat {
                    span: open_span.merge(star_span),
                });
            }

            let close_span = self.current_span();
            self.advance();
            return Some(TraversalOperation::FullSplat {
                span: open_span.merge(close_span),
            });
        }

        let stop = ExprStop::default().with_rbracket();
        let key = match self.parse_expression(stop) {
            Some(expression) => expression,
            None => {
                self.error("expected index expression inside `[` and `]`", open_span);
                Expression::Invalid(Span::new(open_span.end, open_span.end))
            }
        };

        self.skip_expression_newlines(stop);
        let close_span = if self.check_rbracket() {
            let span = self.current_span();
            self.advance();
            span
        } else {
            self.error(
                "expected `]` to close index expression",
                self.current_or_eof_span(),
            );
            self.current_or_eof_span()
        };

        Some(TraversalOperation::Index(IndexOp {
            key: Box::new(key),
            span: open_span.merge(close_span),
        }))
    }

    fn with_traversal_operation(
        &self,
        expression: Expression,
        operation: TraversalOperation,
    ) -> Expression {
        let operation_span = operation.span();
        match expression {
            Expression::Traversal(mut traversal) => {
                traversal.span = traversal.span.merge(operation_span);
                traversal.operations.push(operation);
                Expression::Traversal(traversal)
            }
            expression => {
                let span = expression.span().merge(operation_span);
                Expression::Traversal(TraversalExpr {
                    target: Box::new(expression),
                    operations: vec![operation],
                    span,
                })
            }
        }
    }

    fn next_is_object_identifier_key(&self) -> bool {
        match self.current_kind() {
            Some(TokenKind::Identifier(_)) => {
                matches!(
                    self.peek_kind(1),
                    Some(TokenKind::Equal) | Some(TokenKind::Colon)
                )
            }
            _ => false,
        }
    }

    fn recover_inside_collection(&mut self, closing: TokenKind) {
        while !self.is_eof() {
            if self.check_newline() || self.check_comma() {
                return;
            }
            if self.matches_kind(&closing) {
                return;
            }
            self.advance();
        }
    }

    fn recover_to_rparen_or_newline(&mut self) {
        let mut depth = 0usize;

        while !self.is_eof() {
            if self.check_newline() && depth == 0 {
                return;
            }

            match self.current_kind() {
                Some(TokenKind::LParen) => {
                    depth += 1;
                }
                Some(TokenKind::RParen) => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }

            self.advance();
        }
    }

    fn skip_parenthesized_tokens(&mut self) {
        if !self.check_lparen() {
            return;
        }

        let mut depth = 0usize;
        while !self.is_eof() {
            match self.current_kind() {
                Some(TokenKind::LParen) => {
                    depth += 1;
                    self.advance();
                }
                Some(TokenKind::RParen) => {
                    self.advance();
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => self.advance(),
            }
        }
    }

    fn current_binary_operator(&self) -> Option<(BinaryOperator, Span)> {
        let kind = self.current_kind()?;
        let operator = match kind {
            TokenKind::Star => BinaryOperator::Multiply,
            TokenKind::Slash => BinaryOperator::Divide,
            TokenKind::Percent => BinaryOperator::Modulo,
            TokenKind::Plus => BinaryOperator::Add,
            TokenKind::Minus => BinaryOperator::Subtract,
            TokenKind::Less => BinaryOperator::Less,
            TokenKind::LessEqual => BinaryOperator::LessEqual,
            TokenKind::Greater => BinaryOperator::Greater,
            TokenKind::GreaterEqual => BinaryOperator::GreaterEqual,
            TokenKind::EqualEqual => BinaryOperator::Equal,
            TokenKind::NotEqual => BinaryOperator::NotEqual,
            TokenKind::AndAnd => BinaryOperator::And,
            TokenKind::OrOr => BinaryOperator::Or,
            _ => return None,
        };

        Some((operator, self.current_span()))
    }

    fn binary_operator_symbol(&self, operator: BinaryOperator) -> &'static str {
        match operator {
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => "/",
            BinaryOperator::Modulo => "%",
            BinaryOperator::Add => "+",
            BinaryOperator::Subtract => "-",
            BinaryOperator::Less => "<",
            BinaryOperator::LessEqual => "<=",
            BinaryOperator::Greater => ">",
            BinaryOperator::GreaterEqual => ">=",
            BinaryOperator::Equal => "==",
            BinaryOperator::NotEqual => "!=",
            BinaryOperator::And => "&&",
            BinaryOperator::Or => "||",
        }
    }

    fn is_expression_terminator(&self, stop: ExprStop) -> bool {
        self.is_eof()
            || (stop.newline && self.check_newline())
            || (stop.rbrace && self.check_rbrace())
            || (stop.rparen && self.check_rparen())
            || (stop.rbracket && self.check_rbracket())
            || (stop.comma && self.check_comma())
            || (stop.colon && self.check_colon())
            || (stop.fat_arrow && self.check_fat_arrow())
    }

    fn skip_expression_newlines(&mut self, stop: ExprStop) -> bool {
        if stop.newline {
            return false;
        }

        let mut consumed = false;
        while self.consume_newline() {
            consumed = true;
        }

        consumed
    }

    fn skip_until_expression_terminator(&mut self, stop_at_rbrace: bool) {
        while !self.is_eof() {
            if self.check_newline() {
                break;
            }
            if stop_at_rbrace && self.check_rbrace() {
                break;
            }
            self.advance();
        }
    }

    fn skip_single_line_content(&mut self) {
        let mut nested_blocks = 0usize;

        while !self.is_eof() {
            if self.check_newline() {
                return;
            }

            if self.check_lbrace() {
                nested_blocks += 1;
                self.advance();
                continue;
            }

            if self.check_rbrace() {
                if nested_blocks == 0 {
                    return;
                }
                nested_blocks -= 1;
                self.advance();
                continue;
            }

            self.advance();
        }
    }

    fn recover_to_line_end_or_rbrace(&mut self, stop_on_rbrace: bool) {
        while !self.is_eof() {
            if self.check_newline() {
                break;
            }
            if stop_on_rbrace && self.check_rbrace() {
                break;
            }
            self.advance();
        }
    }

    fn skip_newlines(&mut self) {
        while self.consume_newline() {}
    }

    fn consume_identifier(&mut self) -> Option<(String, Span)> {
        match self.current_kind() {
            Some(TokenKind::Identifier(name)) => {
                let name = name.clone();
                let span = self.current_span();
                self.advance();
                Some((name, span))
            }
            _ => None,
        }
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        if self.check_identifier(keyword) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume_equal(&mut self) -> bool {
        if matches!(self.current_kind(), Some(TokenKind::Equal)) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume_colon(&mut self) -> bool {
        if self.check_colon() {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume_lbrace(&mut self) -> Option<Span> {
        if matches!(self.current_kind(), Some(TokenKind::LBrace)) {
            let span = self.current_span();
            self.advance();
            Some(span)
        } else {
            None
        }
    }

    fn consume_rbrace(&mut self) -> Option<Span> {
        if self.check_rbrace() {
            let span = self.current_span();
            self.advance();
            Some(span)
        } else {
            None
        }
    }

    fn consume_newline(&mut self) -> bool {
        if self.check_newline() {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check_bang(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Bang))
    }

    fn check_colon(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Colon))
    }

    fn check_fat_arrow(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::FatArrow))
    }

    fn check_identifier(&self, keyword: &str) -> bool {
        matches!(
            self.current_kind(),
            Some(TokenKind::Identifier(name)) if name == keyword
        )
    }

    fn check_comma(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Comma))
    }

    fn check_dot(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Dot))
    }

    fn check_ellipsis(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Ellipsis))
    }

    fn check_lbrace(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::LBrace))
    }

    fn check_lbracket(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::LBracket))
    }

    fn check_lparen(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::LParen))
    }

    fn check_minus(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Minus))
    }

    fn check_newline(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Newline))
    }

    fn check_question(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Question))
    }

    fn check_rbrace(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::RBrace))
    }

    fn check_rbracket(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::RBracket))
    }

    fn check_rparen(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::RParen))
    }

    fn check_star(&self) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Star))
    }

    fn current_kind(&self) -> Option<&TokenKind> {
        self.tokens.get(self.index).map(|token| &token.kind)
    }

    fn peek_kind(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens
            .get(self.index + offset)
            .map(|token| &token.kind)
    }

    fn matches_kind(&self, kind: &TokenKind) -> bool {
        matches!(
            (self.current_kind(), kind),
            (Some(TokenKind::RBrace), TokenKind::RBrace)
                | (Some(TokenKind::RBracket), TokenKind::RBracket)
                | (Some(TokenKind::RParen), TokenKind::RParen)
                | (Some(TokenKind::Comma), TokenKind::Comma)
                | (Some(TokenKind::Newline), TokenKind::Newline)
        )
    }

    fn current_token(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn current_span(&self) -> Span {
        self.current_token()
            .map(|token| token.span)
            .or_else(|| self.tokens.last().map(|token| token.span))
            .unwrap_or_default()
    }

    fn previous_span(&self) -> Span {
        self.tokens
            .get(self.index.saturating_sub(1))
            .map(|token| token.span)
            .unwrap_or_default()
    }

    fn current_or_eof_span(&self) -> Span {
        if self.is_eof() {
            let offset = self.current_offset();
            Span::new(offset, offset)
        } else {
            self.current_span()
        }
    }

    fn current_offset(&self) -> usize {
        if let Some(token) = self.current_token() {
            token.span.start
        } else {
            self.tokens.last().map_or(0, |token| token.span.end)
        }
    }

    fn is_eof(&self) -> bool {
        matches!(self.current_kind(), None | Some(TokenKind::Eof))
    }

    fn advance(&mut self) {
        if self.index < self.tokens.len() {
            self.index += 1;
        }
    }

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics.push(Diagnostic::error(message, span));
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::ast::{
        BinaryExpr, BinaryOperator, BodyItem, ConditionalExpr, Expression, ForExprKind,
        TemplateDirective, TemplateSegment, TraversalOperation,
    };
    use crate::lexer::lex_str;
    use crate::static_analysis::compose_schema_diagnostics;
    use crate::test_fixtures::{
        discover_expression_fixtures, load_fixture_contract, message_snippet_matches,
    };

    use super::parse;

    fn parse_source(source: &str) -> super::ParseResult {
        let lexed = lex_str(source);
        assert!(
            lexed.diagnostics.is_empty(),
            "lexer diagnostics: {:#?}",
            lexed.diagnostics
        );
        parse(&lexed.tokens)
    }

    fn parse_fixture(path: &Path) -> super::ParseResult {
        let source = fs::read_to_string(path).expect("fixture source should exist");
        parse_source(&source)
    }

    fn parse_structure_fixture_with_schema(path: &Path) -> Vec<crate::diagnostics::Diagnostic> {
        let result = parse_fixture(path);
        let super::ParseResult {
            config,
            mut diagnostics,
        } = result;

        let schema_path = path.with_extension("hcldec");
        if schema_path.exists() {
            let schema_source = fs::read(&schema_path).expect("schema fixture should be readable");
            compose_schema_diagnostics(&config, &mut diagnostics, &schema_source);
        }

        diagnostics
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct StructureFixtureDiscoveryError {
        path: PathBuf,
        message: String,
    }

    impl StructureFixtureDiscoveryError {
        fn new(path: &Path, message: impl Into<String>) -> Self {
            Self {
                path: path.to_path_buf(),
                message: message.into(),
            }
        }
    }

    impl fmt::Display for StructureFixtureDiscoveryError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "failed to discover structure fixtures from `{}`: {}",
                self.path.display(),
                self.message
            )
        }
    }

    impl std::error::Error for StructureFixtureDiscoveryError {}

    fn discover_structure_hcl_fixtures_from_root(
        root: &Path,
    ) -> Result<Vec<PathBuf>, StructureFixtureDiscoveryError> {
        let mut fixtures = Vec::new();
        collect_structure_hcl_fixtures(root, &mut fixtures)?;

        if fixtures.is_empty() {
            return Err(StructureFixtureDiscoveryError::new(
                root,
                "no structure fixtures were discovered",
            ));
        }

        Ok(fixtures)
    }

    fn collect_structure_hcl_fixtures(
        path: &Path,
        fixtures: &mut Vec<PathBuf>,
    ) -> Result<(), StructureFixtureDiscoveryError> {
        let entries = fs::read_dir(path).map_err(|error| {
            StructureFixtureDiscoveryError::new(
                path,
                format!("failed to read fixture directory: {error}"),
            )
        })?;
        let mut sorted_entries = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|error| {
                StructureFixtureDiscoveryError::new(
                    path,
                    format!("failed to read fixture directory entry: {error}"),
                )
            })?;
            sorted_entries.push(entry.path());
        }
        sorted_entries.sort();

        for entry in sorted_entries {
            if entry.is_dir() {
                collect_structure_hcl_fixtures(&entry, fixtures)?;
                continue;
            }

            if matches!(entry.extension().and_then(|ext| ext.to_str()), Some("hcl")) {
                fixtures.push(entry);
            }
        }

        Ok(())
    }

    #[test]
    fn parses_attributes_and_blocks() {
        let result = parse_source("a = \"a value\"\nservice \"api\" {\n  b = 1\n}\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        assert_eq!(result.config.body.items.len(), 2);

        match &result.config.body.items[0] {
            BodyItem::Attribute(attribute) => assert_eq!(attribute.name, "a"),
            other => panic!("expected attribute, got {other:?}"),
        }

        match &result.config.body.items[1] {
            BodyItem::Block(block) => {
                assert_eq!(block.block_type, "service");
                assert_eq!(block.labels.len(), 1);
                assert_eq!(block.body.items.len(), 1);
            }
            other => panic!("expected block, got {other:?}"),
        }
    }

    #[test]
    fn parses_one_line_block() {
        let result = parse_source("a { b = \"foo\" }\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        assert_eq!(result.config.body.items.len(), 1);
        match &result.config.body.items[0] {
            BodyItem::OneLineBlock(block) => {
                assert_eq!(block.block_type, "a");
                assert!(block.attribute.is_some());
                assert_eq!(block.attribute.as_ref().expect("attribute").name, "b");
            }
            other => panic!("expected one-line block, got {other:?}"),
        }
    }

    #[test]
    fn reports_duplicate_attributes_per_body_scope() {
        let result = parse_source("a = 1\na = 2\ninner {\n  b = 3\n  b = 4\n}\n");

        assert_eq!(result.diagnostics.len(), 2);
        assert!(
            result.diagnostics[0]
                .message
                .contains("duplicate attribute `a`")
        );
        assert!(
            result.diagnostics[1]
                .message
                .contains("duplicate attribute `b`")
        );
        assert_eq!(result.diagnostics[0].span.start, 6);
    }

    #[test]
    fn reports_invalid_single_line_block_forms() {
        let result =
            parse_source("a { b = \"foo\", c = \"bar\" }\na { b = \"foo\"\n}\na { d {} }\n");

        assert!(result.diagnostics.len() >= 3);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("single-line block"))
        );
    }

    #[test]
    fn reports_comma_separated_attributes() {
        let result = parse_source("a = \"a value\", b = \"b value\"\n");

        assert_eq!(result.diagnostics.len(), 1);
        assert!(
            result.diagnostics[0]
                .message
                .contains("each attribute must be on its own line")
        );
    }

    #[test]
    fn reports_unclosed_multiline_block() {
        let result = parse_source("a {\n");

        assert_eq!(result.diagnostics.len(), 1);
        assert!(
            result.diagnostics[0]
                .message
                .contains("expected `}` to close block")
        );
    }

    #[test]
    fn parses_binary_precedence_and_associativity() {
        let result = parse_source("a = 1 + 2 * 3\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let BodyItem::Attribute(attribute) = &result.config.body.items[0] else {
            panic!("expected attribute")
        };

        let Expression::Binary(BinaryExpr {
            operator: BinaryOperator::Add,
            left,
            right,
            ..
        }) = &attribute.expression
        else {
            panic!("expected top-level addition expression")
        };

        assert!(matches!(left.as_ref(), Expression::Literal(_)));
        assert!(matches!(
            right.as_ref(),
            Expression::Binary(BinaryExpr {
                operator: BinaryOperator::Multiply,
                ..
            })
        ));
    }

    #[test]
    fn parses_conditional_and_traversal_chains() {
        let result =
            parse_source("a = service.api[0].name != null ? service.api[0].name : \"default\"\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let BodyItem::Attribute(attribute) = &result.config.body.items[0] else {
            panic!("expected attribute")
        };

        let Expression::Conditional(ConditionalExpr {
            predicate,
            if_true,
            if_false,
            ..
        }) = &attribute.expression
        else {
            panic!("expected conditional expression")
        };

        assert!(matches!(
            predicate.as_ref(),
            Expression::Binary(BinaryExpr {
                operator: BinaryOperator::NotEqual,
                ..
            })
        ));

        let Expression::Traversal(traversal) = if_true.as_ref() else {
            panic!("expected traversal in true branch")
        };
        assert!(
            traversal
                .operations
                .iter()
                .any(|operation| matches!(operation, TraversalOperation::GetAttr(_)))
        );

        assert!(matches!(if_false.as_ref(), Expression::Template(_)));
    }

    #[test]
    fn reports_expression_specific_diagnostics() {
        let result = parse_source("a = 1 +\nb = foo.\nc = fn(1,, 2)\nd = arr[\n");

        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("missing operand after operator")
        }));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("invalid traversal syntax"))
        );
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("invalid function call syntax")
                || diagnostic
                    .message
                    .contains("missing operand in function call")
        }));
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("expected `]` to close index expression")
        }));
    }

    #[test]
    fn parses_tuple_and_object_for_expressions() {
        let result = parse_source(
            "tuple = [for i, v in values: v if i < 2]\nobject = {for k, v in values: k => v... if v != null}\n",
        );
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let BodyItem::Attribute(tuple_attribute) = &result.config.body.items[0] else {
            panic!("expected tuple attribute")
        };
        let Expression::For(tuple_for) = &tuple_attribute.expression else {
            panic!("expected tuple for expression")
        };
        assert_eq!(tuple_for.key_var.as_deref(), Some("i"));
        assert_eq!(tuple_for.value_var, "v");
        assert!(tuple_for.condition.is_some());
        assert!(matches!(tuple_for.kind, ForExprKind::Tuple { .. }));

        let BodyItem::Attribute(object_attribute) = &result.config.body.items[1] else {
            panic!("expected object attribute")
        };
        let Expression::For(object_for) = &object_attribute.expression else {
            panic!("expected object for expression")
        };
        assert_eq!(object_for.key_var.as_deref(), Some("k"));
        assert_eq!(object_for.value_var, "v");
        let ForExprKind::Object { group, .. } = &object_for.kind else {
            panic!("expected object for expression kind")
        };
        assert!(*group);
        assert!(object_for.condition.is_some());
    }

    #[test]
    fn parses_segmented_template_sequences() {
        let result = parse_source(
            "a = \"hello ${name} %{ if enabled }on%{ endif }\"\n\
b = <<EOT\n\
%{ for i, v in values }${v}\n\
%{ endfor }\n\
EOT\n",
        );
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let BodyItem::Attribute(attribute) = &result.config.body.items[0] else {
            panic!("expected attribute")
        };
        let Expression::Template(template) = &attribute.expression else {
            panic!("expected template expression")
        };
        assert!(
            template
                .segments
                .iter()
                .any(|segment| { matches!(segment, TemplateSegment::Interpolation(_)) })
        );
        assert!(template.segments.iter().any(|segment| {
            matches!(
                segment,
                TemplateSegment::Directive(segment)
                    if matches!(segment.directive, TemplateDirective::If { .. })
            )
        }));
        assert!(template.segments.iter().any(|segment| {
            matches!(
                segment,
                TemplateSegment::Directive(segment)
                    if matches!(segment.directive, TemplateDirective::EndIf)
            )
        }));
    }

    #[test]
    fn reports_template_and_for_clause_diagnostics() {
        let template_result = parse_source("a = \"${} %{ if }x%{ endfor extra }\"\n");
        assert!(template_result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("expected expression in template interpolation")
        }));
        assert!(template_result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("expected condition expression in template `if` directive")
        }));
        assert!(template_result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unexpected tokens after template `endfor` directive")
        }));

        let for_result = parse_source(
            "a = [for : v]\n\
b = {for k, v values: k => v}\n\
c = {for k, v in values k => v}\n",
        );
        assert!(for_result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("expected iterator variable after `for`")
        }));
        assert!(for_result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("expected `in` in `for` expression")
        }));
        assert!(for_result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("expected `:` in `for` expression")
        }));
    }

    #[test]
    fn specsuite_expression_fixtures_parse_without_diagnostics() {
        let fixtures = discover_expression_fixtures().unwrap_or_else(|error| panic!("{error}"));
        for fixture in fixtures {
            let fixture_display = fixture.to_string_lossy();
            let result = parse_fixture(&fixture);
            assert!(
                result.diagnostics.is_empty(),
                "unexpected diagnostics for {fixture_display}: {:#?}",
                result.diagnostics
            );
        }
    }

    #[test]
    fn specsuite_comment_fixtures_parse_without_diagnostics() {
        let fixtures = [
            "specsuite/tests/comments/hash_comment.hcl",
            "specsuite/tests/comments/slash_comment.hcl",
            "specsuite/tests/comments/multiline_comment.hcl",
        ];

        for fixture in fixtures {
            let fixture_path = Path::new(fixture);
            let result = parse_fixture(fixture_path);
            assert!(
                result.diagnostics.is_empty(),
                "unexpected diagnostics for {fixture}: {:#?}",
                result.diagnostics
            );
            assert!(result.config.body.items.is_empty());
        }
    }

    #[test]
    fn specsuite_structure_fixtures_match_t_diagnostics_contract() {
        let root = Path::new("specsuite/tests/structure");
        let fixtures = discover_structure_hcl_fixtures_from_root(root)
            .unwrap_or_else(|error| panic!("{error}"));

        for fixture in fixtures {
            let fixture_display = fixture.to_string_lossy();
            let expectation_path = fixture.with_extension("t");

            let contract =
                load_fixture_contract(&expectation_path).unwrap_or_else(|error| panic!("{error}"));
            let expected = contract.diagnostics;
            let actual = parse_structure_fixture_with_schema(&fixture);
            match expected {
                Some(expected_diagnostics) => {
                    assert_eq!(
                        actual.len(),
                        expected_diagnostics.len(),
                        "diagnostic count mismatch for fixture {fixture_display}\nactual: {actual:#?}\nexpected: {expected_diagnostics:#?}",
                    );

                    for (index, (actual_diagnostic, expected_diagnostic)) in
                        actual.iter().zip(expected_diagnostics.iter()).enumerate()
                    {
                        assert_eq!(
                            actual_diagnostic.severity, expected_diagnostic.severity,
                            "severity mismatch for fixture {fixture_display} diagnostic #{index}",
                        );
                        assert_eq!(
                            (actual_diagnostic.span.start, actual_diagnostic.span.end),
                            (expected_diagnostic.start, expected_diagnostic.end),
                            "span mismatch for fixture {fixture_display} diagnostic #{index}",
                        );

                        if !expected_diagnostic.message_like.is_empty() {
                            let actual_message = actual_diagnostic.message.to_ascii_lowercase();
                            let matches_expected = expected_diagnostic
                                .message_like
                                .iter()
                                .any(|snippet| message_snippet_matches(&actual_message, snippet));
                            assert!(
                                matches_expected,
                                "message mismatch for fixture {fixture_display} diagnostic #{index}\nactual: {}\nexpected snippets: {:?}",
                                actual_diagnostic.message, expected_diagnostic.message_like,
                            );
                        }
                    }
                }
                None => {
                    assert!(
                        actual.is_empty(),
                        "unexpected diagnostics for {fixture_display}: {:#?}",
                        actual
                    );
                }
            }
        }
    }
}
