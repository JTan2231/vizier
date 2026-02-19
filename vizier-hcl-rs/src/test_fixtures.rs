use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{Block, BodyItem, ConfigFile, Expression, LiteralValue, TemplateSegment};
use crate::diagnostics::{Severity, Span};
use crate::lexer::lex_str;
use crate::parser::parse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedDiagnostic {
    pub severity: Severity,
    pub start: usize,
    pub end: usize,
    pub message_like: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FixtureContract {
    pub diagnostics: Option<Vec<ExpectedDiagnostic>>,
    pub result: Option<Expression>,
    pub result_type: Option<Expression>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureContractError {
    path: PathBuf,
    message: String,
}

impl FixtureContractError {
    fn new(path: &Path, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

impl fmt::Display for FixtureContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid fixture contract `{}`: {}",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for FixtureContractError {}

#[derive(Debug, Clone, Copy)]
struct SpanBoundary {
    byte: usize,
    line: Option<usize>,
    column: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationSourceKind {
    Hcl,
    Hcldec,
}

impl ValidationSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hcl => "hcl",
            Self::Hcldec => "hcldec",
        }
    }
}

#[derive(Debug, Clone)]
struct BoundaryValidationSource {
    kind: ValidationSourceKind,
    path: PathBuf,
    bytes: Vec<u8>,
}

struct ParsedTopLevelContract<'a> {
    diagnostics_block: Option<&'a Block>,
    result: Option<Expression>,
    result_type: Option<Expression>,
}

struct ParsedDiagnosticEntry {
    from: SpanBoundary,
    to: SpanBoundary,
    message_like: Option<Vec<String>>,
    source: Option<ValidationSourceKind>,
}

pub(crate) fn load_fixture_contract(path: &Path) -> Result<FixtureContract, FixtureContractError> {
    let source = fs::read_to_string(path).map_err(|error| {
        FixtureContractError::new(path, format!("failed to read file: {error}"))
    })?;
    let validation_sources = load_boundary_validation_sources(path)?;
    let parsed_contract = parse_fixture_contract_ast(path, &source)?;
    let top_level = parse_top_level_contract(path, &parsed_contract)?;
    let diagnostics = parse_expected_diagnostics(
        path,
        &source,
        top_level.diagnostics_block,
        &validation_sources,
    )?;

    Ok(FixtureContract {
        diagnostics,
        result: top_level.result,
        result_type: top_level.result_type,
    })
}

fn load_boundary_validation_sources(
    contract_path: &Path,
) -> Result<Vec<BoundaryValidationSource>, FixtureContractError> {
    let mut sources = Vec::new();

    for (extension, kind) in [
        ("hcl", ValidationSourceKind::Hcl),
        ("hcldec", ValidationSourceKind::Hcldec),
    ] {
        let source_path = contract_path.with_extension(extension);
        if !source_path.exists() {
            continue;
        }

        let bytes = fs::read(&source_path).map_err(|error| {
            FixtureContractError::new(
                contract_path,
                format!(
                    "failed to read sibling source `{}` for span validation: {error}",
                    source_path.display()
                ),
            )
        })?;

        sources.push(BoundaryValidationSource {
            kind,
            path: source_path,
            bytes,
        });
    }

    Ok(sources)
}

pub(crate) fn discover_expression_fixtures() -> Result<Vec<PathBuf>, FixtureContractError> {
    discover_expression_fixtures_from_root(Path::new("specsuite/tests/expressions"))
}

pub(crate) fn discover_expression_fixtures_from_root(
    root: &Path,
) -> Result<Vec<PathBuf>, FixtureContractError> {
    let entries = fs::read_dir(root).map_err(|error| {
        FixtureContractError::new(
            root,
            format!("failed to read expression fixture directory: {error}"),
        )
    })?;

    let mut fixtures = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            FixtureContractError::new(
                root,
                format!("failed to read expression fixture directory entry: {error}"),
            )
        })?;
        let path = entry.path();
        if matches!(path.extension().and_then(|ext| ext.to_str()), Some("hcl")) {
            fixtures.push(path);
        }
    }
    fixtures.sort();

    if fixtures.is_empty() {
        return Err(FixtureContractError::new(
            root,
            format!(
                "no expression fixtures were discovered in `{}`",
                root.display()
            ),
        ));
    }

    for fixture in &fixtures {
        let contract_path = fixture.with_extension("t");
        if !contract_path.exists() {
            return Err(FixtureContractError::new(
                &contract_path,
                format!(
                    "missing sibling fixture contract `{}` for expression fixture `{}`",
                    contract_path.display(),
                    fixture.display(),
                ),
            ));
        }
    }

    Ok(fixtures)
}

pub(crate) fn message_snippet_matches(actual: &str, snippet: &str) -> bool {
    if actual.contains(snippet) {
        return true;
    }

    let variants = [
        snippet.replace("arguments", "attributes"),
        snippet.replace("argument", "attribute"),
        snippet.replace("attributes", "arguments"),
        snippet.replace("attribute", "argument"),
    ];

    variants
        .iter()
        .any(|variant| !variant.is_empty() && actual.contains(variant))
}

fn parse_fixture_contract_ast(
    path: &Path,
    source: &str,
) -> Result<ConfigFile, FixtureContractError> {
    let lexed = lex_str(source);
    if !lexed.diagnostics.is_empty() {
        return Err(FixtureContractError::new(
            path,
            format!(
                "lexer diagnostics while parsing contract: {:#?}",
                lexed.diagnostics
            ),
        ));
    }

    let parsed = parse(&lexed.tokens);
    if !parsed.diagnostics.is_empty() {
        return Err(FixtureContractError::new(
            path,
            format!(
                "parser diagnostics while parsing contract: {:#?}",
                parsed.diagnostics
            ),
        ));
    }

    Ok(parsed.config)
}

fn parse_top_level_contract<'a>(
    path: &Path,
    config: &'a ConfigFile,
) -> Result<ParsedTopLevelContract<'a>, FixtureContractError> {
    let mut result = None;
    let mut result_type = None;
    let mut diagnostics_block = None;

    for (index, item) in config.body.items.iter().enumerate() {
        match item {
            BodyItem::Attribute(attribute) => match attribute.name.as_str() {
                "result" => {
                    if result.is_some() {
                        return Err(FixtureContractError::new(
                            path,
                            format!(
                                "contains duplicate top-level `result` attribute at index {index}"
                            ),
                        ));
                    }
                    result = Some(attribute.expression.clone());
                }
                "result_type" => {
                    if result_type.is_some() {
                        return Err(FixtureContractError::new(
                            path,
                            format!(
                                "contains duplicate top-level `result_type` attribute at index {index}"
                            ),
                        ));
                    }
                    result_type = Some(attribute.expression.clone());
                }
                _ => {
                    return Err(FixtureContractError::new(
                        path,
                        format!(
                            "contains unsupported top-level attribute `{}` at index {index}; expected only `result` and `result_type`",
                            attribute.name,
                        ),
                    ));
                }
            },
            BodyItem::Block(block) if block.block_type == "diagnostics" => {
                if diagnostics_block.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        "contains multiple `diagnostics` blocks",
                    ));
                }
                diagnostics_block = Some(block);
            }
            BodyItem::Block(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "contains unsupported top-level block `{}` at index {index}; expected only `diagnostics`",
                        block.block_type,
                    ),
                ));
            }
            BodyItem::OneLineBlock(block) if block.block_type == "diagnostics" => {
                return Err(FixtureContractError::new(
                    path,
                    "has one-line `diagnostics` block; use multiline `diagnostics { ... }`",
                ));
            }
            BodyItem::OneLineBlock(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "contains unsupported top-level one-line block `{}` at index {index}",
                        block.block_type,
                    ),
                ));
            }
        }
    }

    Ok(ParsedTopLevelContract {
        diagnostics_block,
        result,
        result_type,
    })
}

fn parse_expected_diagnostics(
    path: &Path,
    source: &str,
    diagnostics_block: Option<&Block>,
    validation_sources: &[BoundaryValidationSource],
) -> Result<Option<Vec<ExpectedDiagnostic>>, FixtureContractError> {
    let Some(diagnostics_block) = diagnostics_block else {
        return Ok(None);
    };

    if !diagnostics_block.labels.is_empty() {
        return Err(FixtureContractError::new(
            path,
            "`diagnostics` block must not have labels",
        ));
    }

    let mut expected = Vec::new();
    for (index, item) in diagnostics_block.body.items.iter().enumerate() {
        let (severity, diagnostic_block) = match item {
            BodyItem::Block(block) if block.block_type == "error" => (Severity::Error, block),
            BodyItem::Block(block) if block.block_type == "warning" => (Severity::Warning, block),
            BodyItem::Block(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "has invalid diagnostics entry block `{}` at index {index}; expected `error` or `warning`",
                        block.block_type
                    ),
                ));
            }
            BodyItem::OneLineBlock(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry `{}` at index {index} must be multiline",
                        block.block_type
                    ),
                ));
            }
            BodyItem::Attribute(attribute) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics block contains attribute `{}` at index {index}; entries must be `error` or `warning` blocks",
                        attribute.name
                    ),
                ));
            }
        };

        if !diagnostic_block.labels.is_empty() {
            return Err(FixtureContractError::new(
                path,
                format!(
                    "diagnostics entry #{index} (`{}`) must not have labels",
                    diagnostic_block.block_type
                ),
            ));
        }

        let diagnostic_entry = parse_diagnostic_entry(path, diagnostic_block, index)?;

        if diagnostic_entry.from.byte > diagnostic_entry.to.byte {
            return Err(FixtureContractError::new(
                path,
                format!(
                    "diagnostics entry #{index} (`{}`) has invalid span: `from.byte` ({}) must be less than or equal to `to.byte` ({})",
                    diagnostic_block.block_type,
                    diagnostic_entry.from.byte,
                    diagnostic_entry.to.byte
                ),
            ));
        }

        if diagnostic_entry.source.is_some()
            || has_coordinate_expectations(diagnostic_entry.from)
            || has_coordinate_expectations(diagnostic_entry.to)
        {
            let validation_source = resolve_validation_source(
                path,
                diagnostic_block,
                index,
                diagnostic_entry.from,
                diagnostic_entry.to,
                diagnostic_entry.source,
                validation_sources,
            )?;
            validate_boundary_coordinates(
                path,
                diagnostic_block,
                index,
                "from",
                diagnostic_entry.from,
                validation_source,
            )?;
            validate_boundary_coordinates(
                path,
                diagnostic_block,
                index,
                "to",
                diagnostic_entry.to,
                validation_source,
            )?;
        }

        let message_like = diagnostic_entry
            .message_like
            .unwrap_or_else(|| parse_message_like_for_block(source, diagnostic_block.span));

        expected.push(ExpectedDiagnostic {
            severity,
            start: diagnostic_entry.from.byte,
            end: diagnostic_entry.to.byte,
            message_like,
        });
    }

    Ok(Some(expected))
}

fn parse_diagnostic_entry(
    path: &Path,
    diagnostic_block: &Block,
    index: usize,
) -> Result<ParsedDiagnosticEntry, FixtureContractError> {
    let mut from = None;
    let mut to = None;
    let mut message_like = None;
    let mut source = None;

    for item in &diagnostic_block.body.items {
        match item {
            BodyItem::Block(block) if block.block_type == "from" => {
                if from.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!("diagnostics entry #{index} has multiple `from` blocks"),
                    ));
                }
                from = Some(parse_boundary(
                    path,
                    diagnostic_block,
                    block,
                    index,
                    "from",
                )?);
            }
            BodyItem::Block(block) if block.block_type == "to" => {
                if to.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!("diagnostics entry #{index} has multiple `to` blocks"),
                    ));
                }
                to = Some(parse_boundary(path, diagnostic_block, block, index, "to")?);
            }
            BodyItem::Block(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry #{index} (`{}`) has unsupported block `{}`; expected only `from` and `to`",
                        diagnostic_block.block_type, block.block_type,
                    ),
                ));
            }
            BodyItem::OneLineBlock(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry #{index} (`{}`) contains one-line block `{}`; `from` and `to` must be multiline blocks",
                        diagnostic_block.block_type, block.block_type,
                    ),
                ));
            }
            BodyItem::Attribute(attribute) if attribute.name == "message_like" => {
                if message_like.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!(
                            "diagnostics entry #{index} (`{}`) contains multiple `message_like` attributes",
                            diagnostic_block.block_type,
                        ),
                    ));
                }
                message_like = Some(parse_message_like_attribute(
                    path,
                    diagnostic_block,
                    &attribute.expression,
                    index,
                )?);
            }
            BodyItem::Attribute(attribute) if attribute.name == "source" => {
                if source.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!(
                            "diagnostics entry #{index} (`{}`) contains multiple `source` attributes",
                            diagnostic_block.block_type,
                        ),
                    ));
                }
                source = Some(parse_source_selector_attribute(
                    path,
                    diagnostic_block,
                    &attribute.expression,
                    index,
                )?);
            }
            BodyItem::Attribute(attribute) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry #{index} (`{}`) contains unsupported attribute `{}`; expected `from`/`to` blocks and optional `message_like`/`source`",
                        diagnostic_block.block_type, attribute.name,
                    ),
                ));
            }
        }
    }

    let Some(from) = from else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) is missing required `from` block",
                diagnostic_block.block_type,
            ),
        ));
    };

    let Some(to) = to else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) is missing required `to` block",
                diagnostic_block.block_type,
            ),
        ));
    };

    Ok(ParsedDiagnosticEntry {
        from,
        to,
        message_like,
        source,
    })
}

fn parse_boundary(
    path: &Path,
    diagnostic_block: &Block,
    boundary_block: &Block,
    index: usize,
    boundary_name: &str,
) -> Result<SpanBoundary, FixtureContractError> {
    if !boundary_block.labels.is_empty() {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `{boundary_name}` block must not have labels",
                diagnostic_block.block_type,
            ),
        ));
    }

    let mut byte = None;
    let mut line = None;
    let mut column = None;

    for item in &boundary_block.body.items {
        match item {
            BodyItem::Attribute(attribute) if attribute.name == "byte" => {
                if byte.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!(
                            "diagnostics entry #{index} (`{}`) `{boundary_name}` block has multiple `byte` attributes",
                            diagnostic_block.block_type,
                        ),
                    ));
                }
                byte = Some(parse_unsigned_integer_expression(
                    path,
                    diagnostic_block,
                    &attribute.expression,
                    index,
                    boundary_name,
                    "byte",
                )?);
            }
            BodyItem::Attribute(attribute) if attribute.name == "line" => {
                if line.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!(
                            "diagnostics entry #{index} (`{}`) `{boundary_name}` block has multiple `line` attributes",
                            diagnostic_block.block_type,
                        ),
                    ));
                }
                line = Some(parse_unsigned_integer_expression(
                    path,
                    diagnostic_block,
                    &attribute.expression,
                    index,
                    boundary_name,
                    "line",
                )?);
            }
            BodyItem::Attribute(attribute) if attribute.name == "column" => {
                if column.is_some() {
                    return Err(FixtureContractError::new(
                        path,
                        format!(
                            "diagnostics entry #{index} (`{}`) `{boundary_name}` block has multiple `column` attributes",
                            diagnostic_block.block_type,
                        ),
                    ));
                }
                column = Some(parse_unsigned_integer_expression(
                    path,
                    diagnostic_block,
                    &attribute.expression,
                    index,
                    boundary_name,
                    "column",
                )?);
            }
            BodyItem::Attribute(attribute) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry #{index} (`{}`) `{boundary_name}` block has unsupported attribute `{}`; expected `byte` (and optional `line`/`column`)",
                        diagnostic_block.block_type, attribute.name,
                    ),
                ));
            }
            BodyItem::Block(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry #{index} (`{}`) `{boundary_name}` block contains nested block `{}`; expected only attributes",
                        diagnostic_block.block_type, block.block_type,
                    ),
                ));
            }
            BodyItem::OneLineBlock(block) => {
                return Err(FixtureContractError::new(
                    path,
                    format!(
                        "diagnostics entry #{index} (`{}`) `{boundary_name}` block contains one-line block `{}`; expected only attributes",
                        diagnostic_block.block_type, block.block_type,
                    ),
                ));
            }
        }
    }

    let Some(byte) = byte else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `{boundary_name}` block is missing required `byte` attribute",
                diagnostic_block.block_type,
            ),
        ));
    };

    Ok(SpanBoundary { byte, line, column })
}

fn parse_unsigned_integer_expression(
    path: &Path,
    diagnostic_block: &Block,
    expression: &Expression,
    index: usize,
    boundary_name: &str,
    field_name: &str,
) -> Result<usize, FixtureContractError> {
    let Expression::Literal(literal) = expression else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `{boundary_name}` `{field_name}` must be an unsigned integer literal",
                diagnostic_block.block_type,
            ),
        ));
    };

    let LiteralValue::Number(number) = &literal.value else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `{boundary_name}` `{field_name}` must be an unsigned integer literal",
                diagnostic_block.block_type,
            ),
        ));
    };

    number.parse::<usize>().map_err(|error| {
        FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `{boundary_name}` `{field_name}` value `{number}` is invalid: {error}",
                diagnostic_block.block_type,
            ),
        )
    })
}

fn parse_message_like_attribute(
    path: &Path,
    diagnostic_block: &Block,
    expression: &Expression,
    index: usize,
) -> Result<Vec<String>, FixtureContractError> {
    let Expression::Tuple(tuple) = expression else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `message_like` must be a list of string literals",
                diagnostic_block.block_type,
            ),
        ));
    };

    let mut snippets = Vec::with_capacity(tuple.elements.len());
    for value in &tuple.elements {
        let Some(snippet) = extract_string_literal(value) else {
            return Err(FixtureContractError::new(
                path,
                format!(
                    "diagnostics entry #{index} (`{}`) `message_like` must contain only string literals",
                    diagnostic_block.block_type,
                ),
            ));
        };
        snippets.push(snippet.to_ascii_lowercase());
    }

    Ok(snippets)
}

fn parse_source_selector_attribute(
    path: &Path,
    diagnostic_block: &Block,
    expression: &Expression,
    index: usize,
) -> Result<ValidationSourceKind, FixtureContractError> {
    let Some(value) = extract_string_literal(expression) else {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) `source` must be a string literal with value `hcl` or `hcldec`",
                diagnostic_block.block_type,
            ),
        ));
    };

    match value.as_str() {
        "hcl" => Ok(ValidationSourceKind::Hcl),
        "hcldec" => Ok(ValidationSourceKind::Hcldec),
        _ => Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) has invalid `source` value `{value}`; expected `hcl` or `hcldec`",
                diagnostic_block.block_type,
            ),
        )),
    }
}

fn extract_string_literal(expression: &Expression) -> Option<String> {
    let Expression::Template(template) = expression else {
        return None;
    };

    let [TemplateSegment::Literal(segment)] = template.segments.as_slice() else {
        return None;
    };

    Some(segment.value.clone())
}

fn has_coordinate_expectations(boundary: SpanBoundary) -> bool {
    boundary.line.is_some() || boundary.column.is_some()
}

fn resolve_validation_source<'a>(
    path: &Path,
    diagnostic_block: &Block,
    index: usize,
    from: SpanBoundary,
    to: SpanBoundary,
    source_selector: Option<ValidationSourceKind>,
    validation_sources: &'a [BoundaryValidationSource],
) -> Result<&'a BoundaryValidationSource, FixtureContractError> {
    if let Some(kind) = source_selector {
        let Some(selected) = validation_sources.iter().find(|source| source.kind == kind) else {
            return Err(FixtureContractError::new(
                path,
                format!(
                    "diagnostics entry #{index} (`{}`) selects `source = \"{}\"`, but sibling source `.{}` does not exist",
                    diagnostic_block.block_type,
                    kind.as_str(),
                    kind.as_str(),
                ),
            ));
        };
        return Ok(selected);
    }

    if validation_sources.is_empty() {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) includes coordinate validation, but no sibling `.hcl` or `.hcldec` source exists",
                diagnostic_block.block_type,
            ),
        ));
    }
    if validation_sources.len() == 1 {
        return Ok(&validation_sources[0]);
    }

    let matches_both = validation_sources
        .iter()
        .filter(|source| {
            boundary_matches_source(from, &source.bytes)
                && boundary_matches_source(to, &source.bytes)
        })
        .collect::<Vec<_>>();
    if let Some(source) = matches_both
        .iter()
        .copied()
        .find(|source| source.kind == ValidationSourceKind::Hcl)
        .or_else(|| matches_both.first().copied())
    {
        return Ok(source);
    }

    let from_matches = validation_sources
        .iter()
        .filter(|source| boundary_matches_source(from, &source.bytes))
        .collect::<Vec<_>>();
    let to_matches = validation_sources
        .iter()
        .filter(|source| boundary_matches_source(to, &source.bytes))
        .collect::<Vec<_>>();

    if !from_matches.is_empty() && !to_matches.is_empty() {
        return Err(FixtureContractError::new(
            path,
            format!(
                "diagnostics entry #{index} (`{}`) resolves `from` and `to` to different sibling sources (`from`: {}; `to`: {}); set `source = \"hcl\"` or `source = \"hcldec\"` so both boundaries validate against one source",
                diagnostic_block.block_type,
                format_matching_sources(&from_matches),
                format_matching_sources(&to_matches),
            ),
        ));
    }

    Err(FixtureContractError::new(
        path,
        format!(
            "diagnostics entry #{index} (`{}`) could not resolve a sibling source that matches both boundaries (`from`: {}; `to`: {})",
            diagnostic_block.block_type,
            format_matching_sources(&from_matches),
            format_matching_sources(&to_matches),
        ),
    ))
}

fn format_matching_sources(sources: &[&BoundaryValidationSource]) -> String {
    if sources.is_empty() {
        return "<none>".to_owned();
    }

    sources
        .iter()
        .map(|source| format!("`{}` ({})", source.path.display(), source.kind.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_boundary_coordinates(
    path: &Path,
    diagnostic_block: &Block,
    index: usize,
    boundary_name: &str,
    boundary: SpanBoundary,
    validation_source: &BoundaryValidationSource,
) -> Result<(), FixtureContractError> {
    if boundary_matches_source(boundary, &validation_source.bytes) {
        return Ok(());
    }

    let mut expected = Vec::new();
    if let Some(line) = boundary.line {
        expected.push(format!("line={line}"));
    }
    if let Some(column) = boundary.column {
        expected.push(format!("column={column}"));
    }
    let expected_text = if expected.is_empty() {
        format!("byte={}", boundary.byte)
    } else {
        expected.join(", ")
    };

    let observed = match byte_to_line_column(&validation_source.bytes, boundary.byte) {
        Some((line, column)) => format!("line {line}, column {column}"),
        None => format!(
            "byte {} out of range (len {})",
            boundary.byte,
            validation_source.bytes.len()
        ),
    };

    Err(FixtureContractError::new(
        path,
        format!(
            "diagnostics entry #{index} (`{}`) `{boundary_name}` `{}` does not match byte {} in selected source `{}` ({observed})",
            diagnostic_block.block_type,
            expected_text,
            boundary.byte,
            validation_source.path.display(),
        ),
    ))
}

fn boundary_matches_source(boundary: SpanBoundary, source: &[u8]) -> bool {
    let Some((line, column)) = byte_to_line_column(source, boundary.byte) else {
        return false;
    };

    if let Some(expected_line) = boundary.line
        && expected_line != line
    {
        return false;
    }

    if let Some(expected_column) = boundary.column
        && expected_column != column
    {
        return false;
    }

    true
}

fn byte_to_line_column(source: &[u8], byte: usize) -> Option<(usize, usize)> {
    if byte > source.len() {
        return None;
    }

    let mut line = 1;
    let mut column = 1;
    for value in &source[..byte] {
        if *value == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    Some((line, column))
}

fn parse_message_like_for_block(source: &str, span: Span) -> Vec<String> {
    let Some(block_source) = source.get(span.start..span.end) else {
        return Vec::new();
    };

    for line in block_source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            return parse_message_like(trimmed);
        }
    }

    Vec::new()
}

fn parse_message_like(comment_line: &str) -> Vec<String> {
    let text = comment_line.trim_start_matches('#').trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut snippets = Vec::new();
    let mut remaining = text;
    while let Some(start_quote) = remaining.find('"') {
        let quoted = &remaining[start_quote + 1..];
        let Some(end_quote) = quoted.find('"') else {
            break;
        };
        snippets.push(quoted[..end_quote].to_ascii_lowercase());
        remaining = &quoted[end_quote + 1..];
    }

    if snippets.is_empty() {
        snippets.push(text.trim_end_matches('.').to_ascii_lowercase());
    }

    snippets
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::ast::BodyItem;

    use super::{
        BoundaryValidationSource, ExpectedDiagnostic, FixtureContractError, ValidationSourceKind,
        discover_expression_fixtures_from_root, has_coordinate_expectations,
        load_boundary_validation_sources, parse_diagnostic_entry, parse_expected_diagnostics,
        parse_fixture_contract_ast, parse_top_level_contract, resolve_validation_source,
    };

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let mut path = std::env::temp_dir();
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after UNIX_EPOCH")
                .as_nanos();
            path.push(format!(
                "rshcl-test-fixtures-{}-{timestamp}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("temporary fixture directory should be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn parse_diagnostics_from_source(
        source: &str,
    ) -> Result<Option<Vec<ExpectedDiagnostic>>, FixtureContractError> {
        parse_diagnostics_with_sources(source, &[])
    }

    fn parse_diagnostics_with_sources(
        source: &str,
        validation_sources: &[(&str, &[u8])],
    ) -> Result<Option<Vec<ExpectedDiagnostic>>, FixtureContractError> {
        let path = Path::new("inline-fixture.t");
        let parsed_contract = parse_fixture_contract_ast(path, source)?;
        let top_level = parse_top_level_contract(path, &parsed_contract)?;
        let sources = validation_sources
            .iter()
            .map(|(source_path, bytes)| BoundaryValidationSource {
                kind: if source_path.ends_with(".hcl") {
                    ValidationSourceKind::Hcl
                } else {
                    ValidationSourceKind::Hcldec
                },
                path: PathBuf::from(source_path),
                bytes: bytes.to_vec(),
            })
            .collect::<Vec<_>>();

        parse_expected_diagnostics(path, source, top_level.diagnostics_block, &sources)
    }

    fn expect_contract_error(source: &str) -> FixtureContractError {
        parse_diagnostics_from_source(source).expect_err("contract should fail")
    }

    fn discover_structure_contract_fixtures_from_root(root: &Path) -> Result<Vec<PathBuf>, String> {
        let mut fixtures = Vec::new();
        collect_structure_contract_fixtures(root, &mut fixtures)?;
        if fixtures.is_empty() {
            return Err(format!(
                "no structure contract fixtures were discovered in `{}`",
                root.display()
            ));
        }
        Ok(fixtures)
    }

    fn collect_structure_contract_fixtures(
        root: &Path,
        fixtures: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        let entries = fs::read_dir(root).map_err(|error| {
            format!(
                "failed to read structure contract fixture directory `{}`: {error}",
                root.display()
            )
        })?;

        let mut sorted_paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to read structure contract fixture directory entry under `{}`: {error}",
                    root.display()
                )
            })?;
            sorted_paths.push(entry.path());
        }
        sorted_paths.sort();

        for path in sorted_paths {
            if path.is_dir() {
                collect_structure_contract_fixtures(&path, fixtures)?;
                continue;
            }

            if matches!(path.extension().and_then(|ext| ext.to_str()), Some("t")) {
                fixtures.push(path);
            }
        }

        Ok(())
    }

    #[test]
    fn discovers_expression_fixtures_in_sorted_order() {
        let temp = TempDir::new();

        for name in ["zeta", "alpha", "middle"] {
            let fixture = temp.path().join(format!("{name}.hcl"));
            let contract = temp.path().join(format!("{name}.t"));
            std::fs::write(&fixture, "value = 1\n").expect("fixture source should be written");
            std::fs::write(&contract, "result = null\n")
                .expect("fixture contract should be written");
        }

        let fixtures = discover_expression_fixtures_from_root(temp.path())
            .expect("fixtures should be discovered");
        let names = fixtures
            .iter()
            .map(|fixture| {
                fixture
                    .file_name()
                    .expect("fixture file name should exist")
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["alpha.hcl", "middle.hcl", "zeta.hcl"]);
    }

    #[test]
    fn discover_expression_fixtures_reports_missing_contract_path() {
        let temp = TempDir::new();
        let fixture = temp.path().join("missing_contract.hcl");
        std::fs::write(&fixture, "value = 1\n").expect("fixture source should be written");

        let error = discover_expression_fixtures_from_root(temp.path())
            .expect_err("missing contract should fail");
        let message = error.to_string();

        assert!(
            message.contains("missing sibling fixture contract"),
            "unexpected error message: {message}"
        );
        assert!(
            message.contains("missing_contract.hcl"),
            "unexpected error message: {message}"
        );
        assert!(
            message.contains("missing_contract.t"),
            "unexpected error message: {message}"
        );
    }

    #[test]
    fn discover_expression_fixtures_reports_unreadable_root_path() {
        let temp = TempDir::new();
        let root_file = temp.path().join("fixtures.txt");
        std::fs::write(&root_file, "not a directory").expect("root file should be written");

        let error = discover_expression_fixtures_from_root(&root_file)
            .expect_err("non-directory root should fail");
        let message = error.to_string();
        assert!(
            message.contains("failed to read expression fixture directory"),
            "unexpected error message: {message}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_attribute_entries() {
        let source = r#"
diagnostics {
  error = 1
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("entries must be `error` or `warning` blocks"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_duplicate_diagnostics_block() {
        let source = r#"
diagnostics {
}
diagnostics {
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error.to_string().contains("multiple `diagnostics` blocks"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_missing_from() {
        let source = r#"
diagnostics {
  error {
    to {
      byte = 3
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error.to_string().contains("missing required `from` block"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_missing_to() {
        let source = r#"
diagnostics {
  error {
    from {
      byte = 3
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error.to_string().contains("missing required `to` block"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_labeled_diagnostic_entries() {
        let source = r#"
diagnostics {
  error "label" {
    from {
      byte = 1
    }
    to {
      byte = 2
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error.to_string().contains("must not have labels"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_one_line_nested_blocks() {
        let source = r#"
diagnostics {
  error {
    from { byte = 1 }
    to {
      byte = 2
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error.to_string().contains("contains one-line block `from`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_missing_byte() {
        let source = r#"
diagnostics {
  error {
    from {
    }
    to {
      byte = 3
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("`from` block is missing required `byte` attribute"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_diagnostics_contract_rejects_non_numeric_byte_values() {
        let source = r#"
diagnostics {
  warning {
    from {
      byte = "start"
    }
    to {
      byte = 3
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("`from` `byte` must be an unsigned integer literal"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn message_like_attribute_matches_comment_fallback() {
        let attribute_source = r#"
diagnostics {
  error {
    message_like = ["First snippet", "Second snippet"]
    from {
      byte = 1
    }
    to {
      byte = 2
    }
  }
}
"#;

        let comment_source = r#"
diagnostics {
  error {
    # "First snippet" "Second snippet"
    from {
      byte = 1
    }
    to {
      byte = 2
    }
  }
}
"#;

        let attribute_contract = parse_diagnostics_from_source(attribute_source)
            .expect("attribute contract should parse")
            .expect("diagnostics block should be present");
        let comment_contract = parse_diagnostics_from_source(comment_source)
            .expect("comment contract should parse")
            .expect("diagnostics block should be present");

        assert_eq!(
            attribute_contract[0].message_like,
            comment_contract[0].message_like
        );
        assert_eq!(
            attribute_contract[0].message_like,
            vec!["first snippet", "second snippet"]
        );
    }

    #[test]
    fn message_like_attribute_takes_precedence_over_comment() {
        let source = r#"
diagnostics {
  warning {
    message_like = ["from attribute"]
    # "from comment"
    from {
      byte = 1
    }
    to {
      byte = 2
    }
  }
}
"#;

        let contract = parse_diagnostics_from_source(source)
            .expect("contract should parse")
            .expect("diagnostics block should be present");

        assert_eq!(contract[0].message_like, vec!["from attribute"]);
    }

    #[test]
    fn diagnostics_contract_rejects_from_greater_than_to() {
        let source = r#"
diagnostics {
  error {
    from {
      byte = 10
    }
    to {
      byte = 2
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("must be less than or equal to `to.byte`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_rejects_line_mismatch_for_byte() {
        let source = r#"
diagnostics {
  error {
    from {
      line   = 1
      column = 1
      byte   = 4
    }
    to {
      line   = 2
      column = 3
      byte   = 6
    }
  }
}
"#;

        let error = parse_diagnostics_with_sources(source, &[("fixture.hcl", b"abc\ndef\n")])
            .expect_err("line mismatch should fail");
        assert!(
            error
                .to_string()
                .contains("`from` `line=1, column=1` does not match byte 4"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_rejects_column_mismatch_for_byte() {
        let source = r#"
diagnostics {
  warning {
    from {
      line   = 2
      column = 2
      byte   = 4
    }
    to {
      line   = 2
      column = 3
      byte   = 6
    }
  }
}
"#;

        let error = parse_diagnostics_with_sources(source, &[("fixture.hcl", b"abc\ndef\n")])
            .expect_err("column mismatch should fail");
        assert!(
            error
                .to_string()
                .contains("`from` `line=2, column=2` does not match byte 4"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_accepts_matching_line_column_coordinates() {
        let source = r#"
diagnostics {
  warning {
    from {
      line   = 2
      column = 1
      byte   = 4
    }
    to {
      line   = 2
      column = 3
      byte   = 6
    }
  }
}
"#;

        let contract = parse_diagnostics_with_sources(source, &[("fixture.hcl", b"abc\ndef\n")])
            .expect("matching coordinates should parse")
            .expect("diagnostics block should be present");

        assert_eq!(contract[0].start, 4);
        assert_eq!(contract[0].end, 6);
    }

    #[test]
    fn contract_rejects_duplicate_result_attribute() {
        let source = r#"
result = 1
result = 2
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("duplicate attribute `result` in body"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn contract_rejects_duplicate_result_type_attribute() {
        let source = r#"
result_type = number
result_type = string
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("duplicate attribute `result_type` in body"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn contract_rejects_unknown_top_level_attribute() {
        let source = r#"
unexpected = true
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("unsupported top-level attribute `unexpected`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn contract_rejects_unknown_top_level_block() {
        let source = r#"
unexpected {
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("unsupported top-level block `unexpected`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn contract_rejects_unknown_top_level_one_line_block() {
        let source = r#"
unexpected { value = 1 }
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("unsupported top-level one-line block `unexpected`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_rejects_invalid_source_value() {
        let source = r#"
diagnostics {
  error {
    source = "json"
    from {
      byte = 0
    }
    to {
      byte = 0
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error.to_string().contains("invalid `source` value `json`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_rejects_duplicate_source_attribute() {
        let source = r#"
diagnostics {
  error {
    source = "hcl"
    source = "hcldec"
    from {
      byte = 0
    }
    to {
      byte = 0
    }
  }
}
"#;

        let error = expect_contract_error(source);
        assert!(
            error
                .to_string()
                .contains("duplicate attribute `source` in body"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_rejects_selected_source_that_is_missing() {
        let source = r#"
diagnostics {
  error {
    source = "hcldec"
    from {
      line   = 1
      column = 1
      byte   = 0
    }
    to {
      line   = 1
      column = 1
      byte   = 0
    }
  }
}
"#;

        let error = parse_diagnostics_with_sources(source, &[("fixture.hcl", b"abc\n")])
            .expect_err("missing selected source should fail");
        assert!(
            error.to_string().contains(
                "selects `source = \"hcldec\"`, but sibling source `.hcldec` does not exist"
            ),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_rejects_coordinates_that_match_only_non_selected_source() {
        let source = r#"
diagnostics {
  error {
    source = "hcl"
    from {
      line   = 2
      column = 3
      byte   = 4
    }
    to {
      line   = 2
      column = 4
      byte   = 5
    }
  }
}
"#;

        let error = parse_diagnostics_with_sources(
            source,
            &[
                ("fixture.hcl", b"aaa\nbbb\n"),
                ("fixture.hcldec", b"x\nyyy\n"),
            ],
        )
        .expect_err("coordinates that match only hcldec should fail for source=hcl");
        assert!(
            error
                .to_string()
                .contains("does not match byte 4 in selected source `fixture.hcl`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_accepts_selected_source_coordinates() {
        let source = r#"
diagnostics {
  error {
    source = "hcldec"
    from {
      line   = 2
      column = 3
      byte   = 4
    }
    to {
      line   = 2
      column = 4
      byte   = 5
    }
  }
}
"#;

        let diagnostics = parse_diagnostics_with_sources(
            source,
            &[
                ("fixture.hcl", b"aaa\nbbb\n"),
                ("fixture.hcldec", b"x\nyyy\n"),
            ],
        )
        .expect("selected source should parse")
        .expect("diagnostics block should be present");
        assert_eq!(diagnostics[0].start, 4);
        assert_eq!(diagnostics[0].end, 5);
    }

    #[test]
    fn diagnostics_contract_rejects_when_from_and_to_resolve_to_different_sources() {
        let source = r#"
diagnostics {
  warning {
    from {
      line   = 2
      column = 1
      byte   = 4
    }
    to {
      line   = 2
      column = 4
      byte   = 5
    }
  }
}
"#;

        let error = parse_diagnostics_with_sources(
            source,
            &[
                ("fixture.hcl", b"aaa\nbbb\n"),
                ("fixture.hcldec", b"x\nyyy\n"),
            ],
        )
        .expect_err("mixed-source boundaries should fail");
        assert!(
            error
                .to_string()
                .contains("resolves `from` and `to` to different sibling sources"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn diagnostics_contract_defaults_to_matching_hcldec_source() {
        let source = r#"
diagnostics {
  warning {
    from {
      line   = 2
      column = 3
      byte   = 4
    }
    to {
      line   = 2
      column = 4
      byte   = 5
    }
  }
}
"#;

        let diagnostics = parse_diagnostics_with_sources(
            source,
            &[
                ("fixture.hcl", b"aaa\nbbb\n"),
                ("fixture.hcldec", b"x\nyyy\n"),
            ],
        )
        .expect("default source resolution should parse")
        .expect("diagnostics block should be present");
        assert_eq!(diagnostics[0].start, 4);
        assert_eq!(diagnostics[0].end, 5);
    }

    #[test]
    fn diagnostics_contract_prefers_hcl_when_both_sources_match_unless_source_is_explicit() {
        let default_source = r#"
diagnostics {
  error {
    from {
      line   = 1
      column = 1
      byte   = 0
    }
    to {
      line   = 1
      column = 1
      byte   = 0
    }
  }
}
"#;
        let explicit_override = r#"
diagnostics {
  error {
    source = "hcldec"
    from {
      line   = 1
      column = 1
      byte   = 0
    }
    to {
      line   = 1
      column = 1
      byte   = 0
    }
  }
}
"#;
        let validation_sources = vec![
            BoundaryValidationSource {
                kind: ValidationSourceKind::Hcl,
                path: PathBuf::from("fixture.hcl"),
                bytes: b"x\n".to_vec(),
            },
            BoundaryValidationSource {
                kind: ValidationSourceKind::Hcldec,
                path: PathBuf::from("fixture.hcldec"),
                bytes: b"x\n".to_vec(),
            },
        ];
        let path = Path::new("fixture.t");

        let parsed_default = parse_fixture_contract_ast(path, default_source)
            .expect("default source contract should parse");
        let top_level_default = parse_top_level_contract(path, &parsed_default)
            .expect("default source top-level contract should parse");
        let diagnostics_default = top_level_default
            .diagnostics_block
            .expect("default source diagnostics block should exist");
        let BodyItem::Block(default_entry_block) = &diagnostics_default.body.items[0] else {
            panic!("default source diagnostics entry should be a block")
        };
        let default_entry = parse_diagnostic_entry(path, default_entry_block, 0)
            .expect("default diagnostics entry should parse");
        let default_selected = resolve_validation_source(
            path,
            default_entry_block,
            0,
            default_entry.from,
            default_entry.to,
            default_entry.source,
            &validation_sources,
        )
        .expect("default source should resolve");
        assert_eq!(default_selected.kind, ValidationSourceKind::Hcl);

        let parsed_override = parse_fixture_contract_ast(path, explicit_override)
            .expect("explicit source contract should parse");
        let top_level_override = parse_top_level_contract(path, &parsed_override)
            .expect("explicit source top-level contract should parse");
        let diagnostics_override = top_level_override
            .diagnostics_block
            .expect("explicit source diagnostics block should exist");
        let BodyItem::Block(override_entry_block) = &diagnostics_override.body.items[0] else {
            panic!("explicit source diagnostics entry should be a block")
        };
        let override_entry = parse_diagnostic_entry(path, override_entry_block, 0)
            .expect("explicit diagnostics entry should parse");
        let override_selected = resolve_validation_source(
            path,
            override_entry_block,
            0,
            override_entry.from,
            override_entry.to,
            override_entry.source,
            &validation_sources,
        )
        .expect("explicit source should resolve");
        assert_eq!(override_selected.kind, ValidationSourceKind::Hcldec);
    }

    #[test]
    fn coordinate_validated_mixed_sibling_contracts_require_explicit_source() {
        let root = Path::new("specsuite/tests/structure");
        let contracts = discover_structure_contract_fixtures_from_root(root)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut violations = Vec::new();

        for contract_path in contracts {
            let validation_sources = load_boundary_validation_sources(&contract_path)
                .unwrap_or_else(|error| panic!("{error}"));
            let has_hcl = validation_sources
                .iter()
                .any(|source| source.kind == ValidationSourceKind::Hcl);
            let has_hcldec = validation_sources
                .iter()
                .any(|source| source.kind == ValidationSourceKind::Hcldec);

            if !(has_hcl && has_hcldec) {
                continue;
            }

            let source = fs::read_to_string(&contract_path).unwrap_or_else(|error| {
                panic!(
                    "failed to read fixture contract `{}`: {error}",
                    contract_path.display()
                )
            });
            let parsed_contract = parse_fixture_contract_ast(&contract_path, &source)
                .unwrap_or_else(|error| panic!("{error}"));
            let top_level = parse_top_level_contract(&contract_path, &parsed_contract)
                .unwrap_or_else(|error| panic!("{error}"));
            let Some(diagnostics_block) = top_level.diagnostics_block else {
                continue;
            };

            for (index, item) in diagnostics_block.body.items.iter().enumerate() {
                let BodyItem::Block(diagnostic_block) = item else {
                    continue;
                };
                if diagnostic_block.block_type != "error"
                    && diagnostic_block.block_type != "warning"
                {
                    continue;
                }

                let diagnostic_entry =
                    parse_diagnostic_entry(&contract_path, diagnostic_block, index)
                        .unwrap_or_else(|error| panic!("{error}"));
                let has_coordinates = has_coordinate_expectations(diagnostic_entry.from)
                    || has_coordinate_expectations(diagnostic_entry.to);

                if has_coordinates && diagnostic_entry.source.is_none() {
                    violations.push(format!(
                        "{}: diagnostics entry #{index} (`{}`) has line/column coordinates with both `.hcl` and `.hcldec` siblings but is missing `source`",
                        contract_path.display(),
                        diagnostic_block.block_type,
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "coordinate-validated mixed-sibling diagnostics entries must declare `source`:\n{}",
            violations.join("\n")
        );
    }
}
