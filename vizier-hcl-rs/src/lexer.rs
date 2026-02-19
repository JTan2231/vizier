use crate::diagnostics::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateTokenSegment {
    Literal {
        value: String,
        span: Span,
    },
    Interpolation {
        expression_tokens: Vec<Token>,
        strip_left: bool,
        strip_right: bool,
        span: Span,
    },
    Directive {
        directive_tokens: Vec<Token>,
        strip_left: bool,
        strip_right: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Identifier(String),
    Number(String),
    StringLiteral(String),
    QuotedTemplate {
        segments: Vec<TemplateTokenSegment>,
    },
    HeredocTemplate {
        segments: Vec<TemplateTokenSegment>,
        flush: bool,
        marker: String,
    },
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    EqualEqual,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Bang,
    Equal,
    FatArrow,
    Question,
    Colon,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Dot,
    Ellipsis,
    Comma,
    TemplateInterpStart,
    TemplateDirectiveStart,
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn lex_str(input: &str) -> LexResult {
    lex_bytes(input.as_bytes())
}

pub fn lex_bytes(input: &[u8]) -> LexResult {
    let mut diagnostics = Vec::new();

    let source = match std::str::from_utf8(input) {
        Ok(source) => source,
        Err(error) => {
            let start = error.valid_up_to();
            let end = error
                .error_len()
                .map_or_else(|| input.len(), |len| start.saturating_add(len));
            diagnostics.push(Diagnostic::error(
                "input is not valid UTF-8",
                Span::new(start, end),
            ));
            return LexResult {
                tokens: vec![Token::new(TokenKind::Eof, Span::new(start, start))],
                diagnostics,
            };
        }
    };

    let mut base_offset = 0;
    if input.starts_with(&[0xEF, 0xBB, 0xBF]) {
        diagnostics.push(Diagnostic::error(
            "UTF-8 byte order mark is not permitted",
            Span::new(0, 3),
        ));
        base_offset = 3;
    }

    let mut result = Lexer::new(&source[base_offset..], base_offset).lex();
    diagnostics.append(&mut result.diagnostics);
    result.diagnostics = diagnostics;
    result
}

fn is_identifier_start(ch: char) -> bool {
    unicode_ident::is_xid_start(ch)
}

fn is_identifier_continue(ch: char) -> bool {
    unicode_ident::is_xid_continue(ch) || ch == '-'
}

struct Lexer<'a> {
    source: &'a str,
    base_offset: usize,
    cursor: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str, base_offset: usize) -> Self {
        Self {
            source,
            base_offset,
            cursor: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn lex(mut self) -> LexResult {
        while let Some(ch) = self.peek_char() {
            match ch {
                ' ' | '\t' => self.consume_horizontal_whitespace(),
                '\n' => self.lex_lf_newline(),
                '\r' => self.lex_carriage_return(),
                '#' => self.lex_line_comment(1),
                '/' if self.starts_with("//") => self.lex_line_comment(2),
                '/' if self.starts_with("/*") => self.lex_inline_comment(),
                '"' => self.lex_string_literal(),
                '<' if self.starts_with("<<") => {
                    if !self.lex_heredoc_template() && !self.lex_punctuator() {
                        let start = self.cursor;
                        self.advance_char();
                        self.push_diagnostic("unexpected character `<`", start, self.cursor);
                    }
                }
                '0'..='9' => self.lex_number(),
                _ if is_identifier_start(ch) => self.lex_identifier(),
                _ => {
                    if !self.lex_punctuator() {
                        let start = self.cursor;
                        self.advance_char();
                        self.push_diagnostic(
                            format!("unexpected character `{ch}`"),
                            start,
                            self.cursor,
                        );
                    }
                }
            }
        }

        let eof = self.source.len();
        self.push_token(TokenKind::Eof, eof, eof);

        LexResult {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn lex_lf_newline(&mut self) {
        let start = self.cursor;
        self.cursor += 1;
        self.push_token(TokenKind::Newline, start, self.cursor);
    }

    fn lex_carriage_return(&mut self) {
        let start = self.cursor;
        self.cursor += 1;

        if self.peek_char() == Some('\n') {
            self.cursor += 1;
        } else {
            self.push_diagnostic(
                "carriage return must be followed by newline",
                start,
                self.cursor,
            );
        }

        self.push_token(TokenKind::Newline, start, self.cursor);
    }

    fn lex_line_comment(&mut self, prefix_len: usize) {
        let start = self.cursor;
        self.cursor += prefix_len;

        while let Some(ch) = self.peek_char() {
            if ch == '\n' {
                self.cursor += 1;
                self.push_token(TokenKind::Newline, start, self.cursor);
                return;
            }

            if ch == '\r' {
                let newline_start = self.cursor;
                self.cursor += 1;
                if self.peek_char() == Some('\n') {
                    self.cursor += 1;
                } else {
                    self.push_diagnostic(
                        "carriage return must be followed by newline",
                        newline_start,
                        self.cursor,
                    );
                }
                self.push_token(TokenKind::Newline, start, self.cursor);
                return;
            }

            self.advance_char();
        }
    }

    fn lex_inline_comment(&mut self) {
        let start = self.cursor;
        self.cursor += 2;

        while self.cursor < self.source.len() {
            if self.starts_with("*/") {
                self.cursor += 2;
                return;
            }
            self.advance_char();
        }

        self.push_diagnostic("unterminated inline comment", start, self.cursor);
    }

    fn lex_string_literal(&mut self) {
        let start = self.cursor;
        self.cursor += 1;

        let mut value = String::new();
        let mut segments = Vec::new();
        let mut literal_start = self.cursor;
        let mut terminated = false;
        let mut emitted_unterminated = false;
        let mut saw_template_sequence = false;

        while let Some(ch) = self.peek_char() {
            match ch {
                '"' => {
                    if saw_template_sequence {
                        self.push_template_literal_segment(
                            &mut segments,
                            &mut value,
                            literal_start,
                            self.cursor,
                        );
                    }
                    self.cursor += 1;
                    terminated = true;
                    break;
                }
                '\\' => {
                    let escape_start = self.cursor;
                    if self.cursor + 1 >= self.source.len() {
                        self.cursor += 1;
                        self.push_diagnostic("unterminated string literal", start, self.cursor);
                        emitted_unterminated = true;
                        break;
                    }

                    self.cursor += 1;
                    self.decode_quoted_escape(&mut value, escape_start);
                }
                '\n' => {
                    self.push_diagnostic(
                        "string literal cannot contain a newline",
                        self.cursor,
                        self.cursor + 1,
                    );
                    break;
                }
                '\r' => {
                    let end = if self.source[self.cursor..].starts_with("\r\n") {
                        self.cursor + 2
                    } else {
                        self.cursor + 1
                    };
                    self.push_diagnostic(
                        "string literal cannot contain a newline",
                        self.cursor,
                        end,
                    );
                    break;
                }
                _ => {
                    if self.starts_with("$${") {
                        value.push_str("${");
                        self.cursor += 3;
                        continue;
                    }

                    if self.starts_with("%%{") {
                        value.push_str("%{");
                        self.cursor += 3;
                        continue;
                    }

                    if self.starts_with("${") {
                        self.push_template_literal_segment(
                            &mut segments,
                            &mut value,
                            literal_start,
                            self.cursor,
                        );
                        saw_template_sequence = true;
                        segments.push(self.lex_template_sequence(false, self.source.len()));
                        literal_start = self.cursor;
                        continue;
                    }

                    if self.starts_with("%{") {
                        self.push_template_literal_segment(
                            &mut segments,
                            &mut value,
                            literal_start,
                            self.cursor,
                        );
                        saw_template_sequence = true;
                        segments.push(self.lex_template_sequence(true, self.source.len()));
                        literal_start = self.cursor;
                        continue;
                    }

                    self.cursor += ch.len_utf8();
                    value.push(ch);
                }
            }
        }

        if !terminated && !emitted_unterminated {
            self.push_diagnostic("unterminated string literal", start, self.cursor);
        }

        if saw_template_sequence {
            self.push_token(TokenKind::QuotedTemplate { segments }, start, self.cursor);
        } else {
            self.push_token(TokenKind::StringLiteral(value), start, self.cursor);
        }
    }

    fn push_template_literal_segment(
        &self,
        segments: &mut Vec<TemplateTokenSegment>,
        value: &mut String,
        start: usize,
        end: usize,
    ) {
        if start >= end || value.is_empty() {
            return;
        }

        segments.push(TemplateTokenSegment::Literal {
            value: std::mem::take(value),
            span: self.local_span(start, end),
        });
    }

    fn lex_template_segments_until(&mut self, limit: usize) -> Vec<TemplateTokenSegment> {
        let mut value = String::new();
        let mut segments = Vec::new();
        let mut literal_start = self.cursor;

        while self.cursor < limit {
            if self.cursor + 3 <= limit && self.starts_with("$${") {
                value.push_str("${");
                self.cursor += 3;
                continue;
            }

            if self.cursor + 3 <= limit && self.starts_with("%%{") {
                value.push_str("%{");
                self.cursor += 3;
                continue;
            }

            if self.cursor + 2 <= limit && self.starts_with("${") {
                self.push_template_literal_segment(
                    &mut segments,
                    &mut value,
                    literal_start,
                    self.cursor,
                );
                segments.push(self.lex_template_sequence(false, limit));
                literal_start = self.cursor;
                continue;
            }

            if self.cursor + 2 <= limit && self.starts_with("%{") {
                self.push_template_literal_segment(
                    &mut segments,
                    &mut value,
                    literal_start,
                    self.cursor,
                );
                segments.push(self.lex_template_sequence(true, limit));
                literal_start = self.cursor;
                continue;
            }

            let Some(ch) = self.peek_char() else {
                break;
            };

            let width = ch.len_utf8();
            if self.cursor + width > limit {
                break;
            }

            self.cursor += width;
            value.push(ch);
        }

        self.push_template_literal_segment(&mut segments, &mut value, literal_start, self.cursor);
        segments
    }

    fn lex_template_sequence(&mut self, directive: bool, limit: usize) -> TemplateTokenSegment {
        let start = self.cursor;
        self.cursor += 2;

        let strip_left = if self.cursor < limit && self.peek_char() == Some('~') {
            self.cursor += 1;
            true
        } else {
            false
        };

        let body_start = self.cursor;
        let mut body_end = limit;
        let mut strip_right = false;
        let mut brace_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut paren_depth = 0usize;

        while self.cursor < limit {
            if self.peek_char() == Some('"') {
                self.cursor += 1;
                while self.cursor < limit {
                    match self.peek_char() {
                        Some('"') => {
                            self.cursor += 1;
                            break;
                        }
                        Some('\\') => {
                            self.cursor += 1;
                            if self.cursor < limit {
                                if let Some(escaped) = self.peek_char() {
                                    self.cursor += escaped.len_utf8();
                                } else {
                                    break;
                                }
                            }
                        }
                        Some(ch) => {
                            self.cursor += ch.len_utf8();
                        }
                        None => break,
                    }
                }
                continue;
            }

            if brace_depth == 0 && bracket_depth == 0 && paren_depth == 0 {
                if self.cursor + 2 <= limit && self.source[self.cursor..].starts_with("~}") {
                    body_end = self.cursor;
                    strip_right = true;
                    self.cursor += 2;
                    let span = self.local_span(start, self.cursor);
                    let nested_tokens = self.lex_template_inner_tokens(body_start, body_end);
                    return if directive {
                        TemplateTokenSegment::Directive {
                            directive_tokens: nested_tokens,
                            strip_left,
                            strip_right,
                            span,
                        }
                    } else {
                        TemplateTokenSegment::Interpolation {
                            expression_tokens: nested_tokens,
                            strip_left,
                            strip_right,
                            span,
                        }
                    };
                }

                if self.peek_char() == Some('}') {
                    body_end = self.cursor;
                    self.cursor += 1;
                    let span = self.local_span(start, self.cursor);
                    let nested_tokens = self.lex_template_inner_tokens(body_start, body_end);
                    return if directive {
                        TemplateTokenSegment::Directive {
                            directive_tokens: nested_tokens,
                            strip_left,
                            strip_right,
                            span,
                        }
                    } else {
                        TemplateTokenSegment::Interpolation {
                            expression_tokens: nested_tokens,
                            strip_left,
                            strip_right,
                            span,
                        }
                    };
                }
            }

            let Some(ch) = self.peek_char() else {
                break;
            };

            match ch {
                '{' => {
                    brace_depth += 1;
                    self.cursor += 1;
                }
                '}' => {
                    brace_depth = brace_depth.saturating_sub(1);
                    self.cursor += 1;
                }
                '[' => {
                    bracket_depth += 1;
                    self.cursor += 1;
                }
                ']' => {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    self.cursor += 1;
                }
                '(' => {
                    paren_depth += 1;
                    self.cursor += 1;
                }
                ')' => {
                    paren_depth = paren_depth.saturating_sub(1);
                    self.cursor += 1;
                }
                _ => {
                    self.cursor += ch.len_utf8();
                }
            }
        }

        self.push_diagnostic(
            if directive {
                "unterminated template directive sequence"
            } else {
                "unterminated template interpolation sequence"
            },
            start,
            limit,
        );

        self.cursor = limit;
        let span = self.local_span(start, self.cursor);
        let nested_tokens = self.lex_template_inner_tokens(body_start, body_end);

        if directive {
            TemplateTokenSegment::Directive {
                directive_tokens: nested_tokens,
                strip_left,
                strip_right,
                span,
            }
        } else {
            TemplateTokenSegment::Interpolation {
                expression_tokens: nested_tokens,
                strip_left,
                strip_right,
                span,
            }
        }
    }

    fn lex_template_inner_tokens(&mut self, start: usize, end: usize) -> Vec<Token> {
        if start >= end {
            return Vec::new();
        }

        let mut nested = Lexer::new(&self.source[start..end], self.base_offset + start).lex();
        self.diagnostics.append(&mut nested.diagnostics);
        if matches!(
            nested.tokens.last().map(|token| &token.kind),
            Some(TokenKind::Eof)
        ) {
            nested.tokens.pop();
        }
        nested.tokens
    }

    fn decode_quoted_escape(&mut self, value: &mut String, escape_start: usize) {
        let Some(escaped) = self.peek_char() else {
            self.push_diagnostic("unterminated string literal", escape_start, self.cursor);
            return;
        };

        match escaped {
            '"' => {
                self.cursor += 1;
                value.push('"');
            }
            '\\' => {
                self.cursor += 1;
                value.push('\\');
            }
            '/' => {
                self.cursor += 1;
                value.push('/');
            }
            'n' => {
                self.cursor += 1;
                value.push('\n');
            }
            'r' => {
                self.cursor += 1;
                value.push('\r');
            }
            't' => {
                self.cursor += 1;
                value.push('\t');
            }
            'u' => {
                self.cursor += 1;
                if let Some(ch) = self.consume_unicode_escape(4, escape_start) {
                    value.push(ch);
                }
            }
            'U' => {
                self.cursor += 1;
                if let Some(ch) = self.consume_unicode_escape(8, escape_start) {
                    value.push(ch);
                }
            }
            other => {
                self.cursor += other.len_utf8();
                value.push(other);
            }
        }
    }

    fn consume_unicode_escape(&mut self, digits: usize, escape_start: usize) -> Option<char> {
        let mut value = 0u32;

        for _ in 0..digits {
            let Some(ch) = self.peek_char() else {
                self.push_diagnostic(
                    "unterminated unicode escape sequence",
                    escape_start,
                    self.cursor,
                );
                return None;
            };

            let Some(digit) = ch.to_digit(16) else {
                self.push_diagnostic(
                    "invalid unicode escape sequence",
                    escape_start,
                    self.cursor + ch.len_utf8(),
                );
                return None;
            };

            self.cursor += ch.len_utf8();
            value = value * 16 + digit;
        }

        match char::from_u32(value) {
            Some(ch) => Some(ch),
            None => {
                self.push_diagnostic(
                    "unicode escape is not a valid Unicode scalar value",
                    escape_start,
                    self.cursor,
                );
                None
            }
        }
    }

    fn lex_heredoc_template(&mut self) -> bool {
        if !self.starts_with("<<") {
            return false;
        }

        let start = self.cursor;
        self.cursor += 2;

        let flush = if self.peek_char() == Some('-') {
            self.cursor += 1;
            true
        } else {
            false
        };

        let marker_start = self.cursor;
        let Some(marker_first) = self.peek_char() else {
            self.cursor = start;
            return false;
        };

        if !is_identifier_start(marker_first) {
            self.cursor = start;
            return false;
        }

        self.advance_char();
        while let Some(ch) = self.peek_char() {
            if is_identifier_continue(ch) {
                self.advance_char();
            } else {
                break;
            }
        }

        let marker = self.source[marker_start..self.cursor].to_owned();

        while matches!(self.peek_char(), Some(' ' | '\t')) {
            self.advance_char();
        }

        if self.consume_line_ending_span().is_none() {
            self.push_diagnostic(
                "expected newline after heredoc marker",
                marker_start,
                self.cursor,
            );
            self.push_token(
                TokenKind::HeredocTemplate {
                    segments: Vec::new(),
                    flush,
                    marker,
                },
                start,
                self.cursor,
            );
            return true;
        }

        let body_start = self.cursor;
        let mut closing_line_start = None;
        let mut closing_newline_span = None;

        while self.cursor < self.source.len() {
            let line_start = self.cursor;
            let line_end = self.find_line_end(line_start);
            let line = &self.source[line_start..line_end];

            let marker_matches = {
                let non_space_index = line
                    .char_indices()
                    .find_map(|(index, ch)| {
                        if ch == ' ' || ch == '\t' {
                            None
                        } else {
                            Some(index)
                        }
                    })
                    .unwrap_or(line.len());
                line[non_space_index..] == marker
            };

            self.cursor = line_end;
            let newline_span = self.consume_line_ending_span();

            if marker_matches {
                closing_line_start = Some(line_start);
                closing_newline_span = newline_span;
                break;
            }

            if newline_span.is_none() {
                break;
            }
        }

        match closing_line_start {
            Some(marker_line_start) => {
                let resume_cursor = self.cursor;
                self.cursor = body_start;
                let segments = self.lex_template_segments_until(marker_line_start);
                self.cursor = resume_cursor;
                let token_end = closing_newline_span.map_or(self.cursor, |(_, end)| end);
                self.push_token(
                    TokenKind::HeredocTemplate {
                        segments,
                        flush,
                        marker,
                    },
                    start,
                    token_end,
                );

                if let Some((newline_start, newline_end)) = closing_newline_span {
                    self.push_token(TokenKind::Newline, newline_start, newline_end);
                } else {
                    self.push_diagnostic(
                        "heredoc template terminator must be followed by newline",
                        marker_line_start,
                        self.cursor,
                    );
                }
            }
            None => {
                let body_end = self.cursor;
                self.push_diagnostic("unterminated heredoc template", start, self.cursor);
                let resume_cursor = self.cursor;
                self.cursor = body_start;
                let segments = self.lex_template_segments_until(body_end);
                self.cursor = resume_cursor;
                self.push_token(
                    TokenKind::HeredocTemplate {
                        segments,
                        flush,
                        marker,
                    },
                    start,
                    self.cursor,
                );
            }
        }

        true
    }

    fn find_line_end(&self, line_start: usize) -> usize {
        let mut index = line_start;
        while index < self.source.len() {
            let ch = self.source[index..]
                .chars()
                .next()
                .expect("index should always be on a char boundary");
            if ch == '\n' || ch == '\r' {
                break;
            }
            index += ch.len_utf8();
        }
        index
    }

    fn consume_line_ending_span(&mut self) -> Option<(usize, usize)> {
        match self.peek_char() {
            Some('\n') => {
                let start = self.cursor;
                self.cursor += 1;
                Some((start, self.cursor))
            }
            Some('\r') => {
                let start = self.cursor;
                self.cursor += 1;
                if self.peek_char() == Some('\n') {
                    self.cursor += 1;
                } else {
                    self.push_diagnostic(
                        "carriage return must be followed by newline",
                        start,
                        self.cursor,
                    );
                }
                Some((start, self.cursor))
            }
            _ => None,
        }
    }

    fn lex_identifier(&mut self) {
        let start = self.cursor;
        self.advance_char();

        while let Some(ch) = self.peek_char() {
            if is_identifier_continue(ch) {
                self.advance_char();
            } else {
                break;
            }
        }

        let identifier = self.source[start..self.cursor].to_owned();
        self.push_token(TokenKind::Identifier(identifier), start, self.cursor);
    }

    fn lex_number(&mut self) {
        let start = self.cursor;
        self.consume_ascii_digits();

        if self.peek_char() == Some('.')
            && matches!(self.peek_nth_char(1), Some(next) if next.is_ascii_digit())
        {
            self.cursor += 1;
            self.consume_ascii_digits();
        }

        if matches!(self.peek_char(), Some('e' | 'E')) {
            let exponent_start = self.cursor;
            self.cursor += 1;

            if matches!(self.peek_char(), Some('+' | '-')) {
                self.cursor += 1;
            }

            let digits_start = self.cursor;
            self.consume_ascii_digits();
            if self.cursor == digits_start {
                self.push_diagnostic(
                    "numeric literal exponent is missing digits",
                    exponent_start,
                    self.cursor,
                );
            }
        }

        let number = self.source[start..self.cursor].to_owned();
        self.push_token(TokenKind::Number(number), start, self.cursor);
    }

    fn lex_punctuator(&mut self) -> bool {
        let start = self.cursor;

        let (kind, len) = if self.starts_with("...") {
            (TokenKind::Ellipsis, 3)
        } else if self.starts_with("&&") {
            (TokenKind::AndAnd, 2)
        } else if self.starts_with("||") {
            (TokenKind::OrOr, 2)
        } else if self.starts_with("==") {
            (TokenKind::EqualEqual, 2)
        } else if self.starts_with("!=") {
            (TokenKind::NotEqual, 2)
        } else if self.starts_with("<=") {
            (TokenKind::LessEqual, 2)
        } else if self.starts_with(">=") {
            (TokenKind::GreaterEqual, 2)
        } else if self.starts_with("=>") {
            (TokenKind::FatArrow, 2)
        } else if self.starts_with("${") {
            (TokenKind::TemplateInterpStart, 2)
        } else if self.starts_with("%{") {
            (TokenKind::TemplateDirectiveStart, 2)
        } else {
            match self.peek_char() {
                Some('+') => (TokenKind::Plus, 1),
                Some('-') => (TokenKind::Minus, 1),
                Some('*') => (TokenKind::Star, 1),
                Some('/') => (TokenKind::Slash, 1),
                Some('%') => (TokenKind::Percent, 1),
                Some('!') => (TokenKind::Bang, 1),
                Some('<') => (TokenKind::Less, 1),
                Some('>') => (TokenKind::Greater, 1),
                Some(':') => (TokenKind::Colon, 1),
                Some('{') => (TokenKind::LBrace, 1),
                Some('}') => (TokenKind::RBrace, 1),
                Some('[') => (TokenKind::LBracket, 1),
                Some(']') => (TokenKind::RBracket, 1),
                Some('(') => (TokenKind::LParen, 1),
                Some(')') => (TokenKind::RParen, 1),
                Some('?') => (TokenKind::Question, 1),
                Some('=') => (TokenKind::Equal, 1),
                Some('.') => (TokenKind::Dot, 1),
                Some(',') => (TokenKind::Comma, 1),
                _ => return false,
            }
        };

        self.cursor += len;
        self.push_token(kind, start, self.cursor);
        true
    }

    fn consume_horizontal_whitespace(&mut self) {
        while matches!(self.peek_char(), Some(' ' | '\t')) {
            self.advance_char();
        }
    }

    fn consume_ascii_digits(&mut self) {
        while matches!(self.peek_char(), Some(ch) if ch.is_ascii_digit()) {
            self.advance_char();
        }
    }

    fn starts_with(&self, needle: &str) -> bool {
        self.source[self.cursor..].starts_with(needle)
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.cursor..].chars().next()
    }

    fn peek_nth_char(&self, n: usize) -> Option<char> {
        self.source[self.cursor..].chars().nth(n)
    }

    fn advance_char(&mut self) {
        if let Some(ch) = self.peek_char() {
            self.cursor += ch.len_utf8();
        }
    }

    fn push_token(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens
            .push(Token::new(kind, self.local_span(start, end)));
    }

    fn push_diagnostic(&mut self, message: impl Into<String>, start: usize, end: usize) {
        self.diagnostics
            .push(Diagnostic::error(message, self.local_span(start, end)));
    }

    fn local_span(&self, start: usize, end: usize) -> Span {
        Span::new(
            self.base_offset.saturating_add(start),
            self.base_offset.saturating_add(end),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{TemplateTokenSegment, TokenKind, lex_bytes, lex_str};

    fn token_tag(kind: &TokenKind) -> &'static str {
        match kind {
            TokenKind::Identifier(_) => "identifier",
            TokenKind::Number(_) => "number",
            TokenKind::StringLiteral(_) => "string",
            TokenKind::QuotedTemplate { .. } => "quoted-template",
            TokenKind::HeredocTemplate { .. } => "heredoc",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::AndAnd => "&&",
            TokenKind::OrOr => "||",
            TokenKind::EqualEqual => "==",
            TokenKind::NotEqual => "!=",
            TokenKind::Less => "<",
            TokenKind::LessEqual => "<=",
            TokenKind::Greater => ">",
            TokenKind::GreaterEqual => ">=",
            TokenKind::Bang => "!",
            TokenKind::Equal => "=",
            TokenKind::FatArrow => "=>",
            TokenKind::Question => "?",
            TokenKind::Colon => ":",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::Dot => ".",
            TokenKind::Ellipsis => "...",
            TokenKind::Comma => ",",
            TokenKind::TemplateInterpStart => "${",
            TokenKind::TemplateDirectiveStart => "%{",
            TokenKind::Newline => "newline",
            TokenKind::Eof => "eof",
        }
    }

    #[test]
    fn lexes_comments_with_newline_awareness() {
        let result = lex_str("a // trailing\n# hash\nb");
        assert!(result.diagnostics.is_empty());

        let tags: Vec<&str> = result
            .tokens
            .iter()
            .map(|token| token_tag(&token.kind))
            .collect();
        assert_eq!(
            tags,
            vec!["identifier", "newline", "newline", "identifier", "eof"]
        );
    }

    #[test]
    fn lexes_unicode_identifiers_and_reports_invalid_start() {
        let result = lex_str("λx = 1\n");
        assert!(result.diagnostics.is_empty());

        match &result.tokens[0].kind {
            TokenKind::Identifier(identifier) => assert_eq!(identifier, "λx"),
            other => panic!("expected identifier token, got {other:?}"),
        }
        assert_eq!(result.tokens[0].span.start, 0);
        assert_eq!(result.tokens[0].span.end, 3);

        let invalid = lex_str("\u{0301}bad = 1\n");
        assert_eq!(invalid.diagnostics.len(), 1);
        assert_eq!(invalid.diagnostics[0].span.start, 0);
        assert_eq!(invalid.diagnostics[0].span.end, 2);
    }

    #[test]
    fn lexes_numeric_literals_and_flags_bad_exponents() {
        let valid = lex_str("0 12 1.5 6e10 7.2E-3\n");
        assert!(valid.diagnostics.is_empty());

        let numbers: Vec<&str> = valid
            .tokens
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::Number(number) => Some(number.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, vec!["0", "12", "1.5", "6e10", "7.2E-3"]);

        let invalid = lex_str("1e+\n");
        assert_eq!(invalid.diagnostics.len(), 1);
        assert!(
            invalid.diagnostics[0]
                .message
                .contains("exponent is missing digits")
        );
        assert_eq!(invalid.diagnostics[0].span.start, 1);
        assert_eq!(invalid.diagnostics[0].span.end, 3);
    }

    #[test]
    fn lexes_heredoc_templates_and_preserves_expression_boundary() {
        let result = lex_str("a = <<EOT\nfoo\nbar\nEOT\nb = 1\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let tags: Vec<&str> = result
            .tokens
            .iter()
            .map(|token| token_tag(&token.kind))
            .collect();
        assert_eq!(
            tags,
            vec![
                "identifier",
                "=",
                "heredoc",
                "newline",
                "identifier",
                "=",
                "number",
                "newline",
                "eof"
            ]
        );

        let heredoc_token = result
            .tokens
            .iter()
            .find(|token| matches!(token.kind, TokenKind::HeredocTemplate { .. }))
            .expect("heredoc token should be present");
        let TokenKind::HeredocTemplate {
            segments,
            flush,
            marker,
        } = &heredoc_token.kind
        else {
            panic!("expected heredoc template token")
        };

        assert_eq!(segments.len(), 1);
        let TemplateTokenSegment::Literal { value, .. } = &segments[0] else {
            panic!("expected literal heredoc segment")
        };
        assert_eq!(value, "foo\nbar\n");
        assert!(!flush);
        assert_eq!(marker, "EOT");
    }

    #[test]
    fn lexes_flush_heredoc_templates_with_indented_terminator() {
        let result = lex_str("a = <<-EOT\n  foo\n  EOT\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let heredoc_token = result
            .tokens
            .iter()
            .find(|token| matches!(token.kind, TokenKind::HeredocTemplate { .. }))
            .expect("heredoc token should be present");
        let TokenKind::HeredocTemplate {
            segments, flush, ..
        } = &heredoc_token.kind
        else {
            panic!("expected heredoc template token")
        };

        assert_eq!(segments.len(), 1);
        let TemplateTokenSegment::Literal { value, .. } = &segments[0] else {
            panic!("expected literal heredoc segment")
        };
        assert_eq!(value, "  foo\n");
        assert!(*flush);
    }

    #[test]
    fn lexes_non_flush_heredoc_templates_with_indented_terminator() {
        let result = lex_str("a = <<EOT\n  foo\n  EOT\nb = 1\n");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        let tags: Vec<&str> = result
            .tokens
            .iter()
            .map(|token| token_tag(&token.kind))
            .collect();
        assert_eq!(
            tags,
            vec![
                "identifier",
                "=",
                "heredoc",
                "newline",
                "identifier",
                "=",
                "number",
                "newline",
                "eof"
            ]
        );

        let heredoc_token = result
            .tokens
            .iter()
            .find(|token| matches!(token.kind, TokenKind::HeredocTemplate { .. }))
            .expect("heredoc token should be present");
        let TokenKind::HeredocTemplate {
            segments, flush, ..
        } = &heredoc_token.kind
        else {
            panic!("expected heredoc template token")
        };

        assert_eq!(segments.len(), 1);
        let TemplateTokenSegment::Literal { value, .. } = &segments[0] else {
            panic!("expected literal heredoc segment")
        };
        assert_eq!(value, "  foo\n");
        assert!(!flush);
    }

    #[test]
    fn lexes_template_sequences_inside_quoted_and_heredoc_templates() {
        let quoted = lex_str("a = \"prefix ${foo} %{ if bar }x%{ endif }\"\n");
        assert!(quoted.diagnostics.is_empty(), "{:#?}", quoted.diagnostics);

        let quoted_token = quoted
            .tokens
            .iter()
            .find(|token| matches!(token.kind, TokenKind::QuotedTemplate { .. }))
            .expect("quoted template token should be present");
        let TokenKind::QuotedTemplate { segments } = &quoted_token.kind else {
            panic!("expected quoted template token")
        };
        assert!(
            segments
                .iter()
                .any(|segment| matches!(segment, TemplateTokenSegment::Interpolation { .. }))
        );
        assert!(
            segments
                .iter()
                .any(|segment| matches!(segment, TemplateTokenSegment::Directive { .. }))
        );

        let heredoc = lex_str("a = <<EOT\nprefix ${foo}\n%{ if bar }x%{ endif }\nEOT\n");
        assert!(heredoc.diagnostics.is_empty(), "{:#?}", heredoc.diagnostics);
        let heredoc_token = heredoc
            .tokens
            .iter()
            .find(|token| matches!(token.kind, TokenKind::HeredocTemplate { .. }))
            .expect("heredoc template token should be present");
        let TokenKind::HeredocTemplate { segments, .. } = &heredoc_token.kind else {
            panic!("expected heredoc template token")
        };
        assert!(
            segments
                .iter()
                .any(|segment| matches!(segment, TemplateTokenSegment::Interpolation { .. }))
        );
        assert!(
            segments
                .iter()
                .any(|segment| matches!(segment, TemplateTokenSegment::Directive { .. }))
        );
    }

    #[test]
    fn decodes_unicode_escapes_and_reports_invalid_forms() {
        let valid = lex_str("a = \"\\u0041\\U0001F600\"\n");
        assert!(valid.diagnostics.is_empty(), "{:#?}", valid.diagnostics);

        let string_token = valid
            .tokens
            .iter()
            .find(|token| matches!(token.kind, TokenKind::StringLiteral(_)))
            .expect("string literal token should be present");
        let TokenKind::StringLiteral(value) = &string_token.kind else {
            panic!("expected string token")
        };
        assert_eq!(value, "A😀");

        let invalid = lex_str("a = \"\\u12\"\n");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unicode escape"))
        );
    }

    #[test]
    fn reports_unterminated_template_sequences() {
        let quoted = lex_str("a = \"${foo\"\n");
        assert!(quoted.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unterminated template interpolation sequence")
        }));

        let heredoc = lex_str("a = <<EOT\n%{ if foo\nEOT\n");
        assert!(heredoc.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unterminated template directive sequence")
        }));
    }

    #[test]
    fn reports_unterminated_heredoc_templates() {
        let result = lex_str("a = <<EOT\nfoo\n");
        assert_eq!(result.diagnostics.len(), 1);
        assert!(
            result.diagnostics[0]
                .message
                .contains("unterminated heredoc template")
        );
    }

    #[test]
    fn reports_malformed_token_sequences() {
        let unterminated_comment = lex_str("/* missing close");
        assert_eq!(unterminated_comment.diagnostics.len(), 1);
        assert!(
            unterminated_comment.diagnostics[0]
                .message
                .contains("unterminated inline comment")
        );

        let unexpected = lex_str("@\n");
        assert_eq!(unexpected.diagnostics.len(), 1);
        assert!(
            unexpected.diagnostics[0]
                .message
                .contains("unexpected character")
        );

        let invalid_utf8 = lex_bytes(&[0x66, 0x6f, 0x80]);
        assert_eq!(invalid_utf8.diagnostics.len(), 1);
        assert!(
            invalid_utf8.diagnostics[0]
                .message
                .contains("not valid UTF-8")
        );
    }

    #[test]
    fn tokenizes_every_spec_operator_and_delimiter() {
        let input = "+ && == < : { [ ( ${\n- || != > ? } ] ) %{\n* ! <= = .\n/ >= => ,\n% ...\n";
        let result = lex_str(input);
        assert!(result.diagnostics.is_empty());

        let tags: Vec<&str> = result
            .tokens
            .iter()
            .map(|token| token_tag(&token.kind))
            .collect();
        assert!(tags.contains(&"+"));
        assert!(tags.contains(&"&&"));
        assert!(tags.contains(&"=="));
        assert!(tags.contains(&"<"));
        assert!(tags.contains(&":"));
        assert!(tags.contains(&"{"));
        assert!(tags.contains(&"["));
        assert!(tags.contains(&"("));
        assert!(tags.contains(&"${"));
        assert!(tags.contains(&"-"));
        assert!(tags.contains(&"||"));
        assert!(tags.contains(&"!="));
        assert!(tags.contains(&">"));
        assert!(tags.contains(&"?"));
        assert!(tags.contains(&"}"));
        assert!(tags.contains(&"]"));
        assert!(tags.contains(&")"));
        assert!(tags.contains(&"%{"));
        assert!(tags.contains(&"*"));
        assert!(tags.contains(&"!"));
        assert!(tags.contains(&"<="));
        assert!(tags.contains(&"="));
        assert!(tags.contains(&"."));
        assert!(tags.contains(&"/"));
        assert!(tags.contains(&">="));
        assert!(tags.contains(&"=>"));
        assert!(tags.contains(&","));
        assert!(tags.contains(&"%"));
        assert!(tags.contains(&"..."));
    }
}
