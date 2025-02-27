//! Nodes in the semantic graph.

// NOTE(affo): At this stage some nodes are a clone of the AST nodes with some type information added.
//  Nevertheless, new node types allow us to decouple this step of compilation from the parsing.
//  This is of paramount importance if we decide to add responsibilities to the semantic analysis and
//  change it independently from the parsing bits.
//  Uncommented node types are a direct port of the AST ones.
#![allow(clippy::match_single_binding)]

extern crate chrono;
extern crate derivative;

use crate::{
    ast,
    errors::Errors,
    semantic::{
        env::Environment,
        import::Importer,
        infer::{self, Constraint, Constraints},
        sub::{Substitutable, Substituter, Substitution},
        types::{
            self, Array, Dictionary, Function, Kind, MonoType, MonoTypeMap, PolyType, PolyTypeMap,
            Tvar, TvarKinds,
        },
    },
};

use std::{collections::HashMap, fmt::Debug, vec::Vec};

use anyhow::{anyhow, bail, Result as AnyhowResult};
use chrono::{prelude::DateTime, FixedOffset};
use derivative::Derivative;
use derive_more::Display;

/// Result returned from the various 'infer' methods defined in this
/// module. The result of inferring an expression or statement is a
/// set of type constraints to be solved.
pub type Result<T = Constraints> = std::result::Result<T, Error>;

/// Error returned from the various 'infer' methods defined in this
/// module.
pub type Error = Located<ErrorKind>;

/// An error with an attached location
#[derive(Debug, Display, PartialEq)]
#[display(fmt = "error {}: {}", location, error)]
pub struct Located<E> {
    /// The location where the error occured
    pub location: ast::SourceLocation,
    /// The error itself
    pub error: E,
}

fn located<E>(location: ast::SourceLocation, error: E) -> Located<E> {
    Located { location, error }
}

impl<E> Substitutable for Located<E>
where
    E: Substitutable,
{
    fn apply_ref(&self, sub: &dyn Substituter) -> Option<Self> {
        self.error.apply_ref(sub).map(|error| Located {
            location: self.location.clone(),
            error,
        })
    }
    fn free_vars(&self) -> Vec<Tvar> {
        self.error.free_vars()
    }
}

#[derive(Debug, Display, PartialEq)]
#[allow(missing_docs)]
pub enum ErrorKind {
    #[display(fmt = "{}", _0)]
    Inference(types::Error),
    #[display(fmt = "undefined builtin identifier {}", _0)]
    UndefinedBuiltin(String),
    #[display(fmt = "undefined identifier {}", _0)]
    UndefinedIdentifier(String),
    #[display(fmt = "invalid binary operator {}", _0)]
    InvalidBinOp(ast::Operator),
    #[display(fmt = "invalid unary operator {}", _0)]
    InvalidUnaryOp(ast::Operator),
    #[display(fmt = "invalid import path {}", _0)]
    InvalidImportPath(String),
    #[display(fmt = "return not valid in file block")]
    InvalidReturn,
    #[display(fmt = "can't vectorize function: {}", _0)]
    UnableToVectorize(String),
    #[display(fmt = "{}. This is a bug in type inference", _0)]
    Bug(String),
}

impl std::error::Error for Error {}

impl Substitutable for ErrorKind {
    fn apply_ref(&self, sub: &dyn Substituter) -> Option<Self> {
        match self {
            Self::Inference(err) => err.apply_ref(sub).map(Self::Inference),
            Self::UndefinedBuiltin(_)
            | Self::UndefinedIdentifier(_)
            | Self::InvalidBinOp(_)
            | Self::InvalidUnaryOp(_)
            | Self::InvalidImportPath(_)
            | Self::UnableToVectorize(_)
            | Self::InvalidReturn
            | Self::Bug(_) => None,
        }
    }
    fn free_vars(&self) -> Vec<Tvar> {
        match self {
            Self::Inference(err) => err.free_vars(),
            Self::UndefinedBuiltin(_)
            | Self::UndefinedIdentifier(_)
            | Self::InvalidBinOp(_)
            | Self::InvalidUnaryOp(_)
            | Self::InvalidImportPath(_)
            | Self::UnableToVectorize(_)
            | Self::InvalidReturn
            | Self::Bug(_) => Vec::new(),
        }
    }
}

impl From<types::Error> for ErrorKind {
    fn from(err: types::Error) -> Self {
        ErrorKind::Inference(err)
    }
}

impl From<infer::Error> for Error {
    fn from(err: infer::Error) -> Self {
        Located {
            location: err.loc,
            error: ErrorKind::Inference(err.err),
        }
    }
}

type VectorizeEnv = HashMap<String, MonoType>;

struct InferState<'a> {
    sub: &'a mut Substitution,
    env: Environment,
    errors: Errors<Error>,
}

impl InferState<'_> {
    fn solve(&mut self, cons: &Constraints) {
        if let Err(err) = infer::solve(cons, self.sub) {
            self.errors.push(err.into());
        }
    }

    fn error(&mut self, loc: ast::SourceLocation, error: ErrorKind) {
        self.errors.push(located(loc, error));
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub enum Statement {
    Expr(ExprStmt),
    Variable(Box<VariableAssgn>),
    Option(Box<OptionStmt>),
    Return(ReturnStmt),
    Test(Box<TestStmt>),
    TestCase(Box<TestCaseStmt>),
    Builtin(BuiltinStmt),
    Error(ast::SourceLocation),
}

impl Statement {
    fn apply(self, sub: &Substitution) -> Self {
        match self {
            Statement::Expr(stmt) => Statement::Expr(stmt.apply(sub)),
            Statement::Variable(stmt) => Statement::Variable(Box::new(stmt.apply(sub))),
            Statement::Option(stmt) => Statement::Option(Box::new(stmt.apply(sub))),
            Statement::Return(stmt) => Statement::Return(stmt.apply(sub)),
            Statement::Test(stmt) => Statement::Test(Box::new(stmt.apply(sub))),
            Statement::TestCase(stmt) => Statement::TestCase(Box::new(stmt.apply(sub))),
            Statement::Builtin(stmt) => Statement::Builtin(stmt.apply(sub)),
            Statement::Error(stmt) => Statement::Error(stmt),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub enum Assignment {
    Variable(VariableAssgn),
    Member(MemberAssgn),
}

impl Assignment {
    fn apply(self, sub: &Substitution) -> Self {
        match self {
            Assignment::Variable(assign) => Assignment::Variable(assign.apply(sub)),
            Assignment::Member(assign) => Assignment::Member(assign.apply(sub)),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub enum Expression {
    Identifier(IdentifierExpr),
    Array(Box<ArrayExpr>),
    Dict(Box<DictExpr>),
    Function(Box<FunctionExpr>),
    Logical(Box<LogicalExpr>),
    Object(Box<ObjectExpr>),
    Member(Box<MemberExpr>),
    Index(Box<IndexExpr>),
    Binary(Box<BinaryExpr>),
    Unary(Box<UnaryExpr>),
    Call(Box<CallExpr>),
    Conditional(Box<ConditionalExpr>),
    StringExpr(Box<StringExpr>),

    Integer(IntegerLit),
    Float(FloatLit),
    StringLit(StringLit),
    Duration(DurationLit),
    Uint(UintLit),
    Boolean(BooleanLit),
    DateTime(DateTimeLit),
    Regexp(RegexpLit),

    Error(ast::SourceLocation),
}

impl Expression {
    #[allow(missing_docs)]
    pub fn type_of(&self) -> MonoType {
        match self {
            Expression::Identifier(e) => e.typ.clone(),
            Expression::Array(e) => e.typ.clone(),
            Expression::Dict(e) => e.typ.clone(),
            Expression::Function(e) => e.typ.clone(),
            Expression::Logical(_) => MonoType::Bool,
            Expression::Object(e) => e.typ.clone(),
            Expression::Member(e) => e.typ.clone(),
            Expression::Index(e) => e.typ.clone(),
            Expression::Binary(e) => e.typ.clone(),
            Expression::Unary(e) => e.typ.clone(),
            Expression::Call(e) => e.typ.clone(),
            Expression::Conditional(e) => e.alternate.type_of(),
            Expression::StringExpr(_) => MonoType::String,
            Expression::Integer(_) => MonoType::Int,
            Expression::Float(_) => MonoType::Float,
            Expression::StringLit(_) => MonoType::String,
            Expression::Duration(_) => MonoType::Duration,
            Expression::Uint(_) => MonoType::Uint,
            Expression::Boolean(_) => MonoType::Bool,
            Expression::DateTime(_) => MonoType::Time,
            Expression::Regexp(_) => MonoType::Regexp,
            Expression::Error(_) => MonoType::Error,
        }
    }
    #[allow(missing_docs)]
    pub fn loc(&self) -> &ast::SourceLocation {
        match self {
            Expression::Identifier(e) => &e.loc,
            Expression::Array(e) => &e.loc,
            Expression::Dict(e) => &e.loc,
            Expression::Function(e) => &e.loc,
            Expression::Logical(e) => &e.loc,
            Expression::Object(e) => &e.loc,
            Expression::Member(e) => &e.loc,
            Expression::Index(e) => &e.loc,
            Expression::Binary(e) => &e.loc,
            Expression::Unary(e) => &e.loc,
            Expression::Call(e) => &e.loc,
            Expression::Conditional(e) => &e.loc,
            Expression::StringExpr(e) => &e.loc,
            Expression::Integer(lit) => &lit.loc,
            Expression::Float(lit) => &lit.loc,
            Expression::StringLit(lit) => &lit.loc,
            Expression::Duration(lit) => &lit.loc,
            Expression::Uint(lit) => &lit.loc,
            Expression::Boolean(lit) => &lit.loc,
            Expression::DateTime(lit) => &lit.loc,
            Expression::Regexp(lit) => &lit.loc,
            Expression::Error(loc) => loc,
        }
    }
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        match self {
            Expression::Identifier(e) => e.infer(infer),
            Expression::Array(e) => e.infer(infer),
            Expression::Dict(e) => e.infer(infer),
            Expression::Function(e) => e.infer(infer),
            Expression::Logical(e) => e.infer(infer),
            Expression::Object(e) => e.infer(infer),
            Expression::Member(e) => e.infer(infer),
            Expression::Index(e) => e.infer(infer),
            Expression::Binary(e) => e.infer(infer),
            Expression::Unary(e) => e.infer(infer),
            Expression::Call(e) => e.infer(infer),
            Expression::Conditional(e) => e.infer(infer),
            Expression::StringExpr(e) => e.infer(infer),
            Expression::Integer(lit) => lit.infer(),
            Expression::Float(lit) => lit.infer(),
            Expression::StringLit(lit) => lit.infer(),
            Expression::Duration(lit) => lit.infer(),
            Expression::Uint(lit) => lit.infer(),
            Expression::Boolean(lit) => lit.infer(),
            Expression::DateTime(lit) => lit.infer(),
            Expression::Regexp(lit) => lit.infer(),
            Expression::Error(_) => Ok(Constraints::empty()),
        }
    }
    fn apply(self, sub: &Substitution) -> Self {
        match self {
            Expression::Identifier(e) => Expression::Identifier(e.apply(sub)),
            Expression::Array(e) => Expression::Array(Box::new(e.apply(sub))),
            Expression::Dict(e) => Expression::Dict(Box::new(e.apply(sub))),
            Expression::Function(e) => Expression::Function(Box::new(e.apply(sub))),
            Expression::Logical(e) => Expression::Logical(Box::new(e.apply(sub))),
            Expression::Object(e) => Expression::Object(Box::new(e.apply(sub))),
            Expression::Member(e) => Expression::Member(Box::new(e.apply(sub))),
            Expression::Index(e) => Expression::Index(Box::new(e.apply(sub))),
            Expression::Binary(e) => Expression::Binary(Box::new(e.apply(sub))),
            Expression::Unary(e) => Expression::Unary(Box::new(e.apply(sub))),
            Expression::Call(e) => Expression::Call(Box::new(e.apply(sub))),
            Expression::Conditional(e) => Expression::Conditional(Box::new(e.apply(sub))),
            Expression::StringExpr(e) => Expression::StringExpr(Box::new(e.apply(sub))),
            Expression::Integer(lit) => Expression::Integer(lit.apply(sub)),
            Expression::Float(lit) => Expression::Float(lit.apply(sub)),
            Expression::StringLit(lit) => Expression::StringLit(lit.apply(sub)),
            Expression::Duration(lit) => Expression::Duration(lit.apply(sub)),
            Expression::Uint(lit) => Expression::Uint(lit.apply(sub)),
            Expression::Boolean(lit) => Expression::Boolean(lit.apply(sub)),
            Expression::DateTime(lit) => Expression::DateTime(lit.apply(sub)),
            Expression::Regexp(lit) => Expression::Regexp(lit.apply(sub)),
            Expression::Error(loc) => Expression::Error(loc),
        }
    }

    fn vectorize(&self, env: &VectorizeEnv) -> Result<Self> {
        Ok(match self {
            Expression::Identifier(identifier) => {
                Expression::Identifier(identifier.vectorize(env)?)
            }
            Expression::Member(member) => {
                let object = member.object.vectorize(env)?;
                let typ = object.type_of();
                Expression::Member(Box::new(MemberExpr {
                    loc: member.loc.clone(),
                    typ: typ
                        .field(&member.property)
                        .ok_or_else(|| {
                            located(
                                member.object.loc().clone(),
                                ErrorKind::UnableToVectorize(format!(
                                    "Expected record type, got `{}`",
                                    typ
                                )),
                            )
                        })?
                        .clone(),
                    object,
                    property: member.property.clone(),
                }))
            }
            _ => {
                return Err(located(
                    self.loc().clone(),
                    ErrorKind::UnableToVectorize("Unable to vectorize expression".into()),
                ))
            }
        })
    }
}

/// Infer the types of a Flux package.
#[allow(missing_docs)]
pub fn infer_package<T>(
    pkg: &mut Package,
    env: Environment,
    sub: &mut Substitution,
    importer: &mut T,
) -> std::result::Result<Environment, Errors<Error>>
where
    T: Importer,
{
    let mut infer = InferState {
        sub,
        env,
        errors: Errors::new(),
    };
    let cons = pkg
        .infer(&mut infer, importer)
        .map_err(|err| err.apply(infer.sub))?;

    infer.solve(&cons);
    if infer.errors.has_errors() {
        for err in &mut infer.errors {
            err.apply_mut(infer.sub);
        }
        Err(infer.errors)
    } else {
        Ok(infer.env)
    }
}

/// Applies the substitution to the entire package.
#[allow(missing_docs)]
pub fn inject_pkg_types(pkg: Package, sub: &Substitution) -> Package {
    pkg.apply(sub)
}

/// Vectorizes a pkg
pub fn vectorize(pkg: &mut Package) -> Result<()> {
    use crate::semantic::walk::{walk_mut, NodeMut, VisitorMut};
    struct Vectorizer {
        result: Result<()>,
    }
    impl VisitorMut for Vectorizer {
        fn visit(&mut self, node: &mut NodeMut) -> bool {
            if self.result.is_err() {
                return false;
            }
            if let NodeMut::FunctionExpr(function) = node {
                match function.vectorize() {
                    Ok(vectorized) => function.vectorized = Some(Box::new(vectorized)),
                    Err(err) => self.result = Err(err),
                }
            }
            true
        }
    }

    let mut visitor = Vectorizer { result: Ok(()) };
    walk_mut(&mut visitor, &mut NodeMut::Package(pkg));
    visitor.result
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct Package {
    pub loc: ast::SourceLocation,

    pub package: String,
    pub files: Vec<File>,
}

impl Package {
    fn infer<T>(&mut self, infer: &mut InferState, importer: &mut T) -> Result
    where
        T: Importer,
    {
        self.files
            .iter_mut()
            .try_fold(Constraints::empty(), |rest, file| {
                let cons = file.infer(infer, importer)?;
                Ok(cons + rest)
            })
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.files = self.files.into_iter().map(|file| file.apply(sub)).collect();
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct File {
    pub loc: ast::SourceLocation,

    pub package: Option<PackageClause>,
    pub imports: Vec<ImportDeclaration>,
    pub body: Vec<Statement>,
}

impl File {
    fn infer<T>(&mut self, infer: &mut InferState, importer: &mut T) -> Result
    where
        T: Importer,
    {
        let mut imports = Vec::with_capacity(self.imports.len());

        for dec in &self.imports {
            let path = &dec.path.value;
            let name = dec.import_name();

            imports.push(name);

            let poly = importer.import(path).unwrap_or_else(|| {
                infer.error(dec.loc.clone(), ErrorKind::InvalidImportPath(path.clone()));
                PolyType::error()
            });
            infer.env.add(name.to_owned(), poly);
        }

        let constraints = self
            .body
            .iter_mut()
            .try_fold(Constraints::empty(), |rest, node| match node {
                Statement::Builtin(stmt) => {
                    stmt.infer(&mut infer.env)?;
                    Ok(rest)
                }
                Statement::Variable(stmt) => {
                    stmt.infer(infer)?;
                    Ok(rest)
                }
                Statement::Option(stmt) => {
                    let cons = stmt.infer(infer)?;
                    Ok(cons + rest)
                }
                Statement::Expr(stmt) => {
                    stmt.infer(infer)?;
                    Ok(rest)
                }
                Statement::Test(stmt) => {
                    stmt.infer(infer)?;
                    Ok(rest)
                }
                Statement::TestCase(stmt) => {
                    let cons = stmt.infer(infer)?;
                    Ok(cons + rest)
                }
                Statement::Return(stmt) => {
                    infer.error(stmt.loc.clone(), ErrorKind::InvalidReturn);
                    Ok::<_, Error>(rest)
                }
                Statement::Error(_) => Ok(rest),
            })?;

        for name in imports {
            infer.env.remove(name);
        }
        Ok(constraints)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.body = self.body.into_iter().map(|stmt| stmt.apply(sub)).collect();
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct PackageClause {
    pub loc: ast::SourceLocation,

    pub name: Identifier,
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct ImportDeclaration {
    pub loc: ast::SourceLocation,

    pub alias: Option<Identifier>,
    pub path: StringLit,
}

impl ImportDeclaration {
    #[allow(missing_docs)]
    pub fn import_name(&self) -> &str {
        let path = &self.path.value;
        match &self.alias {
            None => path.rsplitn(2, '/').next().unwrap(),
            Some(id) => &id.name[..],
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct OptionStmt {
    pub loc: ast::SourceLocation,

    pub assignment: Assignment,
}

impl OptionStmt {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        match &mut self.assignment {
            Assignment::Member(stmt) => {
                let cons = stmt.init.infer(infer)?;
                let rest = stmt.member.infer(infer)?;

                Ok(cons
                    + rest
                    + vec![Constraint::Equal {
                        exp: stmt.member.typ.clone(),
                        act: stmt.init.type_of(),
                        loc: stmt.init.loc().clone(),
                    }]
                    .into())
            }
            Assignment::Variable(stmt) => {
                stmt.infer(infer)?;
                Ok(Constraints::empty())
            }
        }
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.assignment = self.assignment.apply(sub);
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct BuiltinStmt {
    pub loc: ast::SourceLocation,
    pub id: Identifier,
    pub typ_expr: PolyType,
}

impl BuiltinStmt {
    fn infer(&mut self, env: &mut Environment) -> std::result::Result<(), Error> {
        env.add(self.id.name.clone(), self.typ_expr.clone());
        Ok(())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct TestStmt {
    pub loc: ast::SourceLocation,

    pub assignment: VariableAssgn,
}

impl TestStmt {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result<()> {
        self.assignment.infer(infer)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.assignment = self.assignment.apply(sub);
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct TestCaseStmt {
    pub loc: ast::SourceLocation,
    pub id: Identifier,
    pub block: Block,
}

impl TestCaseStmt {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        self.block.infer(infer)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.block = self.block.apply(sub);
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct ExprStmt {
    pub loc: ast::SourceLocation,

    pub expression: Expression,
}

impl ExprStmt {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result<()> {
        let cons = self.expression.infer(infer)?;
        infer.solve(&cons);
        infer.env.apply_mut(infer.sub);
        Ok(())
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.expression = self.expression.apply(sub);
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct ReturnStmt {
    pub loc: ast::SourceLocation,

    pub argument: Expression,
}

impl ReturnStmt {
    #[allow(dead_code)]
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        self.argument.infer(infer)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.argument = self.argument.apply(sub);
        self
    }
}

#[derive(Debug, Derivative, Clone)]
#[derivative(PartialEq)]
#[allow(missing_docs)]
pub struct VariableAssgn {
    #[derivative(PartialEq = "ignore")]
    vars: Vec<Tvar>,

    #[derivative(PartialEq = "ignore")]
    cons: TvarKinds,

    pub loc: ast::SourceLocation,

    pub id: Identifier,
    pub init: Expression,
}

impl VariableAssgn {
    #[allow(missing_docs)]
    pub fn new(id: Identifier, init: Expression, loc: ast::SourceLocation) -> VariableAssgn {
        VariableAssgn {
            vars: Vec::new(),
            cons: TvarKinds::new(),
            loc,
            id,
            init,
        }
    }
    #[allow(missing_docs)]
    pub fn poly_type_of(&self) -> PolyType {
        PolyType {
            vars: self.vars.clone(),
            cons: self.cons.clone(),
            expr: self.init.type_of(),
        }
    }
    // Polymorphic generalization, necessary for let-polymorphism, is
    // implemented here.
    //
    // In particular, for every variable assignment we infer the type of
    // its corresponding expression. We then generalize that type by
    // quantifying over all of its free type variables. Finally we bind
    // the variable to its newly generalized type in the type environment
    // before inferring the rest of the program.
    //
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result<()> {
        let constraints = self.init.infer(infer)?;

        infer.solve(&constraints);

        // Apply substitution to the type environment
        infer.env.apply_mut(infer.sub);

        let t = self.init.type_of().apply(infer.sub);
        let p = infer::generalize(&infer.env, infer.sub.cons(), t);

        // Update variable assignment nodes with the free vars
        // and kind constraints obtained from generalization.
        //
        // Note these variables are fixed after generalization
        // and so it is safe to update these nodes in place.
        self.vars = p.vars.clone();
        self.cons = p.cons.clone();

        // Update the type environment
        infer.env.add(String::from(&self.id.name), p);
        Ok(())
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.init = self.init.apply(sub);
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct MemberAssgn {
    pub loc: ast::SourceLocation,

    pub member: MemberExpr,
    pub init: Expression,
}

impl MemberAssgn {
    fn apply(mut self, sub: &Substitution) -> Self {
        self.member = self.member.apply(sub);
        self.init = self.init.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct StringExpr {
    pub loc: ast::SourceLocation,
    pub parts: Vec<StringExprPart>,
}

impl StringExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let mut constraints = Vec::new();
        for p in &mut self.parts {
            if let StringExprPart::Interpolated(ref mut ip) = p {
                let cons = ip.expression.infer(infer)?;
                constraints.append(&mut Vec::from(cons));
                constraints.push(Constraint::Kind {
                    exp: Kind::Stringable,
                    act: ip.expression.type_of(),
                    loc: ip.expression.loc().clone(),
                });
            }
        }
        Ok(Constraints::from(constraints))
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.parts = self.parts.into_iter().map(|part| part.apply(sub)).collect();
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub enum StringExprPart {
    Text(TextPart),
    Interpolated(InterpolatedPart),
}

impl StringExprPart {
    fn apply(self, sub: &Substitution) -> Self {
        match self {
            StringExprPart::Interpolated(part) => StringExprPart::Interpolated(part.apply(sub)),
            StringExprPart::Text(_) => self,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct TextPart {
    pub loc: ast::SourceLocation,

    pub value: String,
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct InterpolatedPart {
    pub loc: ast::SourceLocation,

    pub expression: Expression,
}

impl InterpolatedPart {
    fn apply(mut self, sub: &Substitution) -> Self {
        self.expression = self.expression.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct ArrayExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub elements: Vec<Expression>,
}

impl ArrayExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let mut cons = Vec::new();
        let elt = MonoType::Var(infer.sub.fresh());
        for el in &mut self.elements {
            let c = el.infer(infer)?;
            cons.append(&mut c.into());
            cons.push(Constraint::Equal {
                exp: elt.clone(),
                act: el.type_of(),
                loc: el.loc().clone(),
            });
        }
        let at = MonoType::from(Array(elt));
        cons.push(Constraint::Equal {
            exp: at,
            act: self.typ.clone(),
            loc: self.loc.clone(),
        });
        Ok(cons.into())
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.elements = self
            .elements
            .into_iter()
            .map(|element| element.apply(sub))
            .collect();
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct DictExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,
    pub elements: Vec<(Expression, Expression)>,
}

impl DictExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let mut cons = Constraints::empty();

        let key = MonoType::Var(infer.sub.fresh());
        let val = MonoType::Var(infer.sub.fresh());

        for (k, v) in &mut self.elements {
            let c0 = k.infer(infer)?;
            let c1 = v.infer(infer)?;

            let kt = k.type_of();
            let vt = v.type_of();

            let kc = Constraint::Equal {
                exp: key.clone(),
                act: kt,
                loc: k.loc().clone(),
            };
            let vc = Constraint::Equal {
                exp: val.clone(),
                act: vt,
                loc: v.loc().clone(),
            };

            cons = cons + c0 + c1 + vec![kc, vc].into();
        }

        let ty = MonoType::from(Dictionary {
            key: key.clone(),
            val,
        });

        let eq = Constraint::Equal {
            exp: ty,
            act: self.typ.clone(),
            loc: self.loc.clone(),
        };
        let tc = Constraint::Kind {
            exp: Kind::Comparable,
            act: key,
            loc: self.loc.clone(),
        };

        Ok(cons + vec![eq, tc].into())
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.elements = self
            .elements
            .into_iter()
            .map(|(key, val)| (key.apply(sub), val.apply(sub)))
            .collect();
        self
    }
}

/// Represents the definition of a function.
#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct FunctionExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub params: Vec<FunctionParameter>,
    pub body: Block,

    pub vectorized: Option<Box<FunctionExpr>>,
}

impl FunctionExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let mut cons = Constraints::empty();
        let mut pipe = None;
        let mut req = MonoTypeMap::new();
        let mut opt = MonoTypeMap::new();
        // This params will build the nested env when inferring the function body.
        let mut params = PolyTypeMap::new();
        for param in &mut self.params {
            match param.default {
                Some(ref mut e) => {
                    let ncons = e.infer(infer)?;
                    cons = cons + ncons;
                    let id = param.key.name.clone();
                    // We are here: `infer = (a=1) => {...}`.
                    // So, this PolyType is actually a MonoType, whose type
                    // is the one of the default value ("1" in "a=1").
                    let typ = PolyType {
                        vars: Vec::new(),
                        cons: TvarKinds::new(),
                        expr: e.type_of(),
                    };
                    params.insert(id.clone(), typ);
                    opt.insert(id, e.type_of());
                }
                None => {
                    // We are here: `infer = (a) => {...}`.
                    // So, we do not know the type of "a". Let's use a fresh TVar.
                    let id = param.key.name.clone();
                    let ftvar = infer.sub.fresh();
                    let typ = PolyType {
                        vars: Vec::new(),
                        cons: TvarKinds::new(),
                        expr: MonoType::Var(ftvar),
                    };
                    params.insert(id.clone(), typ.clone());
                    // Piped arguments cannot have a default value.
                    // So check if this is a piped argument.
                    if param.is_pipe {
                        pipe = Some(types::Property {
                            k: id,
                            v: MonoType::Var(ftvar),
                        });
                    } else {
                        req.insert(id, MonoType::Var(ftvar));
                    }
                }
            };
        }
        // Add the parameters to some nested environment.
        infer.env.enter_scope();
        for (id, param) in params.into_iter() {
            infer.env.add(id, param);
        }
        // And use it to infer the body.
        let bcons = self.body.infer(infer)?;
        // Now pop the nested environment, we don't need it anymore.
        infer.env.exit_scope();
        let retn = self.body.type_of();
        let func = MonoType::from(Function {
            req,
            opt,
            pipe,
            retn,
        });
        cons = cons + bcons;
        cons.add(Constraint::Equal {
            exp: self.typ.clone(),
            act: func,
            loc: self.loc.clone(),
        });
        Ok(cons)
    }
    #[allow(missing_docs)]
    pub fn pipe(&self) -> Option<&FunctionParameter> {
        for p in &self.params {
            if p.is_pipe {
                return Some(p);
            }
        }
        None
    }
    #[allow(missing_docs)]
    pub fn defaults(&self) -> Vec<&FunctionParameter> {
        let mut ds = Vec::new();
        for p in &self.params {
            if p.default.is_some() {
                ds.push(p);
            };
        }
        ds
    }
    #[allow(missing_docs)]
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.params = self
            .params
            .into_iter()
            .map(|param| param.apply(sub))
            .collect();
        self.body = self.body.apply(sub);
        self
    }

    fn vectorize(&self) -> Result<Self> {
        if self.params.len() == 1 && self.params[0].key.name == "r" {
            fn vectorize_fields(record: &MonoType) -> MonoType {
                use crate::semantic::types::Record;
                match record {
                    MonoType::Record(record) => MonoType::from(match &**record {
                        Record::Empty => Record::Empty,
                        Record::Extension { head, tail } => Record::Extension {
                            head: types::Property {
                                k: head.k.clone(),
                                v: MonoType::vector(types::Vector(head.v.clone())),
                            },
                            tail: vectorize_fields(tail),
                        },
                    }),
                    _ => record.clone(),
                }
            }
            let env: VectorizeEnv = self
                .params
                .iter()
                .map(|param| {
                    let parameter_type =
                        vectorize_fields(self.typ.parameter(&param.key.name).unwrap());
                    (param.key.name.clone(), parameter_type)
                })
                .collect();
            let body = match &self.body {
                Block::Variable(..) | Block::Expr(..) => {
                    return Err(located(
                        self.body.loc().clone(),
                        ErrorKind::UnableToVectorize("Unable to vectorize statements".into()),
                    ))
                }
                Block::Return(e) => {
                    let argument = match &e.argument {
                        Expression::Object(e) => {
                            let properties = e
                                .properties
                                .iter()
                                .map(|p| {
                                    Ok(Property {
                                        loc: p.loc.clone(),
                                        key: p.key.clone(),
                                        value: p.value.vectorize(&env)?,
                                    })
                                })
                                .collect::<Result<Vec<_>>>()?;

                            let with = e
                                .with
                                .as_ref()
                                .map(|with| with.vectorize(&env))
                                .transpose()?;

                            Expression::Object(Box::new(ObjectExpr {
                                loc: e.loc.clone(),
                                typ: MonoType::from(types::Record::new(
                                    properties.iter().map(|p| types::Property {
                                        k: p.key.name.clone(),
                                        v: p.value.type_of(),
                                    }),
                                    with.as_ref().map(|with| with.typ.clone()),
                                )),
                                with,
                                properties,
                            }))
                        }
                        _ => {
                            return Err(located(
                                e.argument.loc().clone(),
                                ErrorKind::UnableToVectorize(
                                    "Vectorization only supports returning a record".into(),
                                ),
                            ))
                        }
                    };
                    Block::Return(ReturnStmt {
                        loc: e.loc.clone(),
                        argument,
                    })
                }
            };
            Ok(FunctionExpr {
                loc: self.loc.clone(),
                typ: self.typ.clone(), // TODO Correct the type
                params: self.params.clone(),
                body,
                vectorized: None,
            })
        } else {
            // Only `map` will get vectorized to start with, so only try to vectorize such functions
            Err(located(
                self.loc.clone(),
                ErrorKind::UnableToVectorize("Does not match the `map` signature".into()),
            ))
        }
    }
}

/// Represents a function block and is equivalent to a let-expression
/// in other functional languages.
///
/// Functions must evaluate to a value in Flux. In other words, a function
/// must always have a return value. This means a function block is by
/// definition an expression.
///
/// A function block is an expression that evaluates to the argument of
/// its terminating ReturnStmt.
#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub enum Block {
    Variable(Box<VariableAssgn>, Box<Block>),
    Expr(ExprStmt, Box<Block>),
    Return(ReturnStmt),
}

impl Block {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        match self {
            Block::Variable(stmt, block) => {
                stmt.infer(infer)?;
                let cons = block.infer(infer)?;

                Ok(cons)
            }
            Block::Expr(stmt, block) => {
                stmt.infer(infer)?;
                let cons = block.infer(infer)?;

                Ok(cons)
            }
            Block::Return(e) => e.infer(infer),
        }
    }
    #[allow(missing_docs)]
    pub fn loc(&self) -> &ast::SourceLocation {
        match self {
            Block::Variable(assign, _) => &assign.loc,
            Block::Expr(es, _) => es.expression.loc(),
            Block::Return(ret) => &ret.loc,
        }
    }
    #[allow(missing_docs)]
    pub fn type_of(&self) -> MonoType {
        let mut n = self;
        loop {
            n = match n {
                Block::Variable(_, b) => b.as_ref(),
                Block::Expr(_, b) => b.as_ref(),
                Block::Return(r) => return r.argument.type_of(),
            }
        }
    }
    fn apply(self, sub: &Substitution) -> Self {
        match self {
            Block::Variable(assign, next) => {
                Block::Variable(Box::new(assign.apply(sub)), Box::new(next.apply(sub)))
            }
            Block::Expr(es, next) => Block::Expr(es.apply(sub), Box::new(next.apply(sub))),
            Block::Return(e) => Block::Return(e.apply(sub)),
        }
    }
}

/// FunctionParameter represents a function parameter.
#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct FunctionParameter {
    pub loc: ast::SourceLocation,

    pub is_pipe: bool,
    pub key: Identifier,
    pub default: Option<Expression>,
}

impl FunctionParameter {
    fn apply(mut self, sub: &Substitution) -> Self {
        match self.default {
            Some(e) => {
                self.default = Some(e.apply(sub));
                self
            }
            None => self,
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct BinaryExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub operator: ast::Operator,
    pub left: Expression,
    pub right: Expression,
}

impl BinaryExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        // Compute the left and right constraints.
        // Do this first so that we can return an error if one occurs.
        let lcons = self.left.infer(infer)?;
        let rcons = self.right.infer(infer)?;

        let binop_arithmetic_constraints = |kind| {
            Constraints::from(vec![
                Constraint::Equal {
                    exp: self.left.type_of(),
                    act: self.right.type_of(),
                    loc: self.right.loc().clone(),
                },
                Constraint::Equal {
                    exp: self.left.type_of(),
                    act: self.typ.clone(),
                    loc: self.loc.clone(),
                },
                Constraint::Kind {
                    act: self.typ.clone(),
                    exp: kind,
                    loc: self.loc.clone(),
                },
            ])
        };
        let binop_compare_constraints = |kind| {
            Constraints::from(vec![
                // https://github.com/influxdata/flux/issues/2393
                // Constraint::Equal{self.left.type_of(), self.right.type_of()),
                Constraint::Equal {
                    act: self.typ.clone(),
                    exp: MonoType::Bool,
                    loc: self.loc.clone(),
                },
                Constraint::Kind {
                    act: self.left.type_of(),
                    exp: kind,
                    loc: self.left.loc().clone(),
                },
                Constraint::Kind {
                    act: self.right.type_of(),
                    exp: kind,
                    loc: self.right.loc().clone(),
                },
            ])
        };
        let cons = match self.operator {
            // The following operators require both sides to be equal.
            ast::Operator::AdditionOperator => binop_arithmetic_constraints(Kind::Addable),
            ast::Operator::SubtractionOperator => binop_arithmetic_constraints(Kind::Subtractable),
            ast::Operator::MultiplicationOperator
            | ast::Operator::DivisionOperator
            | ast::Operator::PowerOperator
            | ast::Operator::ModuloOperator => binop_arithmetic_constraints(Kind::Divisible),
            ast::Operator::GreaterThanOperator | ast::Operator::LessThanOperator => {
                binop_compare_constraints(Kind::Comparable)
            }
            ast::Operator::EqualOperator | ast::Operator::NotEqualOperator => {
                binop_compare_constraints(Kind::Equatable)
            }
            ast::Operator::GreaterThanEqualOperator | ast::Operator::LessThanEqualOperator => {
                Constraints::from(vec![
                    // https://github.com/influxdata/flux/issues/2393
                    // Constraint::Equal{self.left.type_of(), self.right.type_of()),
                    Constraint::Equal {
                        act: self.typ.clone(),
                        exp: MonoType::Bool,
                        loc: self.loc.clone(),
                    },
                    Constraint::Kind {
                        act: self.left.type_of(),
                        exp: Kind::Equatable,
                        loc: self.left.loc().clone(),
                    },
                    Constraint::Kind {
                        act: self.left.type_of(),
                        exp: Kind::Comparable,
                        loc: self.left.loc().clone(),
                    },
                    Constraint::Kind {
                        act: self.right.type_of(),
                        exp: Kind::Equatable,
                        loc: self.right.loc().clone(),
                    },
                    Constraint::Kind {
                        act: self.right.type_of(),
                        exp: Kind::Comparable,
                        loc: self.right.loc().clone(),
                    },
                ])
            }
            // Regular expression operators.
            ast::Operator::RegexpMatchOperator | ast::Operator::NotRegexpMatchOperator => {
                Constraints::from(vec![
                    Constraint::Equal {
                        act: self.typ.clone(),
                        exp: MonoType::Bool,
                        loc: self.loc.clone(),
                    },
                    Constraint::Equal {
                        act: self.left.type_of(),
                        exp: MonoType::String,
                        loc: self.left.loc().clone(),
                    },
                    Constraint::Equal {
                        act: self.right.type_of(),
                        exp: MonoType::Regexp,
                        loc: self.right.loc().clone(),
                    },
                ])
            }
            _ => {
                infer.error(
                    self.loc.clone(),
                    ErrorKind::InvalidBinOp(self.operator.clone()),
                );
                Constraints::empty()
            }
        };

        // Otherwise, add the constraints together and return them.
        Ok(lcons + rcons + cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.left = self.left.apply(sub);
        self.right = self.right.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct CallExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub callee: Expression,
    pub arguments: Vec<Property>,
    pub pipe: Option<Expression>,
}

impl CallExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        // First, recursively infer every type of the children of this call expression,
        // update the environment and the constraints, and use the inferred types to
        // build the fields of the type for this call expression.
        let mut cons = self.callee.infer(infer)?;
        let mut req = MonoTypeMap::new();
        let mut pipe = None;
        for Property {
            key: ref mut id,
            value: ref mut expr,
            ..
        } in &mut self.arguments
        {
            let ncons = expr.infer(infer)?;
            cons = cons + ncons;
            // Every argument is required in a function call.
            req.insert(id.name.clone(), expr.type_of());
        }
        if let Some(ref mut p) = &mut self.pipe {
            let ncons = p.infer(infer)?;
            cons = cons + ncons;
            pipe = Some(types::Property {
                k: "<-".to_string(),
                v: p.type_of(),
            });
        }
        // Constrain the callee to be a Function.
        cons.add(Constraint::Equal {
            exp: self.callee.type_of(),
            act: MonoType::from(Function {
                opt: MonoTypeMap::new(),
                req,
                pipe,
                // The return type of a function call is the type of the call itself.
                // Remind that, when two functions are unified, their return types are unified too.
                // As an example take:
                //   f = (a) => a + 1
                //   f(a: 0)
                // The return type of `f` is `int`.
                // The return type of `f(a: 0)` is `t0` (a fresh type variable).
                // Upon unification a substitution "t0 => int" is created, so that the compiler
                // can infer that, for instance, `f(a: 0) + 1` is legal.
                retn: self.typ.clone(),
            }),
            loc: self.loc.clone(),
        });
        Ok(cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.callee = self.callee.apply(sub);
        self.arguments = self
            .arguments
            .into_iter()
            .map(|arg| arg.apply(sub))
            .collect();
        match self.pipe {
            Some(e) => {
                self.pipe = Some(e.apply(sub));
                self
            }
            None => self,
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct ConditionalExpr {
    pub loc: ast::SourceLocation,
    pub test: Expression,
    pub consequent: Expression,
    pub alternate: Expression,
}

impl ConditionalExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let tcons = self.test.infer(infer)?;
        let ccons = self.consequent.infer(infer)?;
        let acons = self.alternate.infer(infer)?;
        let cons = tcons
            + ccons
            + acons
            + Constraints::from(vec![
                Constraint::Equal {
                    exp: MonoType::Bool,
                    act: self.test.type_of(),
                    loc: self.test.loc().clone(),
                },
                Constraint::Equal {
                    exp: self.consequent.type_of(),
                    act: self.alternate.type_of(),
                    loc: self.alternate.loc().clone(),
                },
            ]);
        Ok(cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.test = self.test.apply(sub);
        self.consequent = self.consequent.apply(sub);
        self.alternate = self.alternate.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct LogicalExpr {
    pub loc: ast::SourceLocation,
    pub operator: ast::LogicalOperator,
    pub left: Expression,
    pub right: Expression,
}

impl LogicalExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let lcons = self.left.infer(infer)?;
        let rcons = self.right.infer(infer)?;
        let cons = lcons
            + rcons
            + Constraints::from(vec![
                Constraint::Equal {
                    exp: MonoType::Bool,
                    act: self.left.type_of(),
                    loc: self.left.loc().clone(),
                },
                Constraint::Equal {
                    exp: MonoType::Bool,
                    act: self.right.type_of(),
                    loc: self.right.loc().clone(),
                },
            ]);
        Ok(cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.left = self.left.apply(sub);
        self.right = self.right.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct MemberExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub object: Expression,
    pub property: String,
}

impl MemberExpr {
    // A member expression such as `r.a` produces the constraint:
    //
    //     type_of(r) = {a: type_of(r.a) | 'r}
    //
    // where 'r is a fresh type variable.
    //
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let head = types::Property {
            k: self.property.to_owned(),
            v: self.typ.to_owned(),
        };
        let tail = MonoType::Var(infer.sub.fresh());

        let r = MonoType::from(types::Record::Extension { head, tail });

        let cons = self.object.infer(infer)?;
        let t = self.object.type_of();

        Ok(cons
            + vec![Constraint::Equal {
                exp: r,
                act: t,
                loc: self.object.loc().clone(),
            }]
            .into())
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.object = self.object.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct IndexExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub array: Expression,
    pub index: Expression,
}

impl IndexExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let acons = self.array.infer(infer)?;
        let icons = self.index.infer(infer)?;
        let cons = acons
            + icons
            + Constraints::from(vec![
                Constraint::Equal {
                    act: self.index.type_of(),
                    exp: MonoType::Int,
                    loc: self.index.loc().clone(),
                },
                Constraint::Equal {
                    act: self.array.type_of(),
                    exp: MonoType::from(Array(self.typ.clone())),
                    loc: self.array.loc().clone(),
                },
            ]);
        Ok(cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.array = self.array.apply(sub);
        self.index = self.index.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct ObjectExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub with: Option<IdentifierExpr>,
    pub properties: Vec<Property>,
}

impl ObjectExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        // If record extension, infer constraints for base
        let (mut r, mut cons) = match &mut self.with {
            Some(expr) => {
                let cons = expr.infer(infer)?;
                (expr.typ.to_owned(), cons)
            }
            None => (MonoType::from(types::Record::Empty), Constraints::empty()),
        };
        // Infer constraints for properties
        for prop in self.properties.iter_mut().rev() {
            let rest = prop.value.infer(infer)?;
            cons = cons + rest;
            r = MonoType::from(types::Record::Extension {
                head: types::Property {
                    k: prop.key.name.to_owned(),
                    v: prop.value.type_of(),
                },
                tail: r,
            });
        }
        Ok(cons
            + vec![Constraint::Equal {
                exp: self.typ.to_owned(),
                act: r,
                loc: self.loc.clone(),
            }]
            .into())
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        if let Some(e) = self.with {
            self.with = Some(e.apply(sub));
        }
        self.properties = self
            .properties
            .into_iter()
            .map(|prop| prop.apply(sub))
            .collect();
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct UnaryExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub operator: ast::Operator,
    pub argument: Expression,
}

impl UnaryExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let acons = self.argument.infer(infer)?;
        let cons = match self.operator {
            ast::Operator::NotOperator => Constraints::from(vec![
                Constraint::Equal {
                    act: self.argument.type_of(),
                    exp: MonoType::Bool,
                    loc: self.argument.loc().clone(),
                },
                Constraint::Equal {
                    act: self.typ.clone(),
                    exp: MonoType::Bool,
                    loc: self.loc.clone(),
                },
            ]),
            ast::Operator::ExistsOperator => Constraints::from(Constraint::Equal {
                act: self.typ.clone(),
                exp: MonoType::Bool,
                loc: self.loc.clone(),
            }),
            ast::Operator::AdditionOperator | ast::Operator::SubtractionOperator => {
                Constraints::from(vec![
                    Constraint::Equal {
                        act: self.argument.type_of(),
                        exp: self.typ.clone(),
                        loc: self.loc.clone(),
                    },
                    Constraint::Kind {
                        act: self.argument.type_of(),
                        exp: Kind::Negatable,
                        loc: self.argument.loc().clone(),
                    },
                ])
            }
            _ => {
                infer.error(
                    self.loc.clone(),
                    ErrorKind::InvalidUnaryOp(self.operator.clone()),
                );
                Constraints::empty()
            }
        };
        Ok(acons + cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self.argument = self.argument.apply(sub);
        self
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct Property {
    pub loc: ast::SourceLocation,

    pub key: Identifier,
    pub value: Expression,
}

impl Property {
    fn apply(mut self, sub: &Substitution) -> Self {
        self.value = self.value.apply(sub);
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct IdentifierExpr {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub typ: MonoType,

    pub name: String,
}

impl IdentifierExpr {
    fn infer(&mut self, infer: &mut InferState<'_>) -> Result {
        let poly = infer.env.lookup(&self.name).cloned().unwrap_or_else(|| {
            infer.error(
                self.loc.clone(),
                ErrorKind::UndefinedIdentifier(self.name.to_string()),
            );
            PolyType::error()
        });

        let (t, cons) = infer::instantiate(poly, infer.sub, self.loc.clone());
        self.typ = t;
        Ok(cons)
    }
    fn apply(mut self, sub: &Substitution) -> Self {
        self.typ = self.typ.apply(sub);
        self
    }

    fn vectorize(&self, env: &VectorizeEnv) -> Result<Self> {
        let typ = env.get(&self.name).unwrap_or(&self.typ).clone();

        Ok(IdentifierExpr {
            loc: self.loc.clone(),
            typ,
            name: self.name.clone(),
        })
    }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct Identifier {
    pub loc: ast::SourceLocation,

    pub name: String,
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct BooleanLit {
    pub loc: ast::SourceLocation,
    pub value: bool,
}

impl BooleanLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct IntegerLit {
    pub loc: ast::SourceLocation,
    pub value: i64,
}

impl IntegerLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct FloatLit {
    pub loc: ast::SourceLocation,
    pub value: f64,
}

impl FloatLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct RegexpLit {
    pub loc: ast::SourceLocation,
    pub value: String,
}

impl RegexpLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct StringLit {
    pub loc: ast::SourceLocation,
    pub value: String,
}

impl StringLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct UintLit {
    pub loc: ast::SourceLocation,
    pub value: u64,
}

impl UintLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct DateTimeLit {
    pub loc: ast::SourceLocation,
    pub value: DateTime<FixedOffset>,
}

impl DateTimeLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

/// A struct that keeps track of time in months and nanoseconds.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(rename = "Duration")]
pub struct Duration {
    /// Must be a positive value.
    pub months: i64,
    /// Must be a positive value.
    pub nanoseconds: i64,
    /// Indicates whether the magnitude of durations converted from the AST have a positive or
    /// negative value. This field is `true` when magnitudes are negative.
    pub negative: bool,
}

/// The atomic unit from which all duration literals are composed.
///
/// A `DurationLit` is a pair consisting of a length of time and the unit of time measured.
#[derive(Derivative)]
#[derivative(Debug, PartialEq, Clone)]
#[allow(missing_docs)]
pub struct DurationLit {
    pub loc: ast::SourceLocation,
    #[derivative(PartialEq = "ignore")]
    pub value: Duration,
}

impl DurationLit {
    fn infer(&mut self) -> Result {
        Ok(Constraints::empty())
    }
    fn apply(self, _: &Substitution) -> Self {
        self
    }
}

// The following durations have nanosecond base units
const NANOS: i64 = 1;
const MICROS: i64 = NANOS * 1000;
const MILLIS: i64 = MICROS * 1000;
const SECONDS: i64 = MILLIS * 1000;
const MINUTES: i64 = SECONDS * 60;
const HOURS: i64 = MINUTES * 60;
const DAYS: i64 = HOURS * 24;
const WEEKS: i64 = DAYS * 7;

// The following durations have month base units
const MONTHS: i64 = 1;
const YEARS: i64 = MONTHS * 12;

/// Convert an [`ast::Duration`] node to its semantic counterpart [`Duration`].
///
/// Returns a `Result` type with a possible error message.
pub fn convert_duration(ast_dur: &[ast::Duration]) -> AnyhowResult<Duration> {
    if ast_dur.is_empty() {
        bail!("AST duration vector must contain at least one duration value");
    };

    let negative = ast_dur[0].magnitude.is_negative();

    let (nanoseconds, months) = ast_dur.iter().try_fold((0i64, 0i64), |acc, d| {
        if (d.magnitude.is_negative() && !negative) || (!d.magnitude.is_negative() && negative) {
            bail!("all values in AST duration vector must have the same sign");
        }

        match d.unit.as_str() {
            "y" => Ok((acc.0, acc.1 + d.magnitude * YEARS)),
            "mo" => Ok((acc.0, acc.1 + d.magnitude * MONTHS)),
            "w" => Ok((acc.0 + d.magnitude * WEEKS, acc.1)),
            "d" => Ok((acc.0 + d.magnitude * DAYS, acc.1)),
            "h" => Ok((acc.0 + d.magnitude * HOURS, acc.1)),
            "m" => Ok((acc.0 + d.magnitude * MINUTES, acc.1)),
            "s" => Ok((acc.0 + d.magnitude * SECONDS, acc.1)),
            "ms" => Ok((acc.0 + d.magnitude * MILLIS, acc.1)),
            "us" | "µs" => Ok((acc.0 + d.magnitude * MICROS, acc.1)),
            "ns" => Ok((acc.0 + d.magnitude * NANOS, acc.1)),
            _ => Err(anyhow!("unrecognized magnitude for duration")),
        }
    })?;

    let nanoseconds = nanoseconds.abs();
    let months = months.abs();

    Ok(Duration {
        months,
        nanoseconds,
        negative,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast;
    use crate::semantic::types::{MonoType, Tvar};
    use crate::semantic::walk::{walk, Node};

    #[test]
    fn duration_conversion_ok() {
        let t = vec![
            ast::Duration {
                magnitude: 1,
                unit: "y".to_string(),
            },
            ast::Duration {
                magnitude: 2,
                unit: "mo".to_string(),
            },
            ast::Duration {
                magnitude: 3,
                unit: "w".to_string(),
            },
            ast::Duration {
                magnitude: 4,
                unit: "m".to_string(),
            },
            ast::Duration {
                magnitude: 5,
                unit: "ns".to_string(),
            },
        ];
        let expect_nano = 3 * WEEKS + 4 * MINUTES + 5 * NANOS;
        let expect_months = 1 * YEARS + 2 * MONTHS;

        let got = convert_duration(&t).unwrap();
        assert_eq!(expect_nano, got.nanoseconds);
        assert_eq!(expect_months, got.months);
        assert_eq!(false, got.negative);
    }

    #[test]
    fn duration_conversion_same_magnitude_twice() {
        let t = vec![
            ast::Duration {
                magnitude: 1,
                unit: "y".to_string(),
            },
            ast::Duration {
                magnitude: 2,
                unit: "mo".to_string(),
            },
            ast::Duration {
                magnitude: 3,
                unit: "y".to_string(),
            },
        ];
        let expect_nano = 0;
        let expect_months = 4 * YEARS + 2 * MONTHS;

        let got = convert_duration(&t).unwrap();
        assert_eq!(expect_nano, got.nanoseconds);
        assert_eq!(expect_months, got.months);
        assert_eq!(false, got.negative);
    }

    #[test]
    fn duration_conversion_negative_ok() {
        let t = vec![
            ast::Duration {
                magnitude: -1,
                unit: "y".to_string(),
            },
            ast::Duration {
                magnitude: -2,
                unit: "mo".to_string(),
            },
            ast::Duration {
                magnitude: -3,
                unit: "w".to_string(),
            },
        ];
        let expect_months = (-1 * YEARS + (-2 * MONTHS)).abs();
        let expect_nano = (-3 * WEEKS).abs();

        let got = convert_duration(&t).unwrap();
        assert_eq!(expect_nano, got.nanoseconds);
        assert_eq!(expect_months, got.months);
        assert_eq!(true, got.negative);
    }

    #[test]
    fn duration_conversion_unit_error() {
        let t = vec![
            ast::Duration {
                magnitude: -1,
                unit: "y".to_string(),
            },
            ast::Duration {
                magnitude: -2,
                unit: "--idk--".to_string(),
            },
            ast::Duration {
                magnitude: -3,
                unit: "w".to_string(),
            },
        ];
        let exp = "unrecognized magnitude for duration";
        let got = convert_duration(&t).err().expect("should be an error");
        assert_eq!(exp, got.to_string());
    }

    #[test]
    fn duration_conversion_different_signs_error() {
        let t = vec![
            ast::Duration {
                magnitude: -1,
                unit: "y".to_string(),
            },
            ast::Duration {
                magnitude: 2,
                unit: "ns".to_string(),
            },
            ast::Duration {
                magnitude: -3,
                unit: "w".to_string(),
            },
        ];
        let exp = "all values in AST duration vector must have the same sign";
        let got = convert_duration(&t).err().expect("should be an error");
        assert_eq!(exp, got.to_string());
    }

    #[test]
    fn duration_conversion_empty_error() {
        let t = Vec::new();
        let exp = "AST duration vector must contain at least one duration value";
        let got = convert_duration(&t).err().expect("should be an error");
        assert_eq!(exp, got.to_string());
    }

    #[test]
    fn test_inject_types() {
        let b = ast::BaseNode::default();
        let pkg = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![
                    Statement::Variable(Box::new(VariableAssgn::new(
                        Identifier {
                            loc: b.location.clone(),
                            name: "f".to_string(),
                        },
                        Expression::Function(Box::new(FunctionExpr {
                            loc: b.location.clone(),
                            typ: MonoType::Var(Tvar(0)),
                            params: vec![
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: true,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "piped".to_string(),
                                    },
                                    default: None,
                                },
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: false,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "a".to_string(),
                                    },
                                    default: None,
                                },
                            ],
                            body: Block::Return(ReturnStmt {
                                loc: b.location.clone(),
                                argument: Expression::Binary(Box::new(BinaryExpr {
                                    loc: b.location.clone(),
                                    typ: MonoType::Var(Tvar(1)),
                                    operator: ast::Operator::AdditionOperator,
                                    left: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: MonoType::Var(Tvar(2)),
                                        name: "a".to_string(),
                                    }),
                                    right: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: MonoType::Var(Tvar(3)),
                                        name: "piped".to_string(),
                                    }),
                                })),
                            }),
                            vectorized: None,
                        })),
                        b.location.clone(),
                    ))),
                    Statement::Expr(ExprStmt {
                        loc: b.location.clone(),
                        expression: Expression::Call(Box::new(CallExpr {
                            loc: b.location.clone(),
                            typ: MonoType::Var(Tvar(4)),
                            pipe: Some(Expression::Integer(IntegerLit {
                                loc: b.location.clone(),
                                value: 3,
                            })),
                            callee: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: MonoType::Var(Tvar(6)),
                                name: "f".to_string(),
                            }),
                            arguments: vec![Property {
                                loc: b.location.clone(),
                                key: Identifier {
                                    loc: b.location.clone(),
                                    name: "a".to_string(),
                                },
                                value: Expression::Integer(IntegerLit {
                                    loc: b.location.clone(),
                                    value: 2,
                                }),
                            }],
                        })),
                    }),
                ],
            }],
        };
        let sub: Substitution = semantic_map! {
            Tvar(0) => MonoType::Int,
            Tvar(1) => MonoType::Int,
            Tvar(2) => MonoType::Int,
            Tvar(3) => MonoType::Int,
            Tvar(4) => MonoType::Int,
            Tvar(5) => MonoType::Int,
            Tvar(6) => MonoType::Int,
            Tvar(7) => MonoType::Int,
        }
        .into();
        let pkg = inject_pkg_types(pkg, &sub);
        let mut no_types_checked = 0;
        walk(
            &mut |node: Node| {
                let typ = node.type_of();
                if let Some(typ) = typ {
                    assert_eq!(typ, MonoType::Int);
                    no_types_checked += 1;
                }
            },
            Node::Package(&pkg),
        );
        assert_eq!(no_types_checked, 8);
    }
}
