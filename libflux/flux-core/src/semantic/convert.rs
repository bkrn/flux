//! Various conversions from AST nodes to their associated
//! types in the semantic graph.

use crate::ast;
use crate::semantic::nodes::*;
use crate::semantic::sub::Substitution;
use crate::semantic::types;
use crate::semantic::types::MonoType;
use crate::semantic::types::MonoTypeMap;
use crate::semantic::types::SemanticMap;
use std::collections::BTreeMap;

use thiserror::Error;

/// Error that categorizes errors when converting from AST to semantic graph.
#[derive(Error, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Error {
    #[error("TestCase is not supported in semantic analysis")]
    TestCase,
    #[error("invalid named type {0}")]
    InvalidNamedType(String),
    #[error("function types can have at most one pipe parameter")]
    AtMostOnePipe,
    #[error("invalid constraint {0}")]
    InvalidConstraint(String),
    #[error("a pipe literal may only be used as a default value for an argument in a function definition")]
    InvalidPipeLit,
    #[error("function parameters must be identifiers")]
    FunctionParameterIdents,
    #[error("missing return statement in block")]
    MissingReturn,
    #[error("invalid {0} statement in function block")]
    InvalidFunctionStatement(&'static str),
    #[error("function parameters is not a record expression")]
    ParametersNotRecord,
    #[error("function parameters are more than one record expression")]
    ExtraParameterRecord,
    #[error("invalid duration, {0}")]
    InvalidDuration(String),
}

/// Result encapsulates any error during the conversion process.
pub type Result<T> = std::result::Result<T, Error>;

/// convert_package converts an [AST package] node to its semantic representation using
/// the provided [`Fresher`].
///
/// Note: most external callers of this function will want to use the analyze()
/// function in the flux crate instead, which is aware of everything in the Flux stdlib and prelude.
///
/// The function explicitly moves the `ast::Package` because it adds information to it.
/// We follow here the principle that every compilation step should be isolated and should add meaning
/// to the previous one. In other terms, once one converts an AST he should not use it anymore.
/// If one wants to do so, he should explicitly pkg.clone() and incur consciously in the memory
/// overhead involved.
///
/// [AST package]: ast::Package
pub fn convert_package(pkg: ast::Package, sub: &mut Substitution) -> Result<Package> {
    let files = pkg
        .files
        .into_iter()
        .map(|file| convert_file(file, sub))
        .collect::<Result<Vec<File>>>()?;
    Ok(Package {
        loc: pkg.base.location,
        package: pkg.package,
        files,
    })
}

fn convert_file(file: ast::File, sub: &mut Substitution) -> Result<File> {
    let package = convert_package_clause(file.package, sub)?;
    let imports = file
        .imports
        .into_iter()
        .map(|i| convert_import_declaration(i, sub))
        .collect::<Result<Vec<ImportDeclaration>>>()?;
    let body = file
        .body
        .into_iter()
        .map(|s| convert_statement(s, sub))
        .collect::<Result<Vec<Statement>>>()?;
    Ok(File {
        loc: file.base.location,
        package,
        imports,
        body,
    })
}

fn convert_package_clause(
    pkg: Option<ast::PackageClause>,
    sub: &mut Substitution,
) -> Result<Option<PackageClause>> {
    if pkg.is_none() {
        return Ok(None);
    }
    let pkg = pkg.unwrap();
    let name = convert_identifier(pkg.name, sub)?;
    Ok(Some(PackageClause {
        loc: pkg.base.location,
        name,
    }))
}

fn convert_import_declaration(
    imp: ast::ImportDeclaration,
    sub: &mut Substitution,
) -> Result<ImportDeclaration> {
    let alias = match imp.alias {
        None => None,
        Some(id) => Some(convert_identifier(id, sub)?),
    };
    let path = convert_string_literal(imp.path, sub)?;
    Ok(ImportDeclaration {
        loc: imp.base.location,
        alias,
        path,
    })
}

fn convert_statement(stmt: ast::Statement, sub: &mut Substitution) -> Result<Statement> {
    match stmt {
        ast::Statement::Option(s) => Ok(Statement::Option(Box::new(convert_option_statement(
            *s, sub,
        )?))),
        ast::Statement::Builtin(s) => Ok(Statement::Builtin(convert_builtin_statement(*s, sub)?)),
        ast::Statement::Test(s) => Ok(Statement::Test(Box::new(convert_test_statement(*s, sub)?))),
        ast::Statement::TestCase(_) => Err(Error::TestCase),
        ast::Statement::Expr(s) => Ok(Statement::Expr(convert_expression_statement(*s, sub)?)),
        ast::Statement::Return(s) => Ok(Statement::Return(convert_return_statement(*s, sub)?)),
        // TODO(affo): we should fix this to include MemberAssignement.
        //  The error lies in AST: the Statement enum does not include that.
        //  This is not a problem when parsing, because we parse it only in the option assignment case,
        //  and we return an OptionStmt, which is a Statement.
        ast::Statement::Variable(s) => Ok(Statement::Variable(Box::new(
            convert_variable_assignment(*s, sub)?,
        ))),
        ast::Statement::Bad(s) => Ok(Statement::Error(s.base.location.clone())),
    }
}

fn convert_assignment(assign: ast::Assignment, sub: &mut Substitution) -> Result<Assignment> {
    match assign {
        ast::Assignment::Variable(a) => {
            Ok(Assignment::Variable(convert_variable_assignment(*a, sub)?))
        }
        ast::Assignment::Member(a) => Ok(Assignment::Member(convert_member_assignment(*a, sub)?)),
    }
}

fn convert_option_statement(stmt: ast::OptionStmt, sub: &mut Substitution) -> Result<OptionStmt> {
    Ok(OptionStmt {
        loc: stmt.base.location,
        assignment: convert_assignment(stmt.assignment, sub)?,
    })
}

fn convert_builtin_statement(
    stmt: ast::BuiltinStmt,
    sub: &mut Substitution,
) -> Result<BuiltinStmt> {
    Ok(BuiltinStmt {
        loc: stmt.base.location,
        id: convert_identifier(stmt.id, sub)?,
        typ_expr: convert_polytype(stmt.ty, sub)?,
    })
}

pub(crate) fn convert_monotype(
    ty: ast::MonoType,
    tvars: &mut BTreeMap<String, types::Tvar>,
    sub: &mut Substitution,
) -> Result<MonoType> {
    match ty {
        ast::MonoType::Tvar(tv) => {
            let tvar = tvars.entry(tv.name.name).or_insert_with(|| sub.fresh());
            Ok(MonoType::Var(*tvar))
        }
        ast::MonoType::Basic(basic) => match basic.name.name.as_str() {
            "bool" => Ok(MonoType::Bool),
            "int" => Ok(MonoType::Int),
            "uint" => Ok(MonoType::Uint),
            "float" => Ok(MonoType::Float),
            "string" => Ok(MonoType::String),
            "duration" => Ok(MonoType::Duration),
            "time" => Ok(MonoType::Time),
            "regexp" => Ok(MonoType::Regexp),
            "bytes" => Ok(MonoType::Bytes),
            _ => Err(Error::InvalidNamedType(basic.name.name.to_string())),
        },
        ast::MonoType::Array(arr) => Ok(MonoType::from(types::Array(convert_monotype(
            arr.element,
            tvars,
            sub,
        )?))),
        ast::MonoType::Dict(dict) => {
            let key = convert_monotype(dict.key, tvars, sub)?;
            let val = convert_monotype(dict.val, tvars, sub)?;
            Ok(MonoType::from(types::Dictionary { key, val }))
        }
        ast::MonoType::Function(func) => {
            let mut req = MonoTypeMap::new();
            let mut opt = MonoTypeMap::new();
            let mut _pipe = None;
            let mut dirty = false;
            for param in func.parameters {
                match param {
                    ast::ParameterType::Required { name, monotype, .. } => {
                        req.insert(name.name, convert_monotype(monotype, tvars, sub)?);
                    }
                    ast::ParameterType::Optional { name, monotype, .. } => {
                        opt.insert(name.name, convert_monotype(monotype, tvars, sub)?);
                    }
                    ast::ParameterType::Pipe { name, monotype, .. } => {
                        if !dirty {
                            _pipe = Some(types::Property {
                                k: match name {
                                    Some(n) => n.name,
                                    None => String::from("<-"),
                                },
                                v: convert_monotype(monotype, tvars, sub)?,
                            });
                            dirty = true;
                        } else {
                            return Err(Error::AtMostOnePipe);
                        }
                    }
                }
            }
            Ok(MonoType::from(types::Function {
                req,
                opt,
                pipe: _pipe,
                retn: convert_monotype(func.monotype, tvars, sub)?,
            }))
        }
        ast::MonoType::Record(rec) => {
            let mut r = match rec.tvar {
                None => MonoType::from(types::Record::Empty),
                Some(id) => {
                    let tv = ast::MonoType::Tvar(ast::TvarType {
                        base: id.clone().base,
                        name: id,
                    });
                    convert_monotype(tv, tvars, sub)?
                }
            };
            for prop in rec.properties {
                let property = types::Property {
                    k: prop.name.name,
                    v: convert_monotype(prop.monotype, tvars, sub)?,
                };
                r = MonoType::from(types::Record::Extension {
                    head: property,
                    tail: r,
                })
            }
            Ok(r)
        }
    }
}

/// Converts a [type expression] in the AST into a [`PolyType`].
///
/// [type expression]: ast::TypeExpression
/// [`PolyType`]: types::PolyType
pub fn convert_polytype(
    type_expression: ast::TypeExpression,
    sub: &mut Substitution,
) -> Result<types::PolyType> {
    let mut tvars = BTreeMap::<String, types::Tvar>::new();
    let expr = convert_monotype(type_expression.monotype, &mut tvars, sub)?;
    let mut vars = Vec::<types::Tvar>::new();
    let mut cons = SemanticMap::<types::Tvar, Vec<types::Kind>>::new();

    for (name, tvar) in tvars {
        vars.push(tvar);
        let mut kinds = Vec::<types::Kind>::new();
        for con in &type_expression.constraints {
            if con.tvar.name == name {
                for k in &con.kinds {
                    match k.name.as_str() {
                        "Addable" => kinds.push(types::Kind::Addable),
                        "Subtractable" => kinds.push(types::Kind::Subtractable),
                        "Divisible" => kinds.push(types::Kind::Divisible),
                        "Numeric" => kinds.push(types::Kind::Numeric),
                        "Comparable" => kinds.push(types::Kind::Comparable),
                        "Equatable" => kinds.push(types::Kind::Equatable),
                        "Nullable" => kinds.push(types::Kind::Nullable),
                        "Negatable" => kinds.push(types::Kind::Negatable),
                        "Timeable" => kinds.push(types::Kind::Timeable),
                        "Record" => kinds.push(types::Kind::Record),
                        "Stringable" => kinds.push(types::Kind::Stringable),
                        _ => {
                            return Err(Error::InvalidConstraint(k.name.clone()));
                        }
                    }
                }
                cons.insert(tvar, kinds.clone());
            }
        }
    }
    Ok(types::PolyType { vars, cons, expr })
}

fn convert_test_statement(stmt: ast::TestStmt, sub: &mut Substitution) -> Result<TestStmt> {
    Ok(TestStmt {
        loc: stmt.base.location,
        assignment: convert_variable_assignment(stmt.assignment, sub)?,
    })
}

fn convert_expression_statement(stmt: ast::ExprStmt, sub: &mut Substitution) -> Result<ExprStmt> {
    Ok(ExprStmt {
        loc: stmt.base.location,
        expression: convert_expression(stmt.expression, sub)?,
    })
}

fn convert_return_statement(stmt: ast::ReturnStmt, sub: &mut Substitution) -> Result<ReturnStmt> {
    Ok(ReturnStmt {
        loc: stmt.base.location,
        argument: convert_expression(stmt.argument, sub)?,
    })
}

fn convert_variable_assignment(
    stmt: ast::VariableAssgn,
    sub: &mut Substitution,
) -> Result<VariableAssgn> {
    Ok(VariableAssgn::new(
        convert_identifier(stmt.id, sub)?,
        convert_expression(stmt.init, sub)?,
        stmt.base.location,
    ))
}

fn convert_member_assignment(
    stmt: ast::MemberAssgn,
    sub: &mut Substitution,
) -> Result<MemberAssgn> {
    Ok(MemberAssgn {
        loc: stmt.base.location,
        member: convert_member_expression(stmt.member, sub)?,
        init: convert_expression(stmt.init, sub)?,
    })
}

fn convert_expression(expr: ast::Expression, sub: &mut Substitution) -> Result<Expression> {
    match expr {
        ast::Expression::Function(expr) => Ok(Expression::Function(Box::new(
            convert_function_expression(*expr, sub)?,
        ))),
        ast::Expression::Call(expr) => Ok(Expression::Call(Box::new(convert_call_expression(
            *expr, sub,
        )?))),
        ast::Expression::Member(expr) => Ok(Expression::Member(Box::new(
            convert_member_expression(*expr, sub)?,
        ))),
        ast::Expression::Index(expr) => Ok(Expression::Index(Box::new(convert_index_expression(
            *expr, sub,
        )?))),
        ast::Expression::PipeExpr(expr) => Ok(Expression::Call(Box::new(convert_pipe_expression(
            *expr, sub,
        )?))),
        ast::Expression::Binary(expr) => Ok(Expression::Binary(Box::new(
            convert_binary_expression(*expr, sub)?,
        ))),
        ast::Expression::Unary(expr) => Ok(Expression::Unary(Box::new(convert_unary_expression(
            *expr, sub,
        )?))),
        ast::Expression::Logical(expr) => Ok(Expression::Logical(Box::new(
            convert_logical_expression(*expr, sub)?,
        ))),
        ast::Expression::Conditional(expr) => Ok(Expression::Conditional(Box::new(
            convert_conditional_expression(*expr, sub)?,
        ))),
        ast::Expression::Object(expr) => Ok(Expression::Object(Box::new(
            convert_object_expression(*expr, sub)?,
        ))),
        ast::Expression::Array(expr) => Ok(Expression::Array(Box::new(convert_array_expression(
            *expr, sub,
        )?))),
        ast::Expression::Dict(expr) => Ok(Expression::Dict(Box::new(convert_dict_expression(
            *expr, sub,
        )?))),
        ast::Expression::Identifier(expr) => Ok(Expression::Identifier(
            convert_identifier_expression(expr, sub)?,
        )),
        ast::Expression::StringExpr(expr) => Ok(Expression::StringExpr(Box::new(
            convert_string_expression(*expr, sub)?,
        ))),
        ast::Expression::Paren(expr) => convert_expression(expr.expression, sub),
        ast::Expression::StringLit(lit) => {
            Ok(Expression::StringLit(convert_string_literal(lit, sub)?))
        }
        ast::Expression::Boolean(lit) => {
            Ok(Expression::Boolean(convert_boolean_literal(lit, sub)?))
        }
        ast::Expression::Float(lit) => Ok(Expression::Float(convert_float_literal(lit, sub)?)),
        ast::Expression::Integer(lit) => {
            Ok(Expression::Integer(convert_integer_literal(lit, sub)?))
        }
        ast::Expression::Uint(lit) => Ok(Expression::Uint(convert_unsigned_integer_literal(
            lit, sub,
        )?)),
        ast::Expression::Regexp(lit) => Ok(Expression::Regexp(convert_regexp_literal(lit, sub)?)),
        ast::Expression::Duration(lit) => {
            Ok(Expression::Duration(convert_duration_literal(lit, sub)?))
        }
        ast::Expression::DateTime(lit) => {
            Ok(Expression::DateTime(convert_date_time_literal(lit, sub)?))
        }
        ast::Expression::PipeLit(_) => Err(Error::InvalidPipeLit),
        ast::Expression::Bad(bad) => Ok(Expression::Error(bad.base.location.clone())),
    }
}

fn convert_function_expression(
    expr: ast::FunctionExpr,
    sub: &mut Substitution,
) -> Result<FunctionExpr> {
    let params = convert_function_params(expr.params, sub)?;
    let body = convert_function_body(expr.body, sub)?;
    Ok(FunctionExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        params,
        body,
        vectorized: None,
    })
}

fn convert_function_params(
    props: Vec<ast::Property>,
    sub: &mut Substitution,
) -> Result<Vec<FunctionParameter>> {
    // The iteration here is complex, cannot use iter().map()..., better to write it explicitly.
    let mut params: Vec<FunctionParameter> = Vec::new();
    let mut piped = false;
    for prop in props {
        let id = match prop.key {
            ast::PropertyKey::Identifier(id) => Ok(id),
            _ => Err(Error::FunctionParameterIdents),
        }?;
        let key = convert_identifier(id, sub)?;
        let mut default: Option<Expression> = None;
        let mut is_pipe = false;
        if let Some(expr) = prop.value {
            match expr {
                ast::Expression::PipeLit(_) => {
                    if piped {
                        return Err(Error::AtMostOnePipe);
                    } else {
                        piped = true;
                        is_pipe = true;
                    };
                }
                e => default = Some(convert_expression(e, sub)?),
            }
        };
        params.push(FunctionParameter {
            loc: prop.base.location,
            is_pipe,
            key,
            default,
        });
    }
    Ok(params)
}

fn convert_function_body(body: ast::FunctionBody, sub: &mut Substitution) -> Result<Block> {
    match body {
        ast::FunctionBody::Expr(expr) => {
            let argument = convert_expression(expr, sub)?;
            Ok(Block::Return(ReturnStmt {
                loc: argument.loc().clone(),
                argument,
            }))
        }
        ast::FunctionBody::Block(block) => Ok(convert_block(block, sub)?),
    }
}

fn convert_block(block: ast::Block, sub: &mut Substitution) -> Result<Block> {
    let mut body = block.body.into_iter().rev();

    let block = if let Some(ast::Statement::Return(stmt)) = body.next() {
        let argument = convert_expression(stmt.argument, sub)?;
        Block::Return(ReturnStmt {
            loc: stmt.base.location.clone(),
            argument,
        })
    } else {
        return Err(Error::MissingReturn);
    };

    body.try_fold(block, |acc, s| match s {
        ast::Statement::Variable(dec) => Ok(Block::Variable(
            Box::new(convert_variable_assignment(*dec, sub)?),
            Box::new(acc),
        )),
        ast::Statement::Expr(stmt) => Ok(Block::Expr(
            convert_expression_statement(*stmt, sub)?,
            Box::new(acc),
        )),
        _ => Err(Error::InvalidFunctionStatement(s.type_name())),
    })
}

fn convert_call_expression(expr: ast::CallExpr, sub: &mut Substitution) -> Result<CallExpr> {
    let callee = convert_expression(expr.callee, sub)?;
    // TODO(affo): I'd prefer these checks to be in ast.Check().
    if expr.arguments.len() > 1 {
        return Err(Error::ExtraParameterRecord);
    }
    let mut args = expr
        .arguments
        .into_iter()
        .map(|a| match a {
            ast::Expression::Object(obj) => convert_object_expression(*obj, sub),
            _ => Err(Error::ParametersNotRecord),
        })
        .collect::<Result<Vec<ObjectExpr>>>()?;
    let arguments = match args.len() {
        0 => Ok(Vec::new()),
        1 => Ok(args.pop().expect("there must be 1 element").properties),
        _ => Err(Error::ExtraParameterRecord),
    }?;
    Ok(CallExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        callee,
        arguments,
        pipe: None,
    })
}

fn convert_member_expression(expr: ast::MemberExpr, sub: &mut Substitution) -> Result<MemberExpr> {
    let object = convert_expression(expr.object, sub)?;
    let property = match expr.property {
        ast::PropertyKey::Identifier(id) => id.name,
        ast::PropertyKey::StringLit(lit) => lit.value,
    };
    Ok(MemberExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        object,
        property,
    })
}

fn convert_index_expression(expr: ast::IndexExpr, sub: &mut Substitution) -> Result<IndexExpr> {
    let array = convert_expression(expr.array, sub)?;
    let index = convert_expression(expr.index, sub)?;
    Ok(IndexExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        array,
        index,
    })
}

fn convert_pipe_expression(expr: ast::PipeExpr, sub: &mut Substitution) -> Result<CallExpr> {
    let mut call = convert_call_expression(expr.call, sub)?;
    let pipe = convert_expression(expr.argument, sub)?;
    call.pipe = Some(pipe);
    Ok(call)
}

fn convert_binary_expression(expr: ast::BinaryExpr, sub: &mut Substitution) -> Result<BinaryExpr> {
    let left = convert_expression(expr.left, sub)?;
    let right = convert_expression(expr.right, sub)?;
    Ok(BinaryExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        operator: expr.operator,
        left,
        right,
    })
}

fn convert_unary_expression(expr: ast::UnaryExpr, sub: &mut Substitution) -> Result<UnaryExpr> {
    let argument = convert_expression(expr.argument, sub)?;
    Ok(UnaryExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        operator: expr.operator,
        argument,
    })
}

fn convert_logical_expression(
    expr: ast::LogicalExpr,
    sub: &mut Substitution,
) -> Result<LogicalExpr> {
    let left = convert_expression(expr.left, sub)?;
    let right = convert_expression(expr.right, sub)?;
    Ok(LogicalExpr {
        loc: expr.base.location,
        operator: expr.operator,
        left,
        right,
    })
}

fn convert_conditional_expression(
    expr: ast::ConditionalExpr,
    sub: &mut Substitution,
) -> Result<ConditionalExpr> {
    let test = convert_expression(expr.test, sub)?;
    let consequent = convert_expression(expr.consequent, sub)?;
    let alternate = convert_expression(expr.alternate, sub)?;
    Ok(ConditionalExpr {
        loc: expr.base.location,
        test,
        consequent,
        alternate,
    })
}

fn convert_object_expression(expr: ast::ObjectExpr, sub: &mut Substitution) -> Result<ObjectExpr> {
    let properties = expr
        .properties
        .into_iter()
        .map(|p| convert_property(p, sub))
        .collect::<Result<Vec<Property>>>()?;
    let with = match expr.with {
        Some(with) => Some(convert_identifier_expression(with.source, sub)?),
        None => None,
    };
    Ok(ObjectExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        with,
        properties,
    })
}

fn convert_property(prop: ast::Property, sub: &mut Substitution) -> Result<Property> {
    let key = match prop.key {
        ast::PropertyKey::Identifier(id) => convert_identifier(id, sub)?,
        ast::PropertyKey::StringLit(lit) => Identifier {
            loc: lit.base.location.clone(),
            name: convert_string_literal(lit, sub)?.value,
        },
    };
    let value = match prop.value {
        Some(expr) => convert_expression(expr, sub)?,
        None => Expression::Identifier(IdentifierExpr {
            loc: key.loc.clone(),
            typ: MonoType::Var(sub.fresh()),
            name: key.name.clone(),
        }),
    };
    Ok(Property {
        loc: prop.base.location,
        key,
        value,
    })
}

fn convert_array_expression(expr: ast::ArrayExpr, sub: &mut Substitution) -> Result<ArrayExpr> {
    let elements = expr
        .elements
        .into_iter()
        .map(|e| convert_expression(e.expression, sub))
        .collect::<Result<Vec<Expression>>>()?;
    Ok(ArrayExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        elements,
    })
}

fn convert_dict_expression(expr: ast::DictExpr, sub: &mut Substitution) -> Result<DictExpr> {
    let mut elements = Vec::new();
    for item in expr.elements.into_iter() {
        elements.push((
            convert_expression(item.key, sub)?,
            convert_expression(item.val, sub)?,
        ));
    }
    Ok(DictExpr {
        loc: expr.base.location,
        typ: MonoType::Var(sub.fresh()),
        elements,
    })
}

fn convert_identifier(id: ast::Identifier, _sub: &mut Substitution) -> Result<Identifier> {
    Ok(Identifier {
        loc: id.base.location,
        name: id.name,
    })
}

fn convert_identifier_expression(
    id: ast::Identifier,
    sub: &mut Substitution,
) -> Result<IdentifierExpr> {
    Ok(IdentifierExpr {
        loc: id.base.location,
        typ: MonoType::Var(sub.fresh()),
        name: id.name,
    })
}

fn convert_string_expression(expr: ast::StringExpr, sub: &mut Substitution) -> Result<StringExpr> {
    let parts = expr
        .parts
        .into_iter()
        .map(|p| convert_string_expression_part(p, sub))
        .collect::<Result<Vec<StringExprPart>>>()?;
    Ok(StringExpr {
        loc: expr.base.location,
        parts,
    })
}

fn convert_string_expression_part(
    expr: ast::StringExprPart,
    sub: &mut Substitution,
) -> Result<StringExprPart> {
    match expr {
        ast::StringExprPart::Text(txt) => Ok(StringExprPart::Text(TextPart {
            loc: txt.base.location,
            value: txt.value,
        })),
        ast::StringExprPart::Interpolated(itp) => {
            Ok(StringExprPart::Interpolated(InterpolatedPart {
                loc: itp.base.location,
                expression: convert_expression(itp.expression, sub)?,
            }))
        }
    }
}

fn convert_string_literal(lit: ast::StringLit, _: &mut Substitution) -> Result<StringLit> {
    Ok(StringLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

fn convert_boolean_literal(lit: ast::BooleanLit, _: &mut Substitution) -> Result<BooleanLit> {
    Ok(BooleanLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

fn convert_float_literal(lit: ast::FloatLit, _: &mut Substitution) -> Result<FloatLit> {
    Ok(FloatLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

fn convert_integer_literal(lit: ast::IntegerLit, _: &mut Substitution) -> Result<IntegerLit> {
    Ok(IntegerLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

fn convert_unsigned_integer_literal(lit: ast::UintLit, _: &mut Substitution) -> Result<UintLit> {
    Ok(UintLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

fn convert_regexp_literal(lit: ast::RegexpLit, _: &mut Substitution) -> Result<RegexpLit> {
    Ok(RegexpLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

fn convert_duration_literal(lit: ast::DurationLit, _: &mut Substitution) -> Result<DurationLit> {
    Ok(DurationLit {
        loc: lit.base.location,
        value: convert_duration(&lit.values).map_err(|e| Error::InvalidDuration(e.to_string()))?,
    })
}

fn convert_date_time_literal(lit: ast::DateTimeLit, _: &mut Substitution) -> Result<DateTimeLit> {
    Ok(DateTimeLit {
        loc: lit.base.location,
        value: lit.value,
    })
}

// In these tests we test the results of semantic analysis on some ASTs.
// NOTE: we do not care about locations.
// We create a default base node and clone it in various AST nodes.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::sub;
    use crate::semantic::types::{MonoType, Tvar};
    use pretty_assertions::assert_eq;

    // type_info() is used for the expected semantic graph.
    // The id for the Tvar does not matter, because that is not compared.
    fn type_info() -> MonoType {
        MonoType::Var(Tvar(0))
    }

    fn test_convert(pkg: ast::Package) -> Result<Package> {
        convert_package(pkg, &mut sub::Substitution::default())
    }

    #[test]
    fn test_convert_empty() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: Vec::new(),
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: Vec::new(),
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_package() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: Some(ast::PackageClause {
                    base: b.clone(),
                    name: ast::Identifier {
                        base: b.clone(),
                        name: "foo".to_string(),
                    },
                }),
                imports: Vec::new(),
                body: Vec::new(),
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: Some(PackageClause {
                    loc: b.location.clone(),
                    name: Identifier {
                        loc: b.location.clone(),
                        name: "foo".to_string(),
                    },
                }),
                imports: Vec::new(),
                body: Vec::new(),
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_imports() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: Some(ast::PackageClause {
                    base: b.clone(),
                    name: ast::Identifier {
                        base: b.clone(),
                        name: "foo".to_string(),
                    },
                }),
                imports: vec![
                    ast::ImportDeclaration {
                        base: b.clone(),
                        path: ast::StringLit {
                            base: b.clone(),
                            value: "path/foo".to_string(),
                        },
                        alias: None,
                    },
                    ast::ImportDeclaration {
                        base: b.clone(),
                        path: ast::StringLit {
                            base: b.clone(),
                            value: "path/bar".to_string(),
                        },
                        alias: Some(ast::Identifier {
                            base: b.clone(),
                            name: "b".to_string(),
                        }),
                    },
                ],
                body: Vec::new(),
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: Some(PackageClause {
                    loc: b.location.clone(),
                    name: Identifier {
                        loc: b.location.clone(),
                        name: "foo".to_string(),
                    },
                }),
                imports: vec![
                    ImportDeclaration {
                        loc: b.location.clone(),
                        path: StringLit {
                            loc: b.location.clone(),
                            value: "path/foo".to_string(),
                        },
                        alias: None,
                    },
                    ImportDeclaration {
                        loc: b.location.clone(),
                        path: StringLit {
                            loc: b.location.clone(),
                            value: "path/bar".to_string(),
                        },
                        alias: Some(Identifier {
                            loc: b.location.clone(),
                            name: "b".to_string(),
                        }),
                    },
                ],
                body: Vec::new(),
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_var_assignment() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![
                    ast::Statement::Variable(Box::new(ast::VariableAssgn {
                        base: b.clone(),
                        id: ast::Identifier {
                            base: b.clone(),
                            name: "a".to_string(),
                        },
                        init: ast::Expression::Boolean(ast::BooleanLit {
                            base: b.clone(),
                            value: true,
                        }),
                    })),
                    ast::Statement::Expr(Box::new(ast::ExprStmt {
                        base: b.clone(),
                        expression: ast::Expression::Identifier(ast::Identifier {
                            base: b.clone(),
                            name: "a".to_string(),
                        }),
                    })),
                ],
                eof: vec![],
            }],
        };
        let want = Package {
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
                            name: "a".to_string(),
                        },
                        Expression::Boolean(BooleanLit {
                            loc: b.location.clone(),
                            value: true,
                        }),
                        b.location.clone(),
                    ))),
                    Statement::Expr(ExprStmt {
                        loc: b.location.clone(),
                        expression: Expression::Identifier(IdentifierExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            name: "a".to_string(),
                        }),
                    }),
                ],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_object() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Object(Box::new(ast::ObjectExpr {
                        base: b.clone(),
                        lbrace: vec![],
                        with: None,
                        properties: vec![ast::Property {
                            base: b.clone(),
                            key: ast::PropertyKey::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "a".to_string(),
                            }),
                            separator: vec![],
                            value: Some(ast::Expression::Integer(ast::IntegerLit {
                                base: b.clone(),
                                value: 10,
                            })),
                            comma: vec![],
                        }],
                        rbrace: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Object(Box::new(ObjectExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        with: None,
                        properties: vec![Property {
                            loc: b.location.clone(),
                            key: Identifier {
                                loc: b.location.clone(),
                                name: "a".to_string(),
                            },
                            value: Expression::Integer(IntegerLit {
                                loc: b.location.clone(),
                                value: 10,
                            }),
                        }],
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_object_with_string_key() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Object(Box::new(ast::ObjectExpr {
                        base: b.clone(),
                        lbrace: vec![],
                        with: None,
                        properties: vec![ast::Property {
                            base: b.clone(),
                            key: ast::PropertyKey::StringLit(ast::StringLit {
                                base: b.clone(),
                                value: "a".to_string(),
                            }),
                            separator: vec![],
                            value: Some(ast::Expression::Integer(ast::IntegerLit {
                                base: b.clone(),
                                value: 10,
                            })),
                            comma: vec![],
                        }],
                        rbrace: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Object(Box::new(ObjectExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        with: None,
                        properties: vec![Property {
                            loc: b.location.clone(),
                            key: Identifier {
                                loc: b.location.clone(),
                                name: "a".to_string(),
                            },
                            value: Expression::Integer(IntegerLit {
                                loc: b.location.clone(),
                                value: 10,
                            }),
                        }],
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_object_with_mixed_keys() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Object(Box::new(ast::ObjectExpr {
                        base: b.clone(),
                        lbrace: vec![],
                        with: None,
                        properties: vec![
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::StringLit(ast::StringLit {
                                    base: b.clone(),
                                    value: "a".to_string(),
                                }),
                                separator: vec![],
                                value: Some(ast::Expression::Integer(ast::IntegerLit {
                                    base: b.clone(),
                                    value: 10,
                                })),
                                comma: vec![],
                            },
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "b".to_string(),
                                }),
                                separator: vec![],
                                value: Some(ast::Expression::Integer(ast::IntegerLit {
                                    base: b.clone(),
                                    value: 11,
                                })),
                                comma: vec![],
                            },
                        ],
                        rbrace: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Object(Box::new(ObjectExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        with: None,
                        properties: vec![
                            Property {
                                loc: b.location.clone(),
                                key: Identifier {
                                    loc: b.location.clone(),
                                    name: "a".to_string(),
                                },
                                value: Expression::Integer(IntegerLit {
                                    loc: b.location.clone(),
                                    value: 10,
                                }),
                            },
                            Property {
                                loc: b.location.clone(),
                                key: Identifier {
                                    loc: b.location.clone(),
                                    name: "b".to_string(),
                                },
                                value: Expression::Integer(IntegerLit {
                                    loc: b.location.clone(),
                                    value: 11,
                                }),
                            },
                        ],
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_object_with_implicit_keys() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Object(Box::new(ast::ObjectExpr {
                        base: b.clone(),
                        lbrace: vec![],
                        with: None,
                        properties: vec![
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "a".to_string(),
                                }),
                                separator: vec![],
                                value: None,
                                comma: vec![],
                            },
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "b".to_string(),
                                }),
                                separator: vec![],
                                value: None,
                                comma: vec![],
                            },
                        ],
                        rbrace: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Object(Box::new(ObjectExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        with: None,
                        properties: vec![
                            Property {
                                loc: b.location.clone(),
                                key: Identifier {
                                    loc: b.location.clone(),
                                    name: "a".to_string(),
                                },
                                value: Expression::Identifier(IdentifierExpr {
                                    loc: b.location.clone(),
                                    typ: type_info(),
                                    name: "a".to_string(),
                                }),
                            },
                            Property {
                                loc: b.location.clone(),
                                key: Identifier {
                                    loc: b.location.clone(),
                                    name: "b".to_string(),
                                },
                                value: Expression::Identifier(IdentifierExpr {
                                    loc: b.location.clone(),
                                    typ: type_info(),
                                    name: "b".to_string(),
                                }),
                            },
                        ],
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_options_declaration() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Option(Box::new(ast::OptionStmt {
                    base: b.clone(),
                    assignment: ast::Assignment::Variable(Box::new(ast::VariableAssgn {
                        base: b.clone(),
                        id: ast::Identifier {
                            base: b.clone(),
                            name: "task".to_string(),
                        },
                        init: ast::Expression::Object(Box::new(ast::ObjectExpr {
                            base: b.clone(),
                            lbrace: vec![],
                            with: None,
                            properties: vec![
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "name".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::StringLit(ast::StringLit {
                                        base: b.clone(),
                                        value: "foo".to_string(),
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "every".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Duration(ast::DurationLit {
                                        base: b.clone(),
                                        values: vec![ast::Duration {
                                            magnitude: 1,
                                            unit: "h".to_string(),
                                        }],
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "delay".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Duration(ast::DurationLit {
                                        base: b.clone(),
                                        values: vec![ast::Duration {
                                            magnitude: 10,
                                            unit: "m".to_string(),
                                        }],
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "cron".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::StringLit(ast::StringLit {
                                        base: b.clone(),
                                        value: "0 2 * * *".to_string(),
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "retry".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Integer(ast::IntegerLit {
                                        base: b.clone(),
                                        value: 5,
                                    })),
                                    comma: vec![],
                                },
                            ],
                            rbrace: vec![],
                        })),
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Option(Box::new(OptionStmt {
                    loc: b.location.clone(),
                    assignment: Assignment::Variable(VariableAssgn::new(
                        Identifier {
                            loc: b.location.clone(),
                            name: "task".to_string(),
                        },
                        Expression::Object(Box::new(ObjectExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            with: None,
                            properties: vec![
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "name".to_string(),
                                    },
                                    value: Expression::StringLit(StringLit {
                                        loc: b.location.clone(),
                                        value: "foo".to_string(),
                                    }),
                                },
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "every".to_string(),
                                    },
                                    value: Expression::Duration(DurationLit {
                                        loc: b.location.clone(),
                                        value: Duration {
                                            months: 5,
                                            nanoseconds: 5000,
                                            negative: false,
                                        },
                                    }),
                                },
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "delay".to_string(),
                                    },
                                    value: Expression::Duration(DurationLit {
                                        loc: b.location.clone(),
                                        value: Duration {
                                            months: 1,
                                            nanoseconds: 50,
                                            negative: true,
                                        },
                                    }),
                                },
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "cron".to_string(),
                                    },
                                    value: Expression::StringLit(StringLit {
                                        loc: b.location.clone(),
                                        value: "0 2 * * *".to_string(),
                                    }),
                                },
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "retry".to_string(),
                                    },
                                    value: Expression::Integer(IntegerLit {
                                        loc: b.location.clone(),
                                        value: 5,
                                    }),
                                },
                            ],
                        })),
                        b.location.clone(),
                    )),
                }))],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_qualified_option_statement() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Option(Box::new(ast::OptionStmt {
                    base: b.clone(),
                    assignment: ast::Assignment::Member(Box::new(ast::MemberAssgn {
                        base: b.clone(),
                        member: ast::MemberExpr {
                            base: b.clone(),
                            object: ast::Expression::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "alert".to_string(),
                            }),
                            lbrack: vec![],
                            property: ast::PropertyKey::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "state".to_string(),
                            }),
                            rbrack: vec![],
                        },
                        init: ast::Expression::StringLit(ast::StringLit {
                            base: b.clone(),
                            value: "Warning".to_string(),
                        }),
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Option(Box::new(OptionStmt {
                    loc: b.location.clone(),
                    assignment: Assignment::Member(MemberAssgn {
                        loc: b.location.clone(),
                        member: MemberExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            object: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                name: "alert".to_string(),
                            }),
                            property: "state".to_string(),
                        },
                        init: Expression::StringLit(StringLit {
                            loc: b.location.clone(),
                            value: "Warning".to_string(),
                        }),
                    }),
                }))],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_function() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![
                    ast::Statement::Variable(Box::new(ast::VariableAssgn {
                        base: b.clone(),
                        id: ast::Identifier {
                            base: b.clone(),
                            name: "f".to_string(),
                        },
                        init: ast::Expression::Function(Box::new(ast::FunctionExpr {
                            base: b.clone(),
                            lparen: vec![],
                            params: vec![
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "a".to_string(),
                                    }),
                                    separator: vec![],
                                    value: None,
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "b".to_string(),
                                    }),
                                    separator: vec![],
                                    value: None,
                                    comma: vec![],
                                },
                            ],
                            rparen: vec![],
                            arrow: vec![],
                            body: ast::FunctionBody::Expr(ast::Expression::Binary(Box::new(
                                ast::BinaryExpr {
                                    base: b.clone(),
                                    operator: ast::Operator::AdditionOperator,
                                    left: ast::Expression::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "a".to_string(),
                                    }),
                                    right: ast::Expression::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "b".to_string(),
                                    }),
                                },
                            ))),
                        })),
                    })),
                    ast::Statement::Expr(Box::new(ast::ExprStmt {
                        base: b.clone(),
                        expression: ast::Expression::Call(Box::new(ast::CallExpr {
                            base: b.clone(),
                            callee: ast::Expression::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "f".to_string(),
                            }),
                            lparen: vec![],
                            arguments: vec![ast::Expression::Object(Box::new(ast::ObjectExpr {
                                base: b.clone(),
                                lbrace: vec![],
                                with: None,
                                properties: vec![
                                    ast::Property {
                                        base: b.clone(),
                                        key: ast::PropertyKey::Identifier(ast::Identifier {
                                            base: b.clone(),
                                            name: "a".to_string(),
                                        }),
                                        separator: vec![],
                                        value: Some(ast::Expression::Integer(ast::IntegerLit {
                                            base: b.clone(),
                                            value: 2,
                                        })),
                                        comma: vec![],
                                    },
                                    ast::Property {
                                        base: b.clone(),
                                        key: ast::PropertyKey::Identifier(ast::Identifier {
                                            base: b.clone(),
                                            name: "b".to_string(),
                                        }),
                                        separator: vec![],
                                        value: Some(ast::Expression::Integer(ast::IntegerLit {
                                            base: b.clone(),
                                            value: 3,
                                        })),
                                        comma: vec![],
                                    },
                                ],
                                rbrace: vec![],
                            }))],
                            rparen: vec![],
                        })),
                    })),
                ],
                eof: vec![],
            }],
        };
        let want = Package {
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
                            typ: type_info(),
                            params: vec![
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: false,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "a".to_string(),
                                    },
                                    default: None,
                                },
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: false,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "b".to_string(),
                                    },
                                    default: None,
                                },
                            ],
                            body: Block::Return(ReturnStmt {
                                loc: b.location.clone(),
                                argument: Expression::Binary(Box::new(BinaryExpr {
                                    loc: b.location.clone(),
                                    typ: type_info(),
                                    operator: ast::Operator::AdditionOperator,
                                    left: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: type_info(),
                                        name: "a".to_string(),
                                    }),
                                    right: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: type_info(),
                                        name: "b".to_string(),
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
                            typ: type_info(),
                            pipe: None,
                            callee: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                name: "f".to_string(),
                            }),
                            arguments: vec![
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "a".to_string(),
                                    },
                                    value: Expression::Integer(IntegerLit {
                                        loc: b.location.clone(),
                                        value: 2,
                                    }),
                                },
                                Property {
                                    loc: b.location.clone(),
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "b".to_string(),
                                    },
                                    value: Expression::Integer(IntegerLit {
                                        loc: b.location.clone(),
                                        value: 3,
                                    }),
                                },
                            ],
                        })),
                    }),
                ],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_function_with_defaults() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![
                    ast::Statement::Variable(Box::new(ast::VariableAssgn {
                        base: b.clone(),
                        id: ast::Identifier {
                            base: b.clone(),
                            name: "f".to_string(),
                        },
                        init: ast::Expression::Function(Box::new(ast::FunctionExpr {
                            base: b.clone(),
                            lparen: vec![],
                            params: vec![
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "a".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Integer(ast::IntegerLit {
                                        base: b.clone(),
                                        value: 0,
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "b".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Integer(ast::IntegerLit {
                                        base: b.clone(),
                                        value: 0,
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "c".to_string(),
                                    }),
                                    separator: vec![],
                                    value: None,
                                    comma: vec![],
                                },
                            ],
                            rparen: vec![],
                            arrow: vec![],
                            body: ast::FunctionBody::Expr(ast::Expression::Binary(Box::new(
                                ast::BinaryExpr {
                                    base: b.clone(),
                                    operator: ast::Operator::AdditionOperator,
                                    left: ast::Expression::Binary(Box::new(ast::BinaryExpr {
                                        base: b.clone(),
                                        operator: ast::Operator::AdditionOperator,
                                        left: ast::Expression::Identifier(ast::Identifier {
                                            base: b.clone(),
                                            name: "a".to_string(),
                                        }),
                                        right: ast::Expression::Identifier(ast::Identifier {
                                            base: b.clone(),
                                            name: "b".to_string(),
                                        }),
                                    })),
                                    right: ast::Expression::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "c".to_string(),
                                    }),
                                },
                            ))),
                        })),
                    })),
                    ast::Statement::Expr(Box::new(ast::ExprStmt {
                        base: b.clone(),
                        expression: ast::Expression::Call(Box::new(ast::CallExpr {
                            base: b.clone(),
                            callee: ast::Expression::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "f".to_string(),
                            }),
                            lparen: vec![],
                            arguments: vec![ast::Expression::Object(Box::new(ast::ObjectExpr {
                                base: b.clone(),
                                lbrace: vec![],
                                with: None,
                                properties: vec![ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "c".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Integer(ast::IntegerLit {
                                        base: b.clone(),
                                        value: 42,
                                    })),
                                    comma: vec![],
                                }],
                                rbrace: vec![],
                            }))],
                            rparen: vec![],
                        })),
                    })),
                ],
                eof: vec![],
            }],
        };
        let want = Package {
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
                            typ: type_info(),
                            params: vec![
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: false,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "a".to_string(),
                                    },
                                    default: Some(Expression::Integer(IntegerLit {
                                        loc: b.location.clone(),
                                        value: 0,
                                    })),
                                },
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: false,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "b".to_string(),
                                    },
                                    default: Some(Expression::Integer(IntegerLit {
                                        loc: b.location.clone(),
                                        value: 0,
                                    })),
                                },
                                FunctionParameter {
                                    loc: b.location.clone(),
                                    is_pipe: false,
                                    key: Identifier {
                                        loc: b.location.clone(),
                                        name: "c".to_string(),
                                    },
                                    default: None,
                                },
                            ],
                            body: Block::Return(ReturnStmt {
                                loc: b.location.clone(),
                                argument: Expression::Binary(Box::new(BinaryExpr {
                                    loc: b.location.clone(),
                                    typ: type_info(),
                                    operator: ast::Operator::AdditionOperator,
                                    left: Expression::Binary(Box::new(BinaryExpr {
                                        loc: b.location.clone(),
                                        typ: type_info(),
                                        operator: ast::Operator::AdditionOperator,
                                        left: Expression::Identifier(IdentifierExpr {
                                            loc: b.location.clone(),
                                            typ: type_info(),
                                            name: "a".to_string(),
                                        }),
                                        right: Expression::Identifier(IdentifierExpr {
                                            loc: b.location.clone(),
                                            typ: type_info(),
                                            name: "b".to_string(),
                                        }),
                                    })),
                                    right: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: type_info(),
                                        name: "c".to_string(),
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
                            typ: type_info(),
                            pipe: None,
                            callee: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                name: "f".to_string(),
                            }),
                            arguments: vec![Property {
                                loc: b.location.clone(),
                                key: Identifier {
                                    loc: b.location.clone(),
                                    name: "c".to_string(),
                                },
                                value: Expression::Integer(IntegerLit {
                                    loc: b.location.clone(),
                                    value: 42,
                                }),
                            }],
                        })),
                    }),
                ],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_function_multiple_pipes() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Variable(Box::new(ast::VariableAssgn {
                    base: b.clone(),
                    id: ast::Identifier {
                        base: b.clone(),
                        name: "f".to_string(),
                    },
                    init: ast::Expression::Function(Box::new(ast::FunctionExpr {
                        base: b.clone(),
                        lparen: vec![],
                        params: vec![
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "a".to_string(),
                                }),
                                separator: vec![],
                                value: None,
                                comma: vec![],
                            },
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "piped1".to_string(),
                                }),
                                separator: vec![],
                                value: Some(ast::Expression::PipeLit(ast::PipeLit {
                                    base: b.clone(),
                                })),
                                comma: vec![],
                            },
                            ast::Property {
                                base: b.clone(),
                                key: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "piped2".to_string(),
                                }),
                                separator: vec![],
                                value: Some(ast::Expression::PipeLit(ast::PipeLit {
                                    base: b.clone(),
                                })),
                                comma: vec![],
                            },
                        ],
                        rparen: vec![],
                        arrow: vec![],
                        body: ast::FunctionBody::Expr(ast::Expression::Identifier(
                            ast::Identifier {
                                base: b.clone(),
                                name: "a".to_string(),
                            },
                        )),
                    })),
                }))],
                eof: vec![],
            }],
        };
        let got = test_convert(pkg).err().unwrap().to_string();
        assert_eq!(
            "function types can have at most one pipe parameter".to_string(),
            got
        );
    }

    #[test]
    fn test_convert_call_multiple_object_arguments() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Call(Box::new(ast::CallExpr {
                        base: b.clone(),
                        callee: ast::Expression::Identifier(ast::Identifier {
                            base: b.clone(),
                            name: "f".to_string(),
                        }),
                        lparen: vec![],
                        arguments: vec![
                            ast::Expression::Object(Box::new(ast::ObjectExpr {
                                base: b.clone(),
                                lbrace: vec![],
                                with: None,
                                properties: vec![ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "a".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Integer(ast::IntegerLit {
                                        base: b.clone(),
                                        value: 0,
                                    })),
                                    comma: vec![],
                                }],
                                rbrace: vec![],
                            })),
                            ast::Expression::Object(Box::new(ast::ObjectExpr {
                                base: b.clone(),
                                lbrace: vec![],
                                with: None,
                                properties: vec![ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "b".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::Integer(ast::IntegerLit {
                                        base: b.clone(),
                                        value: 1,
                                    })),
                                    comma: vec![],
                                }],
                                rbrace: vec![],
                            })),
                        ],
                        rparen: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let got = test_convert(pkg).err().unwrap().to_string();
        assert_eq!(
            "function parameters are more than one record expression".to_string(),
            got
        );
    }

    #[test]
    fn test_convert_pipe_expression() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![
                    ast::Statement::Variable(Box::new(ast::VariableAssgn {
                        base: b.clone(),
                        id: ast::Identifier {
                            base: b.clone(),
                            name: "f".to_string(),
                        },
                        init: ast::Expression::Function(Box::new(ast::FunctionExpr {
                            base: b.clone(),
                            lparen: vec![],
                            params: vec![
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "piped".to_string(),
                                    }),
                                    separator: vec![],
                                    value: Some(ast::Expression::PipeLit(ast::PipeLit {
                                        base: b.clone(),
                                    })),
                                    comma: vec![],
                                },
                                ast::Property {
                                    base: b.clone(),
                                    key: ast::PropertyKey::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "a".to_string(),
                                    }),
                                    separator: vec![],
                                    value: None,
                                    comma: vec![],
                                },
                            ],
                            rparen: vec![],
                            arrow: vec![],
                            body: ast::FunctionBody::Expr(ast::Expression::Binary(Box::new(
                                ast::BinaryExpr {
                                    base: b.clone(),
                                    operator: ast::Operator::AdditionOperator,
                                    left: ast::Expression::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "a".to_string(),
                                    }),
                                    right: ast::Expression::Identifier(ast::Identifier {
                                        base: b.clone(),
                                        name: "piped".to_string(),
                                    }),
                                },
                            ))),
                        })),
                    })),
                    ast::Statement::Expr(Box::new(ast::ExprStmt {
                        base: b.clone(),
                        expression: ast::Expression::PipeExpr(Box::new(ast::PipeExpr {
                            base: b.clone(),
                            argument: ast::Expression::Integer(ast::IntegerLit {
                                base: b.clone(),
                                value: 3,
                            }),
                            call: ast::CallExpr {
                                base: b.clone(),
                                callee: ast::Expression::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "f".to_string(),
                                }),
                                lparen: vec![],
                                arguments: vec![ast::Expression::Object(Box::new(
                                    ast::ObjectExpr {
                                        base: b.clone(),
                                        lbrace: vec![],
                                        with: None,
                                        properties: vec![ast::Property {
                                            base: b.clone(),
                                            key: ast::PropertyKey::Identifier(ast::Identifier {
                                                base: b.clone(),
                                                name: "a".to_string(),
                                            }),
                                            separator: vec![],
                                            value: Some(ast::Expression::Integer(
                                                ast::IntegerLit {
                                                    base: b.clone(),
                                                    value: 2,
                                                },
                                            )),
                                            comma: vec![],
                                        }],
                                        rbrace: vec![],
                                    },
                                ))],
                                rparen: vec![],
                            },
                        })),
                    })),
                ],
                eof: vec![],
            }],
        };
        let want = Package {
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
                            typ: type_info(),
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
                                    typ: type_info(),
                                    operator: ast::Operator::AdditionOperator,
                                    left: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: type_info(),
                                        name: "a".to_string(),
                                    }),
                                    right: Expression::Identifier(IdentifierExpr {
                                        loc: b.location.clone(),
                                        typ: type_info(),
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
                            typ: type_info(),
                            pipe: Some(Expression::Integer(IntegerLit {
                                loc: b.location.clone(),
                                value: 3,
                            })),
                            callee: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
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
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_function_expression_simple() {
        let b = ast::BaseNode::default();
        let f = FunctionExpr {
            loc: b.location.clone(),
            typ: type_info(),
            params: vec![
                FunctionParameter {
                    loc: b.location.clone(),
                    is_pipe: false,
                    key: Identifier {
                        loc: b.location.clone(),
                        name: "a".to_string(),
                    },
                    default: None,
                },
                FunctionParameter {
                    loc: b.location.clone(),
                    is_pipe: false,
                    key: Identifier {
                        loc: b.location.clone(),
                        name: "b".to_string(),
                    },
                    default: None,
                },
            ],
            body: Block::Return(ReturnStmt {
                loc: b.location.clone(),
                argument: Expression::Binary(Box::new(BinaryExpr {
                    loc: b.location.clone(),
                    typ: type_info(),
                    operator: ast::Operator::AdditionOperator,
                    left: Expression::Identifier(IdentifierExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        name: "a".to_string(),
                    }),
                    right: Expression::Identifier(IdentifierExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        name: "b".to_string(),
                    }),
                })),
            }),
            vectorized: None,
        };
        assert_eq!(Vec::<&FunctionParameter>::new(), f.defaults());
        assert_eq!(None, f.pipe());
    }

    #[test]
    fn test_function_expression_defaults_and_pipes() {
        let b = ast::BaseNode::default();
        let piped = FunctionParameter {
            loc: b.location.clone(),
            is_pipe: true,
            key: Identifier {
                loc: b.location.clone(),
                name: "a".to_string(),
            },
            default: Some(Expression::Integer(IntegerLit {
                loc: b.location.clone(),
                value: 0,
            })),
        };
        let default1 = FunctionParameter {
            loc: b.location.clone(),
            is_pipe: false,
            key: Identifier {
                loc: b.location.clone(),
                name: "b".to_string(),
            },
            default: Some(Expression::Integer(IntegerLit {
                loc: b.location.clone(),
                value: 1,
            })),
        };
        let default2 = FunctionParameter {
            loc: b.location.clone(),
            is_pipe: false,
            key: Identifier {
                loc: b.location.clone(),
                name: "c".to_string(),
            },
            default: Some(Expression::Integer(IntegerLit {
                loc: b.location.clone(),
                value: 2,
            })),
        };
        let no_default = FunctionParameter {
            loc: b.location.clone(),
            is_pipe: false,
            key: Identifier {
                loc: b.location.clone(),
                name: "d".to_string(),
            },
            default: None,
        };
        let defaults = vec![&piped, &default1, &default2];
        let f = FunctionExpr {
            loc: b.location.clone(),
            typ: type_info(),
            params: vec![
                piped.clone(),
                default1.clone(),
                default2.clone(),
                no_default.clone(),
            ],
            body: Block::Return(ReturnStmt {
                loc: b.location.clone(),
                argument: Expression::Binary(Box::new(BinaryExpr {
                    loc: b.location.clone(),
                    typ: type_info(),
                    operator: ast::Operator::AdditionOperator,
                    left: Expression::Identifier(IdentifierExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        name: "a".to_string(),
                    }),
                    right: Expression::Identifier(IdentifierExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        name: "b".to_string(),
                    }),
                })),
            }),
            vectorized: None,
        };
        assert_eq!(defaults, f.defaults());
        assert_eq!(Some(&piped), f.pipe());
    }

    #[test]
    fn test_convert_index_expression() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Index(Box::new(ast::IndexExpr {
                        base: b.clone(),
                        array: ast::Expression::Identifier(ast::Identifier {
                            base: b.clone(),
                            name: "a".to_string(),
                        }),
                        lbrack: vec![],
                        index: ast::Expression::Integer(ast::IntegerLit {
                            base: b.clone(),
                            value: 3,
                        }),
                        rbrack: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Index(Box::new(IndexExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        array: Expression::Identifier(IdentifierExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            name: "a".to_string(),
                        }),
                        index: Expression::Integer(IntegerLit {
                            loc: b.location.clone(),
                            value: 3,
                        }),
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_nested_index_expression() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Index(Box::new(ast::IndexExpr {
                        base: b.clone(),
                        array: ast::Expression::Index(Box::new(ast::IndexExpr {
                            base: b.clone(),
                            array: ast::Expression::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "a".to_string(),
                            }),
                            lbrack: vec![],
                            index: ast::Expression::Integer(ast::IntegerLit {
                                base: b.clone(),
                                value: 3,
                            }),
                            rbrack: vec![],
                        })),
                        lbrack: vec![],
                        index: ast::Expression::Integer(ast::IntegerLit {
                            base: b.clone(),
                            value: 5,
                        }),
                        rbrack: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Index(Box::new(IndexExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        array: Expression::Index(Box::new(IndexExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            array: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                name: "a".to_string(),
                            }),
                            index: Expression::Integer(IntegerLit {
                                loc: b.location.clone(),
                                value: 3,
                            }),
                        })),
                        index: Expression::Integer(IntegerLit {
                            loc: b.location.clone(),
                            value: 5,
                        }),
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_access_idexed_object_returned_from_function_call() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Index(Box::new(ast::IndexExpr {
                        base: b.clone(),
                        array: ast::Expression::Call(Box::new(ast::CallExpr {
                            base: b.clone(),
                            callee: ast::Expression::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "f".to_string(),
                            }),
                            lparen: vec![],
                            arguments: vec![],
                            rparen: vec![],
                        })),
                        lbrack: vec![],
                        index: ast::Expression::Integer(ast::IntegerLit {
                            base: b.clone(),
                            value: 3,
                        }),
                        rbrack: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Index(Box::new(IndexExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        array: Expression::Call(Box::new(CallExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            pipe: None,
                            callee: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                name: "f".to_string(),
                            }),
                            arguments: Vec::new(),
                        })),
                        index: Expression::Integer(IntegerLit {
                            loc: b.location.clone(),
                            value: 3,
                        }),
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_nested_member_expression() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Member(Box::new(ast::MemberExpr {
                        base: b.clone(),
                        object: ast::Expression::Member(Box::new(ast::MemberExpr {
                            base: b.clone(),
                            object: ast::Expression::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "a".to_string(),
                            }),
                            lbrack: vec![],
                            property: ast::PropertyKey::Identifier(ast::Identifier {
                                base: b.clone(),
                                name: "b".to_string(),
                            }),
                            rbrack: vec![],
                        })),
                        lbrack: vec![],
                        property: ast::PropertyKey::Identifier(ast::Identifier {
                            base: b.clone(),
                            name: "c".to_string(),
                        }),
                        rbrack: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Member(Box::new(MemberExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        object: Expression::Member(Box::new(MemberExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            object: Expression::Identifier(IdentifierExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                name: "a".to_string(),
                            }),
                            property: "b".to_string(),
                        })),
                        property: "c".to_string(),
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_member_with_call_expression() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Member(Box::new(ast::MemberExpr {
                        base: b.clone(),
                        object: ast::Expression::Call(Box::new(ast::CallExpr {
                            base: b.clone(),
                            callee: ast::Expression::Member(Box::new(ast::MemberExpr {
                                base: b.clone(),
                                object: ast::Expression::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "a".to_string(),
                                }),
                                lbrack: vec![],
                                property: ast::PropertyKey::Identifier(ast::Identifier {
                                    base: b.clone(),
                                    name: "b".to_string(),
                                }),
                                rbrack: vec![],
                            })),
                            lparen: vec![],
                            arguments: vec![],
                            rparen: vec![],
                        })),
                        lbrack: vec![],
                        property: ast::PropertyKey::Identifier(ast::Identifier {
                            base: b.clone(),
                            name: "c".to_string(),
                        }),
                        rbrack: vec![],
                    })),
                }))],
                eof: vec![],
            }],
        };
        let want = Package {
            loc: b.location.clone(),
            package: "main".to_string(),
            files: vec![File {
                loc: b.location.clone(),
                package: None,
                imports: Vec::new(),
                body: vec![Statement::Expr(ExprStmt {
                    loc: b.location.clone(),
                    expression: Expression::Member(Box::new(MemberExpr {
                        loc: b.location.clone(),
                        typ: type_info(),
                        object: Expression::Call(Box::new(CallExpr {
                            loc: b.location.clone(),
                            typ: type_info(),
                            pipe: None,
                            callee: Expression::Member(Box::new(MemberExpr {
                                loc: b.location.clone(),
                                typ: type_info(),
                                object: Expression::Identifier(IdentifierExpr {
                                    loc: b.location.clone(),
                                    typ: type_info(),
                                    name: "a".to_string(),
                                }),
                                property: "b".to_string(),
                            })),
                            arguments: Vec::new(),
                        })),
                        property: "c".to_string(),
                    })),
                })],
            }],
        };
        let got = test_convert(pkg).unwrap();
        assert_eq!(want, got);
    }
    #[test]
    fn test_convert_bad_stmt() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Bad(Box::new(ast::BadStmt {
                    base: b.clone(),
                    text: "bad statement".to_string(),
                }))],
                eof: vec![],
            }],
        };
        test_convert(pkg).unwrap();
    }
    #[test]
    fn test_convert_bad_expr() {
        let b = ast::BaseNode::default();
        let pkg = ast::Package {
            base: b.clone(),
            path: "path".to_string(),
            package: "main".to_string(),
            files: vec![ast::File {
                base: b.clone(),
                name: "foo.flux".to_string(),
                metadata: String::new(),
                package: None,
                imports: Vec::new(),
                body: vec![ast::Statement::Expr(Box::new(ast::ExprStmt {
                    base: b.clone(),
                    expression: ast::Expression::Bad(Box::new(ast::BadExpr {
                        base: b.clone(),
                        text: "bad expression".to_string(),
                        expression: None,
                    })),
                }))],
                eof: vec![],
            }],
        };
        test_convert(pkg).unwrap();
    }

    #[test]
    fn test_convert_monotype_int() {
        let b = ast::BaseNode::default();
        let monotype = ast::MonoType::Basic(ast::NamedType {
            base: b.clone(),
            name: ast::Identifier {
                base: b.clone(),
                name: "int".to_string(),
            },
        });
        let mut m = BTreeMap::<String, types::Tvar>::new();
        let got = convert_monotype(monotype, &mut m, &mut sub::Substitution::default()).unwrap();
        let want = MonoType::Int;
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_monotype_record() {
        let b = ast::BaseNode::default();
        let monotype = ast::MonoType::Record(ast::RecordType {
            base: b.clone(),
            tvar: Some(ast::Identifier {
                base: b.clone(),
                name: "A".to_string(),
            }),
            properties: vec![ast::PropertyType {
                base: b.clone(),
                name: ast::Identifier {
                    base: b.clone(),
                    name: "B".to_string(),
                },
                monotype: ast::MonoType::Basic(ast::NamedType {
                    base: b.clone(),
                    name: ast::Identifier {
                        base: b.clone(),
                        name: "int".to_string(),
                    },
                }),
            }],
        });
        let mut m = BTreeMap::<String, types::Tvar>::new();
        let got = convert_monotype(monotype, &mut m, &mut sub::Substitution::default()).unwrap();
        let want = MonoType::from(types::Record::Extension {
            head: types::Property {
                k: "B".to_string(),
                v: MonoType::Int,
            },
            tail: MonoType::Var(Tvar(0)),
        });
        assert_eq!(want, got);
    }
    #[test]
    fn test_convert_monotype_function() {
        let b = ast::BaseNode::default();
        let monotype_ex = ast::MonoType::Function(Box::new(ast::FunctionType {
            base: b.clone(),
            parameters: vec![ast::ParameterType::Optional {
                base: b.clone(),
                name: ast::Identifier {
                    base: b.clone(),
                    name: "A".to_string(),
                },
                monotype: ast::MonoType::Basic(ast::NamedType {
                    base: b.clone(),
                    name: ast::Identifier {
                        base: b.clone(),
                        name: "int".to_string(),
                    },
                }),
            }],
            monotype: ast::MonoType::Basic(ast::NamedType {
                base: b.clone(),
                name: ast::Identifier {
                    base: b.clone(),
                    name: "int".to_string(),
                },
            }),
        }));
        let mut m = BTreeMap::<String, types::Tvar>::new();
        let got = convert_monotype(monotype_ex, &mut m, &mut sub::Substitution::default()).unwrap();
        let mut opt = MonoTypeMap::new();
        opt.insert(String::from("A"), MonoType::Int);
        let want = MonoType::from(types::Function {
            req: MonoTypeMap::new(),
            opt,
            pipe: None,
            retn: MonoType::Int,
        });
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_polytype() {
        // (A: T, B: S) => T where T: Addable, S: Divisible
        let b = ast::BaseNode::default();
        let type_exp = ast::TypeExpression {
            base: b.clone(),
            monotype: ast::MonoType::Function(Box::new(ast::FunctionType {
                base: b.clone(),
                parameters: vec![
                    ast::ParameterType::Required {
                        base: b.clone(),
                        name: ast::Identifier {
                            base: b.clone(),
                            name: "A".to_string(),
                        },
                        monotype: ast::MonoType::Tvar(ast::TvarType {
                            base: b.clone(),
                            name: ast::Identifier {
                                base: b.clone(),
                                name: "T".to_string(),
                            },
                        }),
                    },
                    ast::ParameterType::Required {
                        base: b.clone(),
                        name: ast::Identifier {
                            base: b.clone(),
                            name: "B".to_string(),
                        },
                        monotype: ast::MonoType::Tvar(ast::TvarType {
                            base: b.clone(),
                            name: ast::Identifier {
                                base: b.clone(),
                                name: "S".to_string(),
                            },
                        }),
                    },
                ],
                monotype: ast::MonoType::Tvar(ast::TvarType {
                    base: b.clone(),
                    name: ast::Identifier {
                        base: b.clone(),
                        name: "T".to_string(),
                    },
                }),
            })),
            constraints: vec![
                ast::TypeConstraint {
                    base: b.clone(),
                    tvar: ast::Identifier {
                        base: b.clone(),
                        name: "T".to_string(),
                    },
                    kinds: vec![ast::Identifier {
                        base: b.clone(),
                        name: "Addable".to_string(),
                    }],
                },
                ast::TypeConstraint {
                    base: b.clone(),
                    tvar: ast::Identifier {
                        base: b.clone(),
                        name: "S".to_string(),
                    },
                    kinds: vec![ast::Identifier {
                        base: b.clone(),
                        name: "Divisible".to_string(),
                    }],
                },
            ],
        };
        let got = convert_polytype(type_exp, &mut sub::Substitution::default()).unwrap();
        let mut vars = Vec::<types::Tvar>::new();
        vars.push(types::Tvar(0));
        vars.push(types::Tvar(1));
        let mut cons = types::TvarKinds::new();
        let mut kind_vector_1 = Vec::<types::Kind>::new();
        kind_vector_1.push(types::Kind::Addable);
        cons.insert(types::Tvar(0), kind_vector_1);

        let mut kind_vector_2 = Vec::<types::Kind>::new();
        kind_vector_2.push(types::Kind::Divisible);
        cons.insert(types::Tvar(1), kind_vector_2);

        let mut req = MonoTypeMap::new();
        req.insert("A".to_string(), MonoType::Var(Tvar(0)));
        req.insert("B".to_string(), MonoType::Var(Tvar(1)));
        let expr = MonoType::from(types::Function {
            req,
            opt: MonoTypeMap::new(),
            pipe: None,
            retn: MonoType::Var(Tvar(0)),
        });
        let want = types::PolyType { vars, cons, expr };
        assert_eq!(want, got);
    }

    #[test]
    fn test_convert_polytype_2() {
        // (A: T, B: S) => T where T: Addable
        let b = ast::BaseNode::default();
        let type_exp = ast::TypeExpression {
            base: b.clone(),
            monotype: ast::MonoType::Function(Box::new(ast::FunctionType {
                base: b.clone(),
                parameters: vec![
                    ast::ParameterType::Required {
                        base: b.clone(),
                        name: ast::Identifier {
                            base: b.clone(),
                            name: "A".to_string(),
                        },
                        monotype: ast::MonoType::Tvar(ast::TvarType {
                            base: b.clone(),
                            name: ast::Identifier {
                                base: b.clone(),
                                name: "T".to_string(),
                            },
                        }),
                    },
                    ast::ParameterType::Required {
                        base: b.clone(),
                        name: ast::Identifier {
                            base: b.clone(),
                            name: "B".to_string(),
                        },
                        monotype: ast::MonoType::Tvar(ast::TvarType {
                            base: b.clone(),
                            name: ast::Identifier {
                                base: b.clone(),
                                name: "S".to_string(),
                            },
                        }),
                    },
                ],
                monotype: ast::MonoType::Tvar(ast::TvarType {
                    base: b.clone(),
                    name: ast::Identifier {
                        base: b.clone(),
                        name: "T".to_string(),
                    },
                }),
            })),
            constraints: vec![ast::TypeConstraint {
                base: b.clone(),
                tvar: ast::Identifier {
                    base: b.clone(),
                    name: "T".to_string(),
                },
                kinds: vec![ast::Identifier {
                    base: b.clone(),
                    name: "Addable".to_string(),
                }],
            }],
        };
        let got = convert_polytype(type_exp, &mut sub::Substitution::default()).unwrap();
        let mut vars = Vec::<types::Tvar>::new();
        vars.push(types::Tvar(0));
        vars.push(types::Tvar(1));
        let mut cons = types::TvarKinds::new();
        let mut kind_vector_1 = Vec::<types::Kind>::new();
        kind_vector_1.push(types::Kind::Addable);
        cons.insert(types::Tvar(0), kind_vector_1);

        let mut req = MonoTypeMap::new();
        req.insert("A".to_string(), MonoType::Var(Tvar(0)));
        req.insert("B".to_string(), MonoType::Var(Tvar(1)));
        let expr = MonoType::from(types::Function {
            req,
            opt: MonoTypeMap::new(),
            pipe: None,
            retn: MonoType::Var(Tvar(0)),
        });
        let want = types::PolyType { vars, cons, expr };
        assert_eq!(want, got);
    }
}
