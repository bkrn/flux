use fluxcore::ast;
use fluxcore::semantic::env::Environment;
use fluxcore::semantic::nodes::*;
use fluxcore::semantic::types::{Function, MonoType, PolyTypeMap, SemanticMap, Tvar};
use fluxcore::semantic::walk::{walk_mut, NodeMut};
use fluxcore::semantic::Analyzer;

use pretty_assertions::assert_eq;

#[test]
fn analyze_end_to_end() {
    let mut analyzer = Analyzer::new_with_defaults(Environment::default(), PolyTypeMap::new());
    let (_, mut got) = analyzer
        .analyze_source(
            "main".to_string(),
            "main.flux".to_string(),
            r#"
n = 1
s = "string"
f = (a) => a + a
f(a: n)
f(a: s)
        "#,
        )
        .unwrap();
    let f_type = Function {
        req: fluxcore::semantic_map! {
            "a".to_string() => MonoType::Var(Tvar(4)),
        },
        opt: SemanticMap::new(),
        pipe: None,
        retn: MonoType::Var(Tvar(4)),
    };
    let f_call_int_type = Function {
        req: fluxcore::semantic_map! {
            "a".to_string() => MonoType::Int,
        },
        opt: SemanticMap::new(),
        pipe: None,
        retn: MonoType::Int,
    };
    let f_call_string_type = Function {
        req: fluxcore::semantic_map! {
            "a".to_string() => MonoType::String,
        },
        opt: SemanticMap::new(),
        pipe: None,
        retn: MonoType::String,
    };
    let want = Package {
        loc: ast::BaseNode::default().location,
        package: "main".to_string(),
        files: vec![File {
            loc: ast::BaseNode::default().location,
            package: None,
            imports: Vec::new(),
            body: vec![
                Statement::Variable(Box::new(VariableAssgn::new(
                    Identifier {
                        loc: ast::BaseNode::default().location,
                        name: "n".to_string(),
                    },
                    Expression::Integer(IntegerLit {
                        loc: ast::BaseNode::default().location,
                        value: 1,
                    }),
                    ast::BaseNode::default().location,
                ))),
                Statement::Variable(Box::new(VariableAssgn::new(
                    Identifier {
                        loc: ast::BaseNode::default().location,
                        name: "s".to_string(),
                    },
                    Expression::StringLit(StringLit {
                        loc: ast::BaseNode::default().location,
                        value: "string".to_string(),
                    }),
                    ast::BaseNode::default().location,
                ))),
                Statement::Variable(Box::new(VariableAssgn::new(
                    Identifier {
                        loc: ast::BaseNode::default().location,
                        name: "f".to_string(),
                    },
                    Expression::Function(Box::new(FunctionExpr {
                        loc: ast::BaseNode::default().location,
                        typ: MonoType::from(f_type),
                        params: vec![FunctionParameter {
                            loc: ast::BaseNode::default().location,
                            is_pipe: false,
                            key: Identifier {
                                loc: ast::BaseNode::default().location,
                                name: "a".to_string(),
                            },
                            default: None,
                        }],
                        body: Block::Return(ReturnStmt {
                            loc: ast::BaseNode::default().location,
                            argument: Expression::Binary(Box::new(BinaryExpr {
                                loc: ast::BaseNode::default().location,
                                typ: MonoType::Var(Tvar(4)),
                                operator: ast::Operator::AdditionOperator,
                                left: Expression::Identifier(IdentifierExpr {
                                    loc: ast::BaseNode::default().location,
                                    typ: MonoType::Var(Tvar(4)),
                                    name: "a".to_string(),
                                }),
                                right: Expression::Identifier(IdentifierExpr {
                                    loc: ast::BaseNode::default().location,
                                    typ: MonoType::Var(Tvar(4)),
                                    name: "a".to_string(),
                                }),
                            })),
                        }),
                        vectorized: None,
                    })),
                    ast::BaseNode::default().location,
                ))),
                Statement::Expr(ExprStmt {
                    loc: ast::BaseNode::default().location,
                    expression: Expression::Call(Box::new(CallExpr {
                        loc: ast::BaseNode::default().location,
                        typ: MonoType::Int,
                        pipe: None,
                        callee: Expression::Identifier(IdentifierExpr {
                            loc: ast::BaseNode::default().location,
                            typ: MonoType::from(f_call_int_type),
                            name: "f".to_string(),
                        }),
                        arguments: vec![Property {
                            loc: ast::BaseNode::default().location,
                            key: Identifier {
                                loc: ast::BaseNode::default().location,
                                name: "a".to_string(),
                            },
                            value: Expression::Identifier(IdentifierExpr {
                                loc: ast::BaseNode::default().location,
                                typ: MonoType::Int,
                                name: "n".to_string(),
                            }),
                        }],
                    })),
                }),
                Statement::Expr(ExprStmt {
                    loc: ast::BaseNode::default().location,
                    expression: Expression::Call(Box::new(CallExpr {
                        loc: ast::BaseNode::default().location,
                        typ: MonoType::String,
                        pipe: None,
                        callee: Expression::Identifier(IdentifierExpr {
                            loc: ast::BaseNode::default().location,
                            typ: MonoType::from(f_call_string_type),
                            name: "f".to_string(),
                        }),
                        arguments: vec![Property {
                            loc: ast::BaseNode::default().location,
                            key: Identifier {
                                loc: ast::BaseNode::default().location,
                                name: "a".to_string(),
                            },
                            value: Expression::Identifier(IdentifierExpr {
                                loc: ast::BaseNode::default().location,
                                typ: MonoType::String,
                                name: "s".to_string(),
                            }),
                        }],
                    })),
                }),
            ],
        }],
    };
    // We don't want to test the locations, so we override those with the base one.
    walk_mut(
        &mut |n: &mut NodeMut| n.set_loc(ast::BaseNode::default().location),
        &mut NodeMut::Package(&mut got),
    );
    assert_eq!(want, got);
}
