use crate::ast::ConfigFile;
use crate::diagnostics::{Diagnostic, Severity};
use crate::eval::{self, EvalContext, Value};
use crate::lexer;
use crate::parser;
use crate::static_analysis;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidateResult {
    pub config: ConfigFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidateResult {
    pub fn has_errors(&self) -> bool {
        diagnostics_have_errors(&self.diagnostics)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EvaluateConfigResult {
    pub config: ConfigFile,
    pub value: Option<Value>,
    pub diagnostics: Vec<Diagnostic>,
}

impl EvaluateConfigResult {
    pub fn has_errors(&self) -> bool {
        diagnostics_have_errors(&self.diagnostics)
    }
}

fn diagnostics_have_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

/// Validates configuration bytes and optionally applies schema diagnostics.
///
/// This is the high-level facade for:
/// 1. `lexer::lex_bytes`
/// 2. `parser::parse`
/// 3. diagnostics accumulation (`lex + parse`)
/// 4. optional schema parse/static-analysis flow through `compose_schema_diagnostics`
///
/// # Examples
///
/// ```
/// use vizier_hcl_rs::api::validate_str;
///
/// let result = validate_str("a = 1\n", None);
/// assert!(!result.has_errors());
/// ```
pub fn validate_bytes(source: &[u8], schema_source: Option<&[u8]>) -> ValidateResult {
    let lexed = lexer::lex_bytes(source);
    let parsed = parser::parse(&lexed.tokens);
    let parser::ParseResult {
        config,
        diagnostics: parser_diagnostics,
    } = parsed;

    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(parser_diagnostics);

    if let Some(schema_source) = schema_source {
        static_analysis::compose_schema_diagnostics(&config, &mut diagnostics, schema_source);
    }

    ValidateResult {
        config,
        diagnostics,
    }
}

/// `&str` wrapper for [`validate_bytes`].
pub fn validate_str(source: &str, schema_source: Option<&str>) -> ValidateResult {
    validate_bytes(source.as_bytes(), schema_source.map(str::as_bytes))
}

/// Validates and then evaluates configuration bytes.
///
/// Evaluation is fail-closed:
/// - if pre-eval diagnostics already contain an error, evaluation is skipped and `value` is `None`.
/// - otherwise, eval diagnostics are appended and `value` is `Some(...)`.
///
/// # Examples
///
/// ```
/// use vizier_hcl_rs::api::evaluate_config_str;
/// use vizier_hcl_rs::eval::EvalContext;
///
/// let source = "name = \"vizier\"\n";
/// let schema = "object {\n  attr \"name\" {\n    type = string\n  }\n}\n";
///
/// let result = evaluate_config_str(source, Some(schema), &EvalContext::default());
/// assert!(!result.has_errors());
/// assert!(result.value.is_some());
/// ```
///
/// Validation can succeed while evaluation still emits diagnostics.
/// In that case, eval diagnostics are appended and `value` remains `Some(...)`.
///
/// ```
/// use vizier_hcl_rs::api::evaluate_config_str;
/// use vizier_hcl_rs::eval::{EvalContext, Value};
///
/// let source = "name = unknown\n";
/// let schema = "object {\n  attr \"name\" {\n    type = string\n  }\n}\n";
///
/// let result = evaluate_config_str(source, Some(schema), &EvalContext::default());
/// assert!(result.has_errors());
/// assert!(result.value.is_some());
/// assert!(result
///     .diagnostics
///     .iter()
///     .any(|diagnostic| diagnostic.message.contains("unknown variable `unknown` in evaluation")));
/// assert!(matches!(
///     result.value.as_ref(),
///     Some(Value::Object(values)) if matches!(values.get("name"), Some(Value::Null))
/// ));
/// ```
pub fn evaluate_config_bytes(
    source: &[u8],
    schema_source: Option<&[u8]>,
    context: &EvalContext,
) -> EvaluateConfigResult {
    let validated = validate_bytes(source, schema_source);
    let ValidateResult {
        config,
        diagnostics,
    } = validated;

    if diagnostics_have_errors(&diagnostics) {
        return EvaluateConfigResult {
            config,
            value: None,
            diagnostics,
        };
    }

    let eval_result = eval::evaluate_config(&config, context);
    let mut diagnostics = diagnostics;
    diagnostics.extend(eval_result.diagnostics);

    EvaluateConfigResult {
        config,
        value: Some(eval_result.value),
        diagnostics,
    }
}

/// `&str` wrapper for [`evaluate_config_bytes`].
pub fn evaluate_config_str(
    source: &str,
    schema_source: Option<&str>,
    context: &EvalContext,
) -> EvaluateConfigResult {
    evaluate_config_bytes(source.as_bytes(), schema_source.map(str::as_bytes), context)
}
