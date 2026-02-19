use crate::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigFile {
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Body {
    pub items: Vec<BodyItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyItem {
    Attribute(Attribute),
    Block(Block),
    OneLineBlock(OneLineBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub expression: Expression,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub block_type: String,
    pub labels: Vec<BlockLabel>,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneLineBlock {
    pub block_type: String,
    pub labels: Vec<BlockLabel>,
    pub attribute: Option<Attribute>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockLabel {
    Identifier(String),
    StringLiteral(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expression {
    Literal(LiteralExpr),
    Template(TemplateExpr),
    For(ForExpr),
    Variable(VariableExpr),
    Tuple(TupleExpr),
    Object(ObjectExpr),
    FunctionCall(FunctionCallExpr),
    Traversal(TraversalExpr),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    Conditional(ConditionalExpr),
    Invalid(Span),
}

impl Expression {
    pub fn span(&self) -> Span {
        match self {
            Expression::Literal(expression) => expression.span,
            Expression::Template(expression) => expression.span,
            Expression::For(expression) => expression.span,
            Expression::Variable(expression) => expression.span,
            Expression::Tuple(expression) => expression.span,
            Expression::Object(expression) => expression.span,
            Expression::FunctionCall(expression) => expression.span,
            Expression::Traversal(expression) => expression.span,
            Expression::Unary(expression) => expression.span,
            Expression::Binary(expression) => expression.span,
            Expression::Conditional(expression) => expression.span,
            Expression::Invalid(span) => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralExpr {
    pub value: LiteralValue,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiteralValue {
    Number(String),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateExpr {
    pub segments: Vec<TemplateSegment>,
    pub kind: TemplateKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateKind {
    Quoted,
    Heredoc { flush: bool, marker: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSegment {
    Literal(TemplateLiteralSegment),
    Interpolation(TemplateInterpolationSegment),
    Directive(TemplateDirectiveSegment),
}

impl TemplateSegment {
    pub fn span(&self) -> Span {
        match self {
            TemplateSegment::Literal(segment) => segment.span,
            TemplateSegment::Interpolation(segment) => segment.span,
            TemplateSegment::Directive(segment) => segment.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateLiteralSegment {
    pub value: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateInterpolationSegment {
    pub expression: Box<Expression>,
    pub strip_left: bool,
    pub strip_right: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateDirectiveSegment {
    pub directive: TemplateDirective,
    pub strip_left: bool,
    pub strip_right: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateDirective {
    If {
        condition: Box<Expression>,
    },
    Else,
    EndIf,
    For {
        key_var: Option<String>,
        value_var: String,
        collection: Box<Expression>,
    },
    EndFor,
    Unknown {
        keyword: String,
        expression: Option<Box<Expression>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForExpr {
    pub key_var: Option<String>,
    pub value_var: String,
    pub collection: Box<Expression>,
    pub kind: ForExprKind,
    pub condition: Option<Box<Expression>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForExprKind {
    Tuple {
        value: Box<Expression>,
    },
    Object {
        key: Box<Expression>,
        value: Box<Expression>,
        group: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableExpr {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleExpr {
    pub elements: Vec<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectExpr {
    pub items: Vec<ObjectItem>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectItem {
    pub key: ObjectKey,
    pub value: Expression,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectKey {
    Identifier { name: String, span: Span },
    Expression { expression: Box<Expression> },
}

impl ObjectKey {
    pub fn span(&self) -> Span {
        match self {
            ObjectKey::Identifier { span, .. } => *span,
            ObjectKey::Expression { expression } => expression.span(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCallExpr {
    pub name: String,
    pub arguments: Vec<Expression>,
    pub expand_final: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalExpr {
    pub target: Box<Expression>,
    pub operations: Vec<TraversalOperation>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraversalOperation {
    GetAttr(GetAttrOp),
    Index(IndexOp),
    LegacyIndex(LegacyIndexOp),
    AttrSplat { span: Span },
    FullSplat { span: Span },
}

impl TraversalOperation {
    pub fn span(&self) -> Span {
        match self {
            TraversalOperation::GetAttr(operation) => operation.span,
            TraversalOperation::Index(operation) => operation.span,
            TraversalOperation::LegacyIndex(operation) => operation.span,
            TraversalOperation::AttrSplat { span } => *span,
            TraversalOperation::FullSplat { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetAttrOp {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOp {
    pub key: Box<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyIndexOp {
    pub index: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnaryExpr {
    pub operator: UnaryOperator,
    pub expression: Box<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Negate,
    Not,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryExpr {
    pub left: Box<Expression>,
    pub operator: BinaryOperator,
    pub right: Box<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Multiply,
    Divide,
    Modulo,
    Add,
    Subtract,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

impl BinaryOperator {
    pub fn precedence(self) -> u8 {
        match self {
            BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Modulo => 6,
            BinaryOperator::Add | BinaryOperator::Subtract => 5,
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => 4,
            BinaryOperator::Equal | BinaryOperator::NotEqual => 3,
            BinaryOperator::And => 2,
            BinaryOperator::Or => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalExpr {
    pub predicate: Box<Expression>,
    pub if_true: Box<Expression>,
    pub if_false: Box<Expression>,
    pub span: Span,
}
