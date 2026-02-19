use std::collections::BTreeMap;

use crate::ast::{
    BinaryExpr, BinaryOperator, Block, BlockLabel, Body, BodyItem, ConditionalExpr, ConfigFile,
    Expression, ForExpr, ForExprKind, FunctionCallExpr, LiteralValue, ObjectKey, OneLineBlock,
    TemplateExpr, TraversalExpr, TraversalOperation, UnaryOperator,
};
use crate::diagnostics::{Diagnostic, Span};
use crate::template;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Tuple(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    Null,
    Bool,
    Number,
    String,
    Tuple,
    Object,
}

impl Value {
    pub fn value_type(&self) -> ValueType {
        match self {
            Value::Null => ValueType::Null,
            Value::Bool(_) => ValueType::Bool,
            Value::Number(_) => ValueType::Number,
            Value::String(_) => ValueType::String,
            Value::Tuple(_) => ValueType::Tuple,
            Value::Object(_) => ValueType::Object,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub variables: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalResult {
    pub value: Value,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn evaluate_config(config: &ConfigFile, context: &EvalContext) -> EvalResult {
    let mut evaluator = Evaluator::new(context);
    let object = evaluator.evaluate_body(&config.body);
    EvalResult {
        value: Value::Object(object),
        diagnostics: evaluator.diagnostics,
    }
}

pub fn evaluate_expression(expression: &Expression, context: &EvalContext) -> EvalResult {
    let mut evaluator = Evaluator::new(context);
    let value = evaluator.evaluate_expression(expression);
    EvalResult {
        value,
        diagnostics: evaluator.diagnostics,
    }
}

pub(crate) struct Evaluator<'a> {
    context: &'a EvalContext,
    scopes: Vec<BTreeMap<String, Value>>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Evaluator<'a> {
    fn new(context: &'a EvalContext) -> Self {
        Self {
            context,
            scopes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn evaluate_body(&mut self, body: &Body) -> BTreeMap<String, Value> {
        let mut object = BTreeMap::new();

        for item in &body.items {
            match item {
                BodyItem::Attribute(attribute) => {
                    let value = self.evaluate_expression(&attribute.expression);
                    object.insert(attribute.name.clone(), value);
                }
                BodyItem::Block(block) => self.evaluate_block(block, &mut object),
                BodyItem::OneLineBlock(block) => self.evaluate_one_line_block(block, &mut object),
            }
        }

        object
    }

    fn evaluate_block(&mut self, block: &Block, object: &mut BTreeMap<String, Value>) {
        let value = Value::Object(self.evaluate_body(&block.body));
        self.insert_block_value(&block.block_type, &block.labels, block.span, value, object);
    }

    fn evaluate_one_line_block(
        &mut self,
        block: &OneLineBlock,
        object: &mut BTreeMap<String, Value>,
    ) {
        let mut body = BTreeMap::new();
        if let Some(attribute) = &block.attribute {
            let value = self.evaluate_expression(&attribute.expression);
            body.insert(attribute.name.clone(), value);
        }

        self.insert_block_value(
            &block.block_type,
            &block.labels,
            block.span,
            Value::Object(body),
            object,
        );
    }

    fn insert_block_value(
        &mut self,
        block_type: &str,
        labels: &[BlockLabel],
        span: Span,
        value: Value,
        object: &mut BTreeMap<String, Value>,
    ) {
        match labels {
            [] => {
                if object.insert(block_type.to_owned(), value).is_some() {
                    self.error(
                        format!("duplicate block `{block_type}` encountered during evaluation"),
                        span,
                    );
                }
            }
            [label] => {
                let label_value = block_label_to_string(label);
                let entry = object
                    .entry(block_type.to_owned())
                    .or_insert_with(|| Value::Object(BTreeMap::new()));
                let Value::Object(map) = entry else {
                    self.error(
                        format!(
                            "cannot add labeled block `{block_type}` because `{block_type}` is already an attribute"
                        ),
                        span,
                    );
                    return;
                };

                if map.insert(label_value.clone(), value).is_some() {
                    self.error(
                        format!(
                            "duplicate labeled block `{block_type}` with label `{label_value}` encountered during evaluation"
                        ),
                        span,
                        );
                }
            }
            _ => self.insert_multi_label_block_value(block_type, labels, span, value, object),
        }
    }

    fn insert_multi_label_block_value(
        &mut self,
        block_type: &str,
        labels: &[BlockLabel],
        span: Span,
        value: Value,
        object: &mut BTreeMap<String, Value>,
    ) {
        let label_path = labels.iter().map(block_label_to_string).collect::<Vec<_>>();
        let full_label_path = label_path.join(".");

        let entry = object
            .entry(block_type.to_owned())
            .or_insert_with(|| Value::Object(BTreeMap::new()));
        let Value::Object(map) = entry else {
            self.error(
                format!(
                    "cannot add labeled block `{block_type}` because `{block_type}` is already an attribute"
                ),
                span,
            );
            return;
        };

        self.insert_label_path_value(block_type, &full_label_path, &label_path, span, value, map);
    }

    fn insert_label_path_value(
        &mut self,
        block_type: &str,
        full_label_path: &str,
        path: &[String],
        span: Span,
        value: Value,
        object: &mut BTreeMap<String, Value>,
    ) {
        let Some((segment, remainder)) = path.split_first() else {
            return;
        };

        if remainder.is_empty() {
            if object.contains_key(segment) {
                self.error(
                    format!(
                        "duplicate labeled block `{block_type}` with label path `{full_label_path}` encountered during evaluation"
                    ),
                    span,
                );
                return;
            }

            object.insert(segment.clone(), value);
            return;
        }

        let entry = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(BTreeMap::new()));
        let Value::Object(next) = entry else {
            self.error(
                format!(
                    "cannot add labeled block `{block_type}` because label path segment `{segment}` collides with existing non-object value"
                ),
                span,
            );
            return;
        };

        self.insert_label_path_value(block_type, full_label_path, remainder, span, value, next);
    }

    pub(crate) fn evaluate_expression(&mut self, expression: &Expression) -> Value {
        match expression {
            Expression::Literal(literal) => match &literal.value {
                LiteralValue::Number(number) => match number.parse::<f64>() {
                    Ok(value) => Value::Number(value),
                    Err(_) => {
                        self.error(
                            format!("invalid numeric literal `{number}` during evaluation"),
                            literal.span,
                        );
                        Value::Null
                    }
                },
                LiteralValue::Bool(value) => Value::Bool(*value),
                LiteralValue::Null => Value::Null,
            },
            Expression::Template(template) => self.evaluate_template(template),
            Expression::For(for_expression) => self.evaluate_for_expression(for_expression),
            Expression::Variable(variable) => match self.lookup_variable(&variable.name) {
                Some(value) => value,
                None => {
                    self.error(
                        format!("unknown variable `{}` in evaluation", variable.name),
                        variable.span,
                    );
                    Value::Null
                }
            },
            Expression::Tuple(tuple) => {
                let values = tuple
                    .elements
                    .iter()
                    .map(|element| self.evaluate_expression(element))
                    .collect();
                Value::Tuple(values)
            }
            Expression::Object(object) => {
                let mut values = BTreeMap::new();
                for item in &object.items {
                    let key = self.evaluate_object_key(&item.key);
                    let value = self.evaluate_expression(&item.value);
                    if let Some(key) = key {
                        values.insert(key, value);
                    }
                }
                Value::Object(values)
            }
            Expression::FunctionCall(function_call) => self.evaluate_function_call(function_call),
            Expression::Traversal(traversal) => self.evaluate_traversal(traversal),
            Expression::Unary(unary) => {
                let value = self.evaluate_expression(&unary.expression);
                match unary.operator {
                    UnaryOperator::Negate => match value {
                        Value::Number(number) => Value::Number(-number),
                        other => {
                            self.error(
                                format!(
                                    "operator `-` requires a number operand, got {}",
                                    describe_type(&other)
                                ),
                                unary.span,
                            );
                            Value::Null
                        }
                    },
                    UnaryOperator::Not => match value {
                        Value::Bool(boolean) => Value::Bool(!boolean),
                        other => {
                            self.error(
                                format!(
                                    "operator `!` requires a bool operand, got {}",
                                    describe_type(&other)
                                ),
                                unary.span,
                            );
                            Value::Null
                        }
                    },
                }
            }
            Expression::Binary(binary) => self.evaluate_binary(
                binary.left.as_ref(),
                &binary.operator,
                &binary.right,
                binary,
            ),
            Expression::Conditional(ConditionalExpr {
                predicate,
                if_true,
                if_false,
                span,
            }) => {
                let predicate_value = self.evaluate_expression(predicate);
                match predicate_value {
                    Value::Bool(true) => self.evaluate_expression(if_true),
                    Value::Bool(false) => self.evaluate_expression(if_false),
                    other => {
                        self.error(
                            format!(
                                "conditional predicate must be bool, got {}",
                                describe_type(&other)
                            ),
                            *span,
                        );
                        Value::Null
                    }
                }
            }
            Expression::Invalid(span) => {
                self.error("cannot evaluate invalid expression", *span);
                Value::Null
            }
        }
    }

    fn evaluate_binary(
        &mut self,
        left_expression: &Expression,
        operator: &BinaryOperator,
        right_expression: &Expression,
        binary: &BinaryExpr,
    ) -> Value {
        if matches!(operator, BinaryOperator::And) {
            let left = self.evaluate_expression(left_expression);
            return match left {
                Value::Bool(false) => Value::Bool(false),
                Value::Bool(true) => {
                    let right = self.evaluate_expression(right_expression);
                    match right {
                        Value::Bool(value) => Value::Bool(value),
                        other => {
                            self.error(
                                format!(
                                    "operator `&&` requires bool operands, got bool and {}",
                                    describe_type(&other)
                                ),
                                binary.span,
                            );
                            Value::Null
                        }
                    }
                }
                other => {
                    self.error(
                        format!(
                            "operator `&&` requires bool operands, got {} and <unevaluated>",
                            describe_type(&other)
                        ),
                        binary.span,
                    );
                    Value::Null
                }
            };
        }

        if matches!(operator, BinaryOperator::Or) {
            let left = self.evaluate_expression(left_expression);
            return match left {
                Value::Bool(true) => Value::Bool(true),
                Value::Bool(false) => {
                    let right = self.evaluate_expression(right_expression);
                    match right {
                        Value::Bool(value) => Value::Bool(value),
                        other => {
                            self.error(
                                format!(
                                    "operator `||` requires bool operands, got bool and {}",
                                    describe_type(&other)
                                ),
                                binary.span,
                            );
                            Value::Null
                        }
                    }
                }
                other => {
                    self.error(
                        format!(
                            "operator `||` requires bool operands, got {} and <unevaluated>",
                            describe_type(&other)
                        ),
                        binary.span,
                    );
                    Value::Null
                }
            };
        }

        let left = self.evaluate_expression(left_expression);
        let right = self.evaluate_expression(right_expression);

        match operator {
            BinaryOperator::Add => {
                self.numeric_binary(left, right, binary.span, "+", |l, r| Value::Number(l + r))
            }
            BinaryOperator::Subtract => {
                self.numeric_binary(left, right, binary.span, "-", |l, r| Value::Number(l - r))
            }
            BinaryOperator::Multiply => {
                self.numeric_binary(left, right, binary.span, "*", |l, r| Value::Number(l * r))
            }
            BinaryOperator::Divide => {
                let (left, right) = match self.numeric_operands(left, right, binary.span, "/") {
                    Some(values) => values,
                    None => return Value::Null,
                };
                if is_effective_zero(right) {
                    self.error("division by zero in evaluation", binary.span);
                    Value::Null
                } else {
                    Value::Number(left / right)
                }
            }
            BinaryOperator::Modulo => {
                let (left, right) = match self.numeric_operands(left, right, binary.span, "%") {
                    Some(values) => values,
                    None => return Value::Null,
                };
                if is_effective_zero(right) {
                    self.error("modulo by zero in evaluation", binary.span);
                    Value::Null
                } else {
                    Value::Number(left % right)
                }
            }
            BinaryOperator::Less => {
                self.numeric_compare(left, right, binary.span, "<", |l, r| l < r)
            }
            BinaryOperator::LessEqual => {
                self.numeric_compare(left, right, binary.span, "<=", |l, r| l <= r)
            }
            BinaryOperator::Greater => {
                self.numeric_compare(left, right, binary.span, ">", |l, r| l > r)
            }
            BinaryOperator::GreaterEqual => {
                self.numeric_compare(left, right, binary.span, ">=", |l, r| l >= r)
            }
            BinaryOperator::Equal => Value::Bool(values_equal(&left, &right)),
            BinaryOperator::NotEqual => Value::Bool(!values_equal(&left, &right)),
            BinaryOperator::And | BinaryOperator::Or => unreachable!(),
        }
    }

    fn lookup_variable(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Some(value.clone());
            }
        }

        self.context.variables.get(name).cloned()
    }

    fn evaluate_for_expression(&mut self, expression: &ForExpr) -> Value {
        let collection = self.evaluate_expression(&expression.collection);
        let Some(iter_items) = self.evaluate_for_collection(
            collection,
            expression.collection.span(),
            "`for` expression",
        ) else {
            return Value::Null;
        };

        match &expression.kind {
            ForExprKind::Tuple { value } => {
                let mut values = Vec::new();
                for (iter_key, iter_value) in iter_items {
                    let bindings = self.iteration_bindings(
                        expression.key_var.as_deref(),
                        &expression.value_var,
                        iter_key,
                        iter_value,
                    );

                    if let Some(condition) = &expression.condition {
                        let predicate = self.with_scope(bindings.clone(), |evaluator| {
                            evaluator.evaluate_expression(condition)
                        });
                        let Some(should_include) = self.expect_bool(
                            predicate,
                            condition.span(),
                            "`for` expression `if` filter",
                        ) else {
                            return Value::Null;
                        };
                        if !should_include {
                            continue;
                        }
                    }

                    let value =
                        self.with_scope(bindings, |evaluator| evaluator.evaluate_expression(value));
                    values.push(value);
                }
                Value::Tuple(values)
            }
            ForExprKind::Object { key, value, group } => {
                let mut values = BTreeMap::new();
                for (iter_key, iter_value) in iter_items {
                    let bindings = self.iteration_bindings(
                        expression.key_var.as_deref(),
                        &expression.value_var,
                        iter_key,
                        iter_value,
                    );

                    if let Some(condition) = &expression.condition {
                        let predicate = self.with_scope(bindings.clone(), |evaluator| {
                            evaluator.evaluate_expression(condition)
                        });
                        let Some(should_include) = self.expect_bool(
                            predicate,
                            condition.span(),
                            "`for` expression `if` filter",
                        ) else {
                            return Value::Null;
                        };
                        if !should_include {
                            continue;
                        }
                    }

                    let (key_value, value_value) = self.with_scope(bindings, |evaluator| {
                        (
                            evaluator.evaluate_expression(key),
                            evaluator.evaluate_expression(value),
                        )
                    });
                    let Some(key_value) = self.value_to_object_key(
                        key_value,
                        key.span(),
                        "object `for` key expression must evaluate to string-like value",
                    ) else {
                        return Value::Null;
                    };

                    if *group {
                        let entry = values
                            .entry(key_value)
                            .or_insert_with(|| Value::Tuple(Vec::new()));
                        if let Value::Tuple(grouped_values) = entry {
                            grouped_values.push(value_value);
                        }
                        continue;
                    }

                    if values.insert(key_value.clone(), value_value).is_some() {
                        self.error(
                            format!(
                                "object `for` expression produced duplicate key `{key_value}` without grouping"
                            ),
                            key.span(),
                        );
                        return Value::Null;
                    }
                }

                Value::Object(values)
            }
        }
    }

    fn evaluate_function_call(&mut self, function_call: &FunctionCallExpr) -> Value {
        match function_call.name.as_str() {
            "length" => {
                let Some(mut arguments) =
                    self.evaluate_function_arguments(function_call, "length", 1)
                else {
                    return Value::Null;
                };
                let argument = arguments.remove(0);
                match argument {
                    Value::String(value) => Value::Number(value.chars().count() as f64),
                    Value::Tuple(values) => Value::Number(values.len() as f64),
                    Value::Object(values) => Value::Number(values.len() as f64),
                    other => {
                        self.error(
                            format!(
                                "function `length` expects string, tuple, or object argument, got {}",
                                describe_type(&other)
                            ),
                            function_call.span,
                        );
                        Value::Null
                    }
                }
            }
            "keys" => {
                let Some(mut arguments) =
                    self.evaluate_function_arguments(function_call, "keys", 1)
                else {
                    return Value::Null;
                };
                let argument = arguments.remove(0);
                match argument {
                    Value::Object(values) => Value::Tuple(
                        values
                            .keys()
                            .map(|key| Value::String(key.clone()))
                            .collect::<Vec<_>>(),
                    ),
                    other => {
                        self.error(
                            format!(
                                "function `keys` expects object argument, got {}",
                                describe_type(&other)
                            ),
                            function_call.span,
                        );
                        Value::Null
                    }
                }
            }
            "values" => {
                let Some(mut arguments) =
                    self.evaluate_function_arguments(function_call, "values", 1)
                else {
                    return Value::Null;
                };
                let argument = arguments.remove(0);
                match argument {
                    Value::Object(values) => {
                        Value::Tuple(values.values().cloned().collect::<Vec<_>>())
                    }
                    other => {
                        self.error(
                            format!(
                                "function `values` expects object argument, got {}",
                                describe_type(&other)
                            ),
                            function_call.span,
                        );
                        Value::Null
                    }
                }
            }
            _ => {
                self.error(
                    format!("unknown function `{}` in evaluation", function_call.name),
                    function_call.span,
                );
                Value::Null
            }
        }
    }

    fn evaluate_function_arguments(
        &mut self,
        function_call: &FunctionCallExpr,
        function_name: &str,
        expected_arity: usize,
    ) -> Option<Vec<Value>> {
        if !function_call.expand_final {
            if function_call.arguments.len() != expected_arity {
                self.error(
                    format!(
                        "function `{function_name}` expects exactly {expected_arity} argument, got {}",
                        function_call.arguments.len()
                    ),
                    function_call.span,
                );
                return None;
            }

            return Some(
                function_call
                    .arguments
                    .iter()
                    .map(|argument| self.evaluate_expression(argument))
                    .collect::<Vec<_>>(),
            );
        }

        let Some((final_argument, positional_arguments)) = function_call.arguments.split_last()
        else {
            self.error(
                format!(
                    "function `{function_name}` expects exactly {expected_arity} argument, got 0"
                ),
                function_call.span,
            );
            return None;
        };

        let mut arguments = positional_arguments
            .iter()
            .map(|argument| self.evaluate_expression(argument))
            .collect::<Vec<_>>();
        let final_value = self.evaluate_expression(final_argument);
        let Value::Tuple(expanded) = final_value else {
            self.error(
                format!(
                    "function argument expansion with `...` requires tuple final argument, got {}",
                    describe_type(&final_value)
                ),
                final_argument.span(),
            );
            return None;
        };
        arguments.extend(expanded);

        if arguments.len() != expected_arity {
            self.error(
                format!(
                    "function `{function_name}` expects exactly {expected_arity} argument, got {}",
                    arguments.len()
                ),
                function_call.span,
            );
            return None;
        }

        Some(arguments)
    }

    fn evaluate_traversal(&mut self, traversal: &TraversalExpr) -> Value {
        let value = self.evaluate_expression(&traversal.target);
        self.apply_traversal_operations(value, &traversal.operations)
            .unwrap_or(Value::Null)
    }

    fn apply_traversal_operations(
        &mut self,
        target: Value,
        operations: &[TraversalOperation],
    ) -> Option<Value> {
        let Some((operation, remaining)) = operations.split_first() else {
            return Some(target);
        };

        match operation {
            TraversalOperation::AttrSplat { span } => {
                self.apply_attr_splat_operation(target, remaining, *span)
            }
            TraversalOperation::FullSplat { .. } => {
                self.apply_full_splat_operation(target, remaining)
            }
            _ => {
                let next = self.apply_traversal_operation(target, operation)?;
                self.apply_traversal_operations(next, remaining)
            }
        }
    }

    fn apply_attr_splat_operation(
        &mut self,
        target: Value,
        remaining: &[TraversalOperation],
        span: Span,
    ) -> Option<Value> {
        let attr_len = remaining
            .iter()
            .take_while(|operation| matches!(operation, TraversalOperation::GetAttr(_)))
            .count();
        let (attr_operations, trailing_operations) = remaining.split_at(attr_len);
        let iter_values = self.attr_splat_values(target, span)?;
        let mut values = Vec::with_capacity(iter_values.len());

        for iter_value in iter_values {
            let value = self.apply_direct_traversal_operations(iter_value, attr_operations)?;
            values.push(value);
        }

        self.apply_traversal_operations(Value::Tuple(values), trailing_operations)
    }

    fn apply_full_splat_operation(
        &mut self,
        target: Value,
        remaining: &[TraversalOperation],
    ) -> Option<Value> {
        let iter_values = self.full_splat_values(target);
        let values = self.collect_splat_values(iter_values, remaining)?;

        Some(Value::Tuple(values))
    }

    fn apply_direct_traversal_operations(
        &mut self,
        mut target: Value,
        operations: &[TraversalOperation],
    ) -> Option<Value> {
        for operation in operations {
            target = self.apply_traversal_operation(target, operation)?;
        }

        Some(target)
    }

    fn collect_splat_values(
        &mut self,
        iter_values: Vec<Value>,
        remaining: &[TraversalOperation],
    ) -> Option<Vec<Value>> {
        let mut values = Vec::with_capacity(iter_values.len());

        for iter_value in iter_values {
            // Keep splat chains deterministic by terminating at the first failing branch.
            let value = self.apply_traversal_operations(iter_value, remaining)?;
            values.push(value);
        }

        Some(values)
    }

    fn attr_splat_values(&mut self, target: Value, span: Span) -> Option<Vec<Value>> {
        match target {
            Value::Tuple(values) => Some(values),
            Value::Object(values) => Some(values.into_values().collect::<Vec<_>>()),
            other => {
                self.error(
                    format!(
                        "splat traversal requires tuple or object target, got {}",
                        describe_type(&other)
                    ),
                    span,
                );
                None
            }
        }
    }

    fn full_splat_values(&self, target: Value) -> Vec<Value> {
        match target {
            Value::Tuple(values) => values,
            Value::Object(values) => values.into_values().collect::<Vec<_>>(),
            Value::Null => Vec::new(),
            value @ (Value::Bool(_) | Value::Number(_) | Value::String(_)) => vec![value],
        }
    }

    fn apply_traversal_operation(
        &mut self,
        target: Value,
        operation: &TraversalOperation,
    ) -> Option<Value> {
        match operation {
            TraversalOperation::GetAttr(operation) => match target {
                Value::Object(values) => match values.get(&operation.name) {
                    Some(value) => Some(value.clone()),
                    None => {
                        self.error(
                            format!("object does not contain key `{}`", operation.name),
                            operation.span,
                        );
                        None
                    }
                },
                other => {
                    self.error(
                        format!(
                            "attribute traversal requires object target, got {}",
                            describe_type(&other)
                        ),
                        operation.span,
                    );
                    None
                }
            },
            TraversalOperation::Index(operation) => {
                let key_value = self.evaluate_expression(&operation.key);
                self.index_value(target, key_value, operation.span)
            }
            TraversalOperation::LegacyIndex(operation) => match target {
                Value::Tuple(values) => {
                    let index = operation.index.parse::<usize>().unwrap_or(usize::MAX);
                    if index < values.len() {
                        Some(values[index].clone())
                    } else {
                        self.error(
                            format!(
                                "tuple index {index} is out of range for length {}",
                                values.len()
                            ),
                            operation.span,
                        );
                        None
                    }
                }
                Value::Object(values) => match values.get(&operation.index) {
                    Some(value) => Some(value.clone()),
                    None => {
                        self.error(
                            format!("object does not contain key `{}`", operation.index),
                            operation.span,
                        );
                        None
                    }
                },
                other => {
                    self.error(
                        format!(
                            "legacy index traversal requires tuple or object target, got {}",
                            describe_type(&other)
                        ),
                        operation.span,
                    );
                    None
                }
            },
            TraversalOperation::AttrSplat { .. } | TraversalOperation::FullSplat { .. } => {
                unreachable!("splat operations are handled in apply_traversal_operations")
            }
        }
    }

    fn index_value(&mut self, target: Value, key: Value, span: Span) -> Option<Value> {
        match target {
            Value::Tuple(values) => {
                let Value::Number(index_number) = key else {
                    self.error(
                        format!("tuple index must be number, got {}", describe_type(&key)),
                        span,
                    );
                    return None;
                };

                let index = self.number_to_index(index_number, span)?;
                if index < values.len() {
                    Some(values[index].clone())
                } else {
                    self.error(
                        format!(
                            "tuple index {index} is out of range for length {}",
                            values.len()
                        ),
                        span,
                    );
                    None
                }
            }
            Value::Object(values) => {
                let key_string = self.value_to_object_key(
                    key,
                    span,
                    "object index must evaluate to string-like value",
                )?;
                match values.get(&key_string) {
                    Some(value) => Some(value.clone()),
                    None => {
                        self.error(format!("object does not contain key `{key_string}`"), span);
                        None
                    }
                }
            }
            other => {
                self.error(
                    format!(
                        "index traversal requires tuple or object target, got {}",
                        describe_type(&other)
                    ),
                    span,
                );
                None
            }
        }
    }

    fn number_to_index(&mut self, value: f64, span: Span) -> Option<usize> {
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
            self.error(
                format!(
                    "tuple index must be a non-negative whole number, got {}",
                    format_number(value)
                ),
                span,
            );
            return None;
        }

        if value > usize::MAX as f64 {
            self.error(
                format!(
                    "tuple index must be a non-negative whole number, got {}",
                    format_number(value)
                ),
                span,
            );
            return None;
        }

        Some(value as usize)
    }

    fn numeric_binary(
        &mut self,
        left: Value,
        right: Value,
        span: Span,
        operator: &str,
        apply: impl FnOnce(f64, f64) -> Value,
    ) -> Value {
        let (left, right) = match self.numeric_operands(left, right, span, operator) {
            Some(values) => values,
            None => return Value::Null,
        };
        apply(left, right)
    }

    fn numeric_compare(
        &mut self,
        left: Value,
        right: Value,
        span: Span,
        operator: &str,
        compare: impl FnOnce(f64, f64) -> bool,
    ) -> Value {
        let (left, right) = match self.numeric_operands(left, right, span, operator) {
            Some(values) => values,
            None => return Value::Null,
        };
        Value::Bool(compare(left, right))
    }

    fn numeric_operands(
        &mut self,
        left: Value,
        right: Value,
        span: Span,
        operator: &str,
    ) -> Option<(f64, f64)> {
        match (left, right) {
            (Value::Number(left), Value::Number(right)) => Some((left, right)),
            (left, right) => {
                self.error(
                    format!(
                        "operator `{operator}` requires numeric operands, got {} and {}",
                        describe_type(&left),
                        describe_type(&right)
                    ),
                    span,
                );
                None
            }
        }
    }

    fn evaluate_object_key(&mut self, key: &ObjectKey) -> Option<String> {
        match key {
            ObjectKey::Identifier { name, .. } => Some(name.clone()),
            ObjectKey::Expression { expression } => {
                let value = self.evaluate_expression(expression);
                self.value_to_object_key(
                    value,
                    key.span(),
                    "object key expression must evaluate to string-like value",
                )
            }
        }
    }

    fn evaluate_template(&mut self, template: &TemplateExpr) -> Value {
        if let Some(expression) = template::unwrap_candidate_expression(template) {
            return self.evaluate_expression(expression);
        }

        Value::String(template::render_template(self, template))
    }

    fn iteration_bindings(
        &self,
        key_var: Option<&str>,
        value_var: &str,
        key_value: Value,
        value_value: Value,
    ) -> BTreeMap<String, Value> {
        let mut bindings = BTreeMap::new();
        if let Some(key_var) = key_var {
            bindings.insert(key_var.to_owned(), key_value);
        }
        bindings.insert(value_var.to_owned(), value_value);
        bindings
    }

    fn value_to_object_key(&mut self, value: Value, span: Span, context: &str) -> Option<String> {
        match value {
            Value::String(value) => Some(value),
            Value::Number(value) => Some(format_number(value)),
            Value::Bool(true) => Some("true".to_owned()),
            Value::Bool(false) => Some("false".to_owned()),
            Value::Null => Some("null".to_owned()),
            other => {
                self.error(format!("{context}, got {}", describe_type(&other)), span);
                None
            }
        }
    }

    pub(crate) fn evaluate_for_collection(
        &mut self,
        collection: Value,
        span: Span,
        context: &str,
    ) -> Option<Vec<(Value, Value)>> {
        match collection {
            Value::Tuple(values) => Some(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| (Value::Number(index as f64), value))
                    .collect::<Vec<_>>(),
            ),
            Value::Object(values) => Some(
                values
                    .into_iter()
                    .map(|(key, value)| (Value::String(key), value))
                    .collect::<Vec<_>>(),
            ),
            other => {
                self.error(
                    format!(
                        "{context} collection must evaluate to tuple or object, got {}",
                        describe_type(&other)
                    ),
                    span,
                );
                None
            }
        }
    }

    pub(crate) fn expect_bool(&mut self, value: Value, span: Span, context: &str) -> Option<bool> {
        match value {
            Value::Bool(value) => Some(value),
            other => {
                self.error(
                    format!(
                        "{context} must evaluate to bool, got {}",
                        describe_type(&other)
                    ),
                    span,
                );
                None
            }
        }
    }

    pub(crate) fn with_scope<R>(
        &mut self,
        bindings: BTreeMap<String, Value>,
        run: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.scopes.push(bindings);
        let result = run(self);
        self.scopes.pop();
        result
    }

    pub(crate) fn interpolation_to_string(&mut self, value: Value, span: Span) -> String {
        match value {
            Value::Null => "null".to_owned(),
            Value::Bool(true) => "true".to_owned(),
            Value::Bool(false) => "false".to_owned(),
            Value::Number(number) => format_number(number),
            Value::String(value) => value,
            Value::Tuple(_) | Value::Object(_) => {
                self.error(
                    "template interpolation currently supports only primitive values".to_owned(),
                    span,
                );
                String::new()
            }
        }
    }

    pub(crate) fn error(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics.push(Diagnostic::error(message, span));
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => numbers_close(*left, *right),
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Tuple(left), Value::Tuple(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| values_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left_value)| {
                    right
                        .get(key)
                        .is_some_and(|right_value| values_equal(left_value, right_value))
                })
        }
        _ => false,
    }
}

fn numbers_close(left: f64, right: f64) -> bool {
    let delta = (left - right).abs();
    delta <= 1e-12_f64 * left.abs().max(right.abs()).max(1.0)
}

fn is_effective_zero(number: f64) -> bool {
    number.abs() <= f64::EPSILON
}

fn describe_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Tuple(_) => "tuple",
        Value::Object(_) => "object",
    }
}

fn block_label_to_string(label: &BlockLabel) -> String {
    match label {
        BlockLabel::Identifier(value) => value.clone(),
        BlockLabel::StringLiteral(value) => value.clone(),
    }
}

fn format_number(number: f64) -> String {
    let rendered = number.to_string();
    if rendered == "-0" {
        "0".to_owned()
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use crate::ast::{BodyItem, Expression, LiteralValue, ObjectKey, TemplateSegment};
    use crate::lexer::lex_str;
    use crate::parser::parse;
    use crate::test_fixtures::{
        discover_expression_fixtures, load_fixture_contract, message_snippet_matches,
    };

    use super::{EvalContext, Value, evaluate_config, evaluate_expression};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TypeContract {
        Any,
        Bool,
        Number,
        String,
        Null,
        Tuple(Vec<TypeContract>),
        Object(BTreeMap<String, TypeContract>),
        Map(Box<TypeContract>),
    }

    fn parse_source(source: &str) -> crate::parser::ParseResult {
        let lexed = lex_str(source);
        assert!(
            lexed.diagnostics.is_empty(),
            "lexer diagnostics: {:#?}",
            lexed.diagnostics
        );

        let parsed = parse(&lexed.tokens);
        assert!(
            parsed.diagnostics.is_empty(),
            "parser diagnostics: {:#?}",
            parsed.diagnostics
        );
        parsed
    }

    fn parse_fixture(path: &Path) -> crate::parser::ParseResult {
        let source = fs::read_to_string(path).expect("fixture source should exist");
        parse_source(&source)
    }

    fn load_fixture_variables(path: &Path) -> EvalContext {
        let schema_path = path.with_extension("hcldec");
        if !schema_path.exists() {
            return EvalContext::default();
        }

        let parsed = parse_fixture(&schema_path);
        let mut variables = BTreeMap::new();

        for item in parsed.config.body.items {
            let BodyItem::Block(block) = item else {
                continue;
            };
            if block.block_type != "variables" {
                continue;
            }

            for variable_item in block.body.items {
                let BodyItem::Attribute(attribute) = variable_item else {
                    continue;
                };
                let context = EvalContext {
                    variables: variables.clone(),
                };
                let result = evaluate_expression(&attribute.expression, &context);
                assert!(
                    result.diagnostics.is_empty(),
                    "unexpected diagnostics while loading fixture variables from {}: {:#?}",
                    schema_path.display(),
                    result.diagnostics
                );
                variables.insert(attribute.name, result.value);
            }
        }

        EvalContext { variables }
    }

    fn eval_fixture(fixture_path: &Path) {
        let parsed = parse_fixture(fixture_path);
        let context = load_fixture_variables(fixture_path);
        let evaluation = evaluate_config(&parsed.config, &context);

        let contract_path = fixture_path.with_extension("t");
        let contract =
            load_fixture_contract(&contract_path).unwrap_or_else(|error| panic!("{error}"));
        assert_fixture_diagnostics(
            &fixture_path.display().to_string(),
            &evaluation.diagnostics,
            contract.diagnostics,
        );

        if let Some(expected_result_expression) = contract.result {
            let expected_result =
                evaluate_expression(&expected_result_expression, &EvalContext::default());
            assert!(
                expected_result.diagnostics.is_empty(),
                "unexpected diagnostics while evaluating expected result contract {}: {:#?}",
                contract_path.display(),
                expected_result.diagnostics
            );

            assert_value_matches(&evaluation.value, &expected_result.value, "$result");
        }

        if let Some(expected_type_expression) = contract.result_type {
            let expected_type = parse_type_contract(&expected_type_expression);
            assert!(
                type_contract_matches(&expected_type, &evaluation.value),
                "result_type mismatch for {}\nactual: {:#?}\nexpected type: {expected_type:#?}",
                fixture_path.display(),
                evaluation.value
            );
        }
    }

    fn assert_fixture_diagnostics(
        fixture_path: &str,
        actual: &[crate::diagnostics::Diagnostic],
        expected: Option<Vec<crate::test_fixtures::ExpectedDiagnostic>>,
    ) {
        match expected {
            Some(expected_diagnostics) => {
                assert_eq!(
                    actual.len(),
                    expected_diagnostics.len(),
                    "diagnostic count mismatch for fixture {fixture_path}\nactual: {actual:#?}\nexpected: {expected_diagnostics:#?}",
                );

                for (index, (actual_diagnostic, expected_diagnostic)) in
                    actual.iter().zip(expected_diagnostics.iter()).enumerate()
                {
                    assert_eq!(
                        actual_diagnostic.severity, expected_diagnostic.severity,
                        "severity mismatch for fixture {fixture_path} diagnostic #{index}",
                    );
                    assert_eq!(
                        (actual_diagnostic.span.start, actual_diagnostic.span.end),
                        (expected_diagnostic.start, expected_diagnostic.end),
                        "span mismatch for fixture {fixture_path} diagnostic #{index}",
                    );

                    if !expected_diagnostic.message_like.is_empty() {
                        let actual_message = actual_diagnostic.message.to_ascii_lowercase();
                        let matches_expected = expected_diagnostic
                            .message_like
                            .iter()
                            .any(|snippet| message_snippet_matches(&actual_message, snippet));
                        assert!(
                            matches_expected,
                            "message mismatch for fixture {fixture_path} diagnostic #{index}\nactual: {}\nexpected snippets: {:?}",
                            actual_diagnostic.message, expected_diagnostic.message_like,
                        );
                    }
                }
            }
            None => {
                assert!(
                    actual.is_empty(),
                    "unexpected diagnostics for fixture {fixture_path}: {actual:#?}",
                );
            }
        }
    }

    fn assert_value_matches(actual: &Value, expected: &Value, path: &str) {
        match (actual, expected) {
            (Value::Null, Value::Null) => {}
            (Value::Bool(actual), Value::Bool(expected)) => {
                assert_eq!(actual, expected, "bool mismatch at {path}")
            }
            (Value::Number(actual), Value::Number(expected)) => {
                assert!(
                    numbers_close(*actual, *expected),
                    "number mismatch at {path}: actual={actual} expected={expected}"
                );
            }
            (Value::String(actual), Value::String(expected)) => {
                assert_eq!(actual, expected, "string mismatch at {path}")
            }
            (Value::Tuple(actual), Value::Tuple(expected)) => {
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "tuple length mismatch at {path}"
                );
                for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
                    assert_value_matches(actual, expected, &format!("{path}[{index}]"));
                }
            }
            (Value::Object(actual), Value::Object(expected)) => {
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "object key count mismatch at {path}\nactual keys: {:?}\nexpected keys: {:?}",
                    actual.keys().collect::<Vec<_>>(),
                    expected.keys().collect::<Vec<_>>()
                );
                for (key, expected_value) in expected {
                    let actual_value = actual
                        .get(key)
                        .unwrap_or_else(|| panic!("missing key `{key}` at {path}"));
                    assert_value_matches(actual_value, expected_value, &format!("{path}.{key}"));
                }
            }
            _ => panic!("value kind mismatch at {path}: actual={actual:#?} expected={expected:#?}"),
        }
    }

    fn parse_type_contract(expression: &Expression) -> TypeContract {
        match expression {
            Expression::Variable(variable) => match variable.name.as_str() {
                "any" => TypeContract::Any,
                "bool" => TypeContract::Bool,
                "number" => TypeContract::Number,
                "string" => TypeContract::String,
                "null" => TypeContract::Null,
                other => panic!("unsupported result_type symbol `{other}`"),
            },
            Expression::FunctionCall(function) => match function.name.as_str() {
                "map" => {
                    assert_eq!(
                        function.arguments.len(),
                        1,
                        "map() in result_type must take one argument"
                    );
                    TypeContract::Map(Box::new(parse_type_contract(&function.arguments[0])))
                }
                "object" => {
                    assert_eq!(
                        function.arguments.len(),
                        1,
                        "object() in result_type must take one argument"
                    );
                    let Expression::Object(object) = &function.arguments[0] else {
                        panic!("object() in result_type must receive an object literal")
                    };

                    let mut fields = BTreeMap::new();
                    for item in &object.items {
                        let key = object_key_to_string(&item.key).unwrap_or_else(|| {
                            panic!("unsupported object() key expression in result_type")
                        });
                        fields.insert(key, parse_type_contract(&item.value));
                    }
                    TypeContract::Object(fields)
                }
                other => panic!("unsupported result_type function `{other}`"),
            },
            Expression::Object(object) => {
                let mut fields = BTreeMap::new();
                for item in &object.items {
                    let key = object_key_to_string(&item.key)
                        .expect("unsupported object key expression in result_type literal");
                    fields.insert(key, parse_type_contract(&item.value));
                }
                TypeContract::Object(fields)
            }
            Expression::Tuple(tuple) => TypeContract::Tuple(
                tuple
                    .elements
                    .iter()
                    .map(parse_type_contract)
                    .collect::<Vec<_>>(),
            ),
            other => panic!("unsupported result_type expression: {other:?}"),
        }
    }

    fn object_key_to_string(key: &ObjectKey) -> Option<String> {
        match key {
            ObjectKey::Identifier { name, .. } => Some(name.clone()),
            ObjectKey::Expression { expression } => match expression.as_ref() {
                Expression::Template(template) => {
                    if template.segments.len() != 1 {
                        return None;
                    }
                    match &template.segments[0] {
                        TemplateSegment::Literal(segment) => Some(segment.value.clone()),
                        _ => None,
                    }
                }
                Expression::Literal(literal) => match &literal.value {
                    LiteralValue::Bool(true) => Some("true".to_owned()),
                    LiteralValue::Bool(false) => Some("false".to_owned()),
                    LiteralValue::Null => Some("null".to_owned()),
                    LiteralValue::Number(number) => Some(number.clone()),
                },
                Expression::Variable(variable) => Some(variable.name.clone()),
                _ => None,
            },
        }
    }

    fn type_contract_matches(expected: &TypeContract, value: &Value) -> bool {
        match expected {
            TypeContract::Any => true,
            TypeContract::Bool => matches!(value, Value::Bool(_)),
            TypeContract::Number => matches!(value, Value::Number(_)),
            TypeContract::String => matches!(value, Value::String(_)),
            TypeContract::Null => matches!(value, Value::Null),
            TypeContract::Tuple(expected_items) => match value {
                Value::Tuple(actual_items) => {
                    expected_items.len() == actual_items.len()
                        && expected_items
                            .iter()
                            .zip(actual_items.iter())
                            .all(|(expected, actual)| type_contract_matches(expected, actual))
                }
                _ => false,
            },
            TypeContract::Object(expected_fields) => match value {
                Value::Object(actual_fields) => {
                    expected_fields.len() == actual_fields.len()
                        && expected_fields.iter().all(|(key, expected_type)| {
                            actual_fields
                                .get(key)
                                .is_some_and(|actual| type_contract_matches(expected_type, actual))
                        })
                }
                _ => false,
            },
            TypeContract::Map(expected_value_type) => match value {
                Value::Object(actual_fields) => actual_fields
                    .values()
                    .all(|actual| type_contract_matches(expected_value_type, actual)),
                _ => false,
            },
        }
    }

    fn numbers_close(left: f64, right: f64) -> bool {
        let delta = (left - right).abs();
        delta <= 1e-12_f64 * left.abs().max(right.abs()).max(1.0)
    }

    #[test]
    fn primitive_literals_fixture_keeps_decomposed_n_tilde_bytes() {
        let source = fs::read("specsuite/tests/expressions/primitive_literals.hcl")
            .expect("primitive_literals fixture should be readable");
        let needle = b"an\xcc\x83os";

        assert!(
            source.windows(needle.len()).any(|window| window == needle),
            "expected decomposed `an\\u0303os` byte sequence in primitive_literals fixture source",
        );
    }

    #[test]
    fn specsuite_expression_fixtures_match_eval_contracts() {
        let fixtures = discover_expression_fixtures().unwrap_or_else(|error| panic!("{error}"));
        for fixture in fixtures {
            eval_fixture(&fixture);
        }
    }
}
