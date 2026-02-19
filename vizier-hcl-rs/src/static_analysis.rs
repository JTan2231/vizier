use std::collections::{HashMap, HashSet};

use crate::ast::{
    Attribute, Block, BlockLabel, BodyItem, ConfigFile, Expression, LiteralValue, OneLineBlock,
    TemplateSegment,
};
use crate::diagnostics::{Diagnostic, Severity, Span};
use crate::lexer::lex_bytes;
use crate::parser::parse;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    root: Option<ObjectSchema>,
}

impl Schema {
    fn unconstrained() -> Self {
        Self { root: None }
    }

    fn from_root(root: ObjectSchema) -> Self {
        Self { root: Some(root) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ObjectSchema {
    attributes: Vec<AttributeSchema>,
    blocks: Vec<BlockSchema>,
}

impl ObjectSchema {
    fn attribute(&self, name: &str) -> Option<&AttributeSchema> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
    }

    fn block(&self, block_type: &str) -> Option<&BlockSchema> {
        self.blocks
            .iter()
            .find(|block| block.block_type == block_type)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttributeSchema {
    name: String,
    required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockSchema {
    block_type: String,
    cardinality: BlockCardinality,
    required: bool,
    object: ObjectSchema,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockCardinality {
    Single,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchemaParseResult {
    pub schema: Schema,
    pub diagnostics: Vec<Diagnostic>,
}

impl SchemaParseResult {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub fn parse_schema_bytes(source: &[u8]) -> SchemaParseResult {
    let lexed = lex_bytes(source);
    let parsed = parse(&lexed.tokens);

    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(parsed.diagnostics);

    let mut schema_diagnostics = Vec::new();
    let schema = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        Schema::unconstrained()
    } else {
        extract_schema(&parsed.config, &mut schema_diagnostics)
    };

    diagnostics.extend(schema_diagnostics);

    SchemaParseResult {
        schema,
        diagnostics,
    }
}

pub fn parse_schema_str(source: &str) -> SchemaParseResult {
    parse_schema_bytes(source.as_bytes())
}

pub fn compose_schema_diagnostics(
    config: &ConfigFile,
    diagnostics: &mut Vec<Diagnostic>,
    schema_source: &[u8],
) {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return;
    }

    let schema_result = parse_schema_bytes(schema_source);
    let schema_has_errors = schema_result.has_errors();
    diagnostics.extend(schema_result.diagnostics);
    if !schema_has_errors {
        diagnostics.extend(analyze(config, &schema_result.schema));
    }
}

pub fn analyze(config: &ConfigFile, schema: &Schema) -> Vec<Diagnostic> {
    let Some(root) = &schema.root else {
        return Vec::new();
    };

    let mut diagnostics = Vec::new();
    let top_level_missing_required_span = top_level_missing_required_span(config);
    analyze_body_items(
        &config.body.items,
        root,
        top_level_missing_required_span,
        None,
        &mut diagnostics,
    );
    diagnostics
}

fn analyze_body_items(
    items: &[BodyItem],
    schema: &ObjectSchema,
    missing_required_span: Span,
    block_type_context: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen_attributes = HashSet::new();
    let mut seen_blocks = HashMap::new();

    for item in items {
        match item {
            BodyItem::Attribute(attribute) => {
                seen_attributes.insert(attribute.name.as_str());
                if schema.attribute(&attribute.name).is_none() {
                    diagnostics.push(Diagnostic::error(
                        format!(
                            "an argument named \"{}\" is not expected here",
                            attribute.name
                        ),
                        attribute_name_span(attribute),
                    ));
                }
            }
            BodyItem::Block(block) => {
                record_block_usage(
                    &block.block_type,
                    block_type_span(block.span.start, &block.block_type),
                    schema,
                    &mut seen_blocks,
                    diagnostics,
                );
                analyze_block(
                    &block.block_type,
                    block.span.start,
                    &block.body.items,
                    schema,
                    diagnostics,
                );
            }
            BodyItem::OneLineBlock(block) => {
                record_block_usage(
                    &block.block_type,
                    one_line_block_type_span(block),
                    schema,
                    &mut seen_blocks,
                    diagnostics,
                );
                analyze_one_line_block(block, schema, diagnostics);
            }
        }
    }

    for attribute in &schema.attributes {
        if attribute.required && !seen_attributes.contains(attribute.name.as_str()) {
            diagnostics.push(Diagnostic::error(
                missing_required_attribute_message(&attribute.name, block_type_context),
                missing_required_span,
            ));
        }
    }

    for block in &schema.blocks {
        if block.required && !seen_blocks.contains_key(block.block_type.as_str()) {
            diagnostics.push(Diagnostic::error(
                missing_required_block_message(&block.block_type, block_type_context),
                missing_required_span,
            ));
        }
    }
}

fn record_block_usage(
    block_type: &str,
    span: Span,
    schema: &ObjectSchema,
    seen_blocks: &mut HashMap<String, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(block_schema) = schema.block(block_type) else {
        return;
    };

    let count = seen_blocks.entry(block_type.to_owned()).or_default();
    *count += 1;

    if block_schema.cardinality == BlockCardinality::Single && *count > 1 {
        diagnostics.push(Diagnostic::error(
            format!("duplicate block \"{block_type}\" in this body"),
            span,
        ));
    }
}

fn analyze_block(
    block_type: &str,
    block_start: usize,
    items: &[BodyItem],
    schema: &ObjectSchema,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(block_schema) = schema.block(block_type) else {
        diagnostics.push(Diagnostic::error(
            format!("a block named \"{block_type}\" is not expected here"),
            block_type_span(block_start, block_type),
        ));
        return;
    };

    let nested_block_span = block_type_span(block_start, block_type);
    analyze_body_items(
        items,
        &block_schema.object,
        nested_block_span,
        Some(block_type),
        diagnostics,
    );
}

fn analyze_one_line_block(
    block: &OneLineBlock,
    schema: &ObjectSchema,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(block_schema) = schema.block(&block.block_type) else {
        diagnostics.push(Diagnostic::error(
            format!(
                "a block named \"{}\" is not expected here",
                block.block_type
            ),
            one_line_block_type_span(block),
        ));
        return;
    };

    let mut seen_attributes = HashSet::new();

    if let Some(attribute) = &block.attribute {
        seen_attributes.insert(attribute.name.as_str());
        if block_schema.object.attribute(&attribute.name).is_none() {
            diagnostics.push(Diagnostic::error(
                format!(
                    "an argument named \"{}\" is not expected here",
                    attribute.name
                ),
                attribute_name_span(attribute),
            ));
        }
    }

    for attribute in &block_schema.object.attributes {
        if attribute.required && !seen_attributes.contains(attribute.name.as_str()) {
            diagnostics.push(Diagnostic::error(
                missing_required_attribute_message(&attribute.name, Some(&block.block_type)),
                one_line_block_type_span(block),
            ));
        }
    }

    for nested_block in &block_schema.object.blocks {
        if nested_block.required {
            diagnostics.push(Diagnostic::error(
                missing_required_block_message(&nested_block.block_type, Some(&block.block_type)),
                one_line_block_type_span(block),
            ));
        }
    }
}

fn missing_required_attribute_message(attribute: &str, block_type_context: Option<&str>) -> String {
    match block_type_context {
        Some(block_type) => {
            format!("missing required argument \"{attribute}\" in block \"{block_type}\"")
        }
        None => format!("missing required argument \"{attribute}\""),
    }
}

fn missing_required_block_message(block_type: &str, block_type_context: Option<&str>) -> String {
    match block_type_context {
        Some(parent_block_type) => {
            format!("missing required block \"{block_type}\" in block \"{parent_block_type}\"")
        }
        None => format!("missing required block \"{block_type}\""),
    }
}

fn extract_schema(config: &ConfigFile, diagnostics: &mut Vec<Diagnostic>) -> Schema {
    let Some(root_item) = config.body.items.first() else {
        diagnostics.push(Diagnostic::error(
            "schema root must be a single `object`, `block`, or `block_list` block",
            Span::default(),
        ));
        return Schema::unconstrained();
    };

    if config.body.items.len() != 1 {
        diagnostics.push(Diagnostic::error(
            "schema root must be a single `object`, `block`, or `block_list` block",
            item_span(root_item),
        ));
        return Schema::unconstrained();
    }

    let BodyItem::Block(root_block) = root_item else {
        diagnostics.push(Diagnostic::error(
            "schema root must be a block declaration",
            item_span(root_item),
        ));
        return Schema::unconstrained();
    };

    match root_block.block_type.as_str() {
        "object" => {
            if !root_block.labels.is_empty() {
                diagnostics.push(Diagnostic::error(
                    "schema `object` block must not include labels",
                    root_block.span,
                ));
                return Schema::unconstrained();
            }

            Schema::from_root(extract_object_schema(&root_block.body.items, diagnostics))
        }
        "block" | "block_list" => {
            let Some(block_schema) = extract_schema_block_declaration(root_block, diagnostics)
            else {
                return Schema::unconstrained();
            };

            let root = ObjectSchema {
                attributes: Vec::new(),
                blocks: vec![block_schema],
            };
            Schema::from_root(root)
        }
        _ => {
            diagnostics.push(Diagnostic::error(
                format!(
                    "unsupported schema root block `{}`; expected `object`, `block`, or `block_list`",
                    root_block.block_type
                ),
                block_type_span(root_block.span.start, &root_block.block_type),
            ));
            Schema::unconstrained()
        }
    }
}

fn extract_object_schema(items: &[BodyItem], diagnostics: &mut Vec<Diagnostic>) -> ObjectSchema {
    let mut schema = ObjectSchema::default();

    for item in items {
        let BodyItem::Block(block) = item else {
            diagnostics.push(Diagnostic::error(
                "schema `object` entries must be blocks",
                item_span(item),
            ));
            continue;
        };

        match block.block_type.as_str() {
            "attr" => {
                let Some(attribute_schema) = extract_schema_attribute(block, diagnostics) else {
                    continue;
                };

                if schema.attribute(&attribute_schema.name).is_some() {
                    diagnostics.push(Diagnostic::error(
                        format!("duplicate schema attribute `{}`", attribute_schema.name),
                        block.span,
                    ));
                    continue;
                }

                schema.attributes.push(attribute_schema);
            }
            "block" | "block_list" => {
                let Some(block_schema) = extract_schema_block_declaration(block, diagnostics)
                else {
                    continue;
                };

                if schema.block(&block_schema.block_type).is_some() {
                    diagnostics.push(Diagnostic::error(
                        format!("duplicate schema block `{}`", block_schema.block_type),
                        block.span,
                    ));
                    continue;
                }

                schema.blocks.push(block_schema);
            }
            _ => diagnostics.push(Diagnostic::error(
                format!("unsupported schema declaration `{}`", block.block_type),
                block_type_span(block.span.start, &block.block_type),
            )),
        }
    }

    schema
}

fn extract_schema_attribute(
    block: &Block,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<AttributeSchema> {
    let name = extract_schema_attribute_name(block, diagnostics)?;

    let mut required = false;
    let mut required_seen = false;

    for item in &block.body.items {
        let BodyItem::Attribute(attribute) = item else {
            continue;
        };

        if attribute.name != "required" {
            continue;
        }

        if required_seen {
            diagnostics.push(Diagnostic::error(
                "schema `attr` block must not define `required` more than once",
                attribute_name_span(attribute),
            ));
            continue;
        }

        required_seen = true;
        match extract_bool_literal(&attribute.expression) {
            Some(value) => required = value,
            None => diagnostics.push(Diagnostic::error(
                "schema `required` argument must be a boolean literal",
                attribute.span,
            )),
        }
    }

    Some(AttributeSchema { name, required })
}

fn extract_schema_block_declaration(
    block: &Block,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BlockSchema> {
    let cardinality = match block.block_type.as_str() {
        "block" => BlockCardinality::Single,
        "block_list" => BlockCardinality::List,
        _ => {
            diagnostics.push(Diagnostic::error(
                format!("unsupported schema declaration `{}`", block.block_type),
                block_type_span(block.span.start, &block.block_type),
            ));
            return None;
        }
    };

    let labeled_block_type = extract_schema_block_label(block, diagnostics);

    let mut attribute_block_type = None;
    let mut required = false;
    let mut required_seen = false;
    let mut object_schema = None;

    for item in &block.body.items {
        match item {
            BodyItem::Attribute(attribute) => {
                match attribute.name.as_str() {
                    "block_type" => {
                        if attribute_block_type.is_some() {
                            diagnostics.push(Diagnostic::error(
                                format!(
                                    "schema `{}` block must not define `block_type` more than once",
                                    block.block_type
                                ),
                                attribute_name_span(attribute),
                            ));
                            continue;
                        }

                        match extract_string_literal(&attribute.expression) {
                            Some(value) => attribute_block_type = Some(value),
                            None => diagnostics.push(Diagnostic::error(
                                "schema `block_type` argument must be a string literal",
                                attribute.span,
                            )),
                        }
                    }
                    "required" => {
                        if required_seen {
                            diagnostics.push(Diagnostic::error(
                                format!(
                                    "schema `{}` block must not define `required` more than once",
                                    block.block_type
                                ),
                                attribute_name_span(attribute),
                            ));
                            continue;
                        }

                        required_seen = true;
                        match extract_bool_literal(&attribute.expression) {
                            Some(value) => required = value,
                            None => diagnostics.push(Diagnostic::error(
                                "schema `required` argument must be a boolean literal",
                                attribute.span,
                            )),
                        }
                    }
                    _ => diagnostics.push(Diagnostic::error(
                        format!(
                            "schema `{}` block body only supports `block_type`, `required`, and nested `object`",
                            block.block_type
                        ),
                        attribute_name_span(attribute),
                    )),
                }
            }
            BodyItem::Block(inner_block) if inner_block.block_type == "object" => {
                if object_schema.is_some() {
                    diagnostics.push(Diagnostic::error(
                        format!(
                            "schema `{}` block must include only one nested `object` block",
                            block.block_type
                        ),
                        inner_block.span,
                    ));
                    continue;
                }

                if !inner_block.labels.is_empty() {
                    diagnostics.push(Diagnostic::error(
                        "schema `object` block must not include labels",
                        inner_block.span,
                    ));
                }

                object_schema = Some(extract_object_schema(&inner_block.body.items, diagnostics));
            }
            BodyItem::OneLineBlock(inner_block) if inner_block.block_type == "object" => {
                if object_schema.is_some() {
                    diagnostics.push(Diagnostic::error(
                        format!(
                            "schema `{}` block must include only one nested `object` block",
                            block.block_type
                        ),
                        inner_block.span,
                    ));
                    continue;
                }

                if !inner_block.labels.is_empty() {
                    diagnostics.push(Diagnostic::error(
                        "schema `object` block must not include labels",
                        inner_block.span,
                    ));
                }

                if inner_block.attribute.is_some() {
                    diagnostics.push(Diagnostic::error(
                        "schema `object` block must be empty in single-line form",
                        inner_block.span,
                    ));
                }

                object_schema = Some(ObjectSchema::default());
            }
            _ => diagnostics.push(Diagnostic::error(
                format!(
                    "schema `{}` block body only supports `block_type`, `required`, and nested `object`",
                    block.block_type
                ),
                item_span(item),
            )),
        }
    }

    let block_type = match (labeled_block_type, attribute_block_type) {
        (Some(label), Some(attribute)) => {
            if label != attribute {
                diagnostics.push(Diagnostic::error(
                    format!(
                        "schema `{}` block label `{label}` does not match `block_type` value `{attribute}`",
                        block.block_type
                    ),
                    block.span,
                ));
            }
            label
        }
        (Some(label), None) => label,
        (None, Some(attribute)) => attribute,
        (None, None) => {
            diagnostics.push(Diagnostic::error(
                format!(
                    "schema `{}` block must declare a block type via label or `block_type`",
                    block.block_type
                ),
                block.span,
            ));
            return None;
        }
    };

    let object = if let Some(object_schema) = object_schema {
        object_schema
    } else {
        diagnostics.push(Diagnostic::error(
            format!(
                "schema `{}` block must include a nested `object` block",
                block.block_type
            ),
            block.span,
        ));
        ObjectSchema::default()
    };

    Some(BlockSchema {
        block_type,
        cardinality,
        required,
        object,
    })
}

fn extract_schema_block_label(block: &Block, diagnostics: &mut Vec<Diagnostic>) -> Option<String> {
    if block.labels.is_empty() {
        return None;
    }

    if block.labels.len() != 1 {
        diagnostics.push(Diagnostic::error(
            format!(
                "schema `{}` block must have at most one label",
                block.block_type
            ),
            block.span,
        ));
        return None;
    }

    match &block.labels[0] {
        BlockLabel::StringLiteral(label) => Some(label.clone()),
        BlockLabel::Identifier(label) => Some(label.clone()),
    }
}

fn extract_schema_attribute_name(
    block: &Block,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    if block.labels.len() != 1 {
        diagnostics.push(Diagnostic::error(
            "schema `attr` block must have exactly one string label",
            block.span,
        ));
        return None;
    }

    match &block.labels[0] {
        BlockLabel::StringLiteral(name) => Some(name.clone()),
        BlockLabel::Identifier(_) => {
            diagnostics.push(Diagnostic::error(
                "schema `attr` block label must be a string literal",
                block.span,
            ));
            None
        }
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

fn extract_bool_literal(expression: &Expression) -> Option<bool> {
    let Expression::Literal(literal) = expression else {
        return None;
    };

    match literal.value {
        LiteralValue::Bool(value) => Some(value),
        _ => None,
    }
}

fn item_span(item: &BodyItem) -> Span {
    match item {
        BodyItem::Attribute(attribute) => attribute.span,
        BodyItem::Block(block) => block.span,
        BodyItem::OneLineBlock(block) => block.span,
    }
}

fn top_level_missing_required_span(config: &ConfigFile) -> Span {
    match config.body.items.first() {
        Some(BodyItem::Attribute(attribute)) => attribute_name_span(attribute),
        Some(BodyItem::Block(block)) => block_type_span(block.span.start, &block.block_type),
        Some(BodyItem::OneLineBlock(block)) => one_line_block_type_span(block),
        None => Span::default(),
    }
}

fn block_type_span(start: usize, block_type: &str) -> Span {
    Span::new(start, start.saturating_add(block_type.len()))
}

fn one_line_block_type_span(block: &OneLineBlock) -> Span {
    block_type_span(block.span.start, &block.block_type)
}

fn attribute_name_span(attribute: &Attribute) -> Span {
    Span::new(
        attribute.span.start,
        attribute.span.start.saturating_add(attribute.name.len()),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::diagnostics::{Diagnostic, Severity, Span};
    use crate::lexer::lex_str;
    use crate::parser::parse;

    use super::{analyze, compose_schema_diagnostics, parse_schema_str};

    fn parse_config(source: &str) -> crate::ast::ConfigFile {
        let lexed = lex_str(source);
        assert!(
            lexed.diagnostics.is_empty(),
            "unexpected lexer diagnostics: {:#?}",
            lexed.diagnostics
        );

        let parsed = parse(&lexed.tokens);
        assert!(
            parsed.diagnostics.is_empty(),
            "unexpected parser diagnostics: {:#?}",
            parsed.diagnostics
        );

        parsed.config
    }

    #[test]
    fn allows_declared_top_level_attributes() {
        let schema = parse_schema_str(
            "object {\n  attr \"a\" {\n    type = string\n  }\n  attr \"b\" {\n    type = string\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a = 1\nb = 2\n");
        let diagnostics = analyze(&config, &schema.schema);
        assert!(diagnostics.is_empty(), "{:#?}", diagnostics);
    }

    #[test]
    fn extracts_block_schema_declarations_from_block_fixtures() {
        let schema_source =
            fs::read_to_string("specsuite/tests/structure/blocks/single_oneline_invalid.hcldec")
                .expect("schema fixture should exist");

        let schema = parse_schema_str(&schema_source);
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);
    }

    #[test]
    fn reports_unsupported_schema_root_with_deterministic_span() {
        let schema = parse_schema_str("literal {\n  value = null\n}\n");

        assert!(schema.has_errors());
        assert_eq!(schema.diagnostics.len(), 1, "{:#?}", schema.diagnostics);
        assert!(
            schema.diagnostics[0]
                .message
                .contains("unsupported schema root block `literal`"),
            "unexpected message: {}",
            schema.diagnostics[0].message
        );
        assert_eq!(schema.diagnostics[0].span, Span::new(0, 7));
    }

    #[test]
    fn compose_schema_diagnostics_skips_schema_parse_when_config_has_errors() {
        let config = parse_config("known = 1\n");
        let mut diagnostics = vec![Diagnostic::error("existing config error", Span::new(3, 5))];

        compose_schema_diagnostics(&config, &mut diagnostics, b"literal {\n  value = null\n}\n");

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].message, "existing config error");
        assert_eq!(diagnostics[0].span, Span::new(3, 5));
    }

    #[test]
    fn compose_schema_diagnostics_emits_schema_errors_and_skips_analysis() {
        let config = parse_config("z = 1\n");
        let mut diagnostics = Vec::new();

        compose_schema_diagnostics(
            &config,
            &mut diagnostics,
            b"object {\n  attr \"a\" {\n    type = string\n  }\n  attr \"a\" {\n    type = string\n  }\n}\n",
        );

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert!(
            diagnostics[0]
                .message
                .contains("duplicate schema attribute `a`"),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn compose_schema_diagnostics_runs_analysis_when_schema_is_valid() {
        let config = parse_config("z = 1\n");
        let mut diagnostics = Vec::new();

        compose_schema_diagnostics(
            &config,
            &mut diagnostics,
            b"object {\n  attr \"a\" {\n    type = string\n  }\n}\n",
        );

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 1));
        assert!(
            diagnostics[0]
                .message
                .contains("an argument named \"z\" is not expected here"),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_unexpected_attribute_with_deterministic_span() {
        let config_source =
            fs::read_to_string("specsuite/tests/structure/attributes/unexpected.hcl")
                .expect("fixture should exist");
        let schema_source =
            fs::read_to_string("specsuite/tests/structure/attributes/unexpected.hcldec")
                .expect("schema fixture should exist");

        let config = parse_config(&config_source);
        let schema = parse_schema_str(&schema_source);
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let diagnostics = analyze(&config, &schema.schema);
        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(28, 29));
        assert!(
            diagnostics[0].message.contains("\"c\""),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_unexpected_block_type_with_deterministic_span() {
        let schema = parse_schema_str("block {\n  block_type = \"a\"\n  object {}\n}\n");
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("b {}\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 1));
        assert!(
            diagnostics[0]
                .message
                .contains("a block named \"b\" is not expected here"),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_missing_required_nested_attribute_for_block() {
        let schema = parse_schema_str(
            "block {\n  block_type = \"a\"\n  object {\n    attr \"b\" {\n      required = true\n      type     = string\n    }\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a {}\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 1));
        assert!(
            diagnostics[0]
                .message
                .contains("missing required argument \"b\" in block \"a\""),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_missing_required_top_level_attribute_with_deterministic_anchor() {
        let schema = parse_schema_str(
            "object {\n  attr \"present\" {\n    type = string\n  }\n  attr \"req\" {\n    required = true\n    type = string\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("present = 1\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 7));
        assert!(
            diagnostics[0]
                .message
                .contains("missing required argument \"req\""),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_missing_required_top_level_block_with_deterministic_anchor() {
        let schema = parse_schema_str(
            "object {\n  attr \"present\" {\n    type = string\n  }\n  block \"svc\" {\n    required = true\n    object {}\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("present = 1\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 7));
        assert!(
            diagnostics[0]
                .message
                .contains("missing required block \"svc\""),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_duplicate_singleton_block_instances() {
        let schema = parse_schema_str("object {\n  block \"a\" {\n    object {}\n  }\n}\n");
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a {}\na {}\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(5, 6));
        assert!(
            diagnostics[0]
                .message
                .contains("duplicate block \"a\" in this body"),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn allows_multiple_block_list_instances() {
        let schema = parse_schema_str("object {\n  block_list \"a\" {\n    object {}\n  }\n}\n");
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a {}\na {}\n");
        let diagnostics = analyze(&config, &schema.schema);
        assert!(diagnostics.is_empty(), "{:#?}", diagnostics);
    }

    #[test]
    fn reports_missing_required_nested_block_for_multiline_block() {
        let schema = parse_schema_str(
            "block \"a\" {\n  object {\n    block \"d\" {\n      required = true\n      object {}\n    }\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a {}\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 1));
        assert!(
            diagnostics[0]
                .message
                .contains("missing required block \"d\" in block \"a\""),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn reports_missing_required_nested_block_for_oneline_block() {
        let schema = parse_schema_str(
            "block \"a\" {\n  object {\n    attr \"b\" {\n      type = string\n    }\n    block \"d\" {\n      required = true\n      object {}\n    }\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a { b = \"ok\" }\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 1, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(0, 1));
        assert!(
            diagnostics[0]
                .message
                .contains("missing required block \"d\" in block \"a\""),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn preserves_deterministic_order_for_missing_required_items() {
        let schema = parse_schema_str(
            "block \"a\" {\n  object {\n    attr \"x\" {\n      required = true\n      type = string\n    }\n    block \"d\" {\n      required = true\n      object {}\n    }\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("a {}\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 2, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[1].severity, Severity::Error);
        assert!(
            diagnostics[0]
                .message
                .contains("missing required argument \"x\" in block \"a\""),
            "unexpected first message: {}",
            diagnostics[0].message
        );
        assert!(
            diagnostics[1]
                .message
                .contains("missing required block \"d\" in block \"a\""),
            "unexpected second message: {}",
            diagnostics[1].message
        );
    }

    #[test]
    fn preserves_deterministic_order_for_duplicate_and_missing_required_items() {
        let schema = parse_schema_str(
            "object {\n  attr \"req\" {\n    required = true\n    type = string\n  }\n  block \"dup\" {\n    object {}\n  }\n  block \"needed\" {\n    required = true\n    object {}\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);

        let config = parse_config("dup {}\ndup {}\n");
        let diagnostics = analyze(&config, &schema.schema);

        assert_eq!(diagnostics.len(), 3, "{:#?}", diagnostics);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[1].severity, Severity::Error);
        assert_eq!(diagnostics[2].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Span::new(7, 10));
        assert_eq!(diagnostics[1].span, Span::new(0, 3));
        assert_eq!(diagnostics[2].span, Span::new(0, 3));
        assert!(
            diagnostics[0]
                .message
                .contains("duplicate block \"dup\" in this body"),
            "unexpected first message: {}",
            diagnostics[0].message
        );
        assert!(
            diagnostics[1]
                .message
                .contains("missing required argument \"req\""),
            "unexpected second message: {}",
            diagnostics[1].message
        );
        assert!(
            diagnostics[2]
                .message
                .contains("missing required block \"needed\""),
            "unexpected third message: {}",
            diagnostics[2].message
        );
    }

    #[test]
    fn supports_label_bearing_block_declarations() {
        let schema = parse_schema_str(
            "object {\n  block \"a\" {\n    object {}\n  }\n  block_list list_b {\n    required = true\n    object {}\n  }\n}\n",
        );
        assert!(schema.diagnostics.is_empty(), "{:#?}", schema.diagnostics);
    }

    #[test]
    fn reports_invalid_block_schema_declaration_shapes() {
        let schema = parse_schema_str(
            "object {\n  block \"a\" {\n    required = \"yes\"\n    nope = true\n    object {}\n  }\n}\n",
        );

        assert_eq!(schema.diagnostics.len(), 2, "{:#?}", schema.diagnostics);
        assert!(schema.has_errors());
        assert!(
            schema.diagnostics[0]
                .message
                .contains("schema `required` argument must be a boolean literal"),
            "unexpected message: {}",
            schema.diagnostics[0].message
        );
        assert!(
            schema.diagnostics[1]
                .message
                .contains("schema `block` block body only supports `block_type`, `required`, and nested `object`"),
            "unexpected message: {}",
            schema.diagnostics[1].message
        );
    }

    #[test]
    fn reports_duplicate_schema_attribute_declarations() {
        let schema = parse_schema_str(
            "object {\n  attr \"a\" {\n    type = string\n  }\n  attr \"a\" {\n    type = string\n  }\n}\n",
        );

        assert_eq!(schema.diagnostics.len(), 1, "{:#?}", schema.diagnostics);
        assert_eq!(schema.diagnostics[0].severity, Severity::Error);
        assert!(
            schema.diagnostics[0]
                .message
                .contains("duplicate schema attribute `a`"),
            "unexpected message: {}",
            schema.diagnostics[0].message
        );
    }

    #[test]
    fn reports_malformed_schema_documents() {
        let malformed_syntax = parse_schema_str("object {\n");
        assert!(malformed_syntax.has_errors());
        assert!(
            malformed_syntax
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("expected `}` to close block") })
        );

        let malformed_attr = parse_schema_str("object {\n  attr a {\n    type = string\n  }\n}\n");
        assert!(malformed_attr.has_errors());
        assert!(malformed_attr.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("schema `attr` block label must be a string literal")
        }));
    }
}
