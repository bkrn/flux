use std::ops;

use derive_more::Display;

use crate::{
    ast::SourceLocation,
    semantic::{
        env::Environment,
        sub::{Substitutable, Substituter, Substitution},
        types::{self, minus, Kind, MonoType, PolyType, SubstitutionMap, Tvar, TvarKinds},
    },
};

// Type constraints are produced during type inference and come
// in two flavors.
//
// A kind constraint asserts that a particular type is of a
// particular kind or family of types.
//
// An equality contraint asserts that two types are equivalent
// and will be unified at some point.
//
// A constraint is composed of an expected type, the actual type
// that was found, and the source location of the actual type.
//
#[derive(Debug, PartialEq)]
pub enum Constraint {
    Kind {
        exp: Kind,
        act: MonoType,
        loc: SourceLocation,
    },
    Equal {
        exp: MonoType,
        act: MonoType,
        loc: SourceLocation,
    },
}

#[derive(Debug, PartialEq)]
pub struct Constraints(Vec<Constraint>);

impl Constraints {
    pub fn empty() -> Constraints {
        Constraints(Vec::new())
    }

    pub fn add(&mut self, cons: Constraint) {
        self.0.push(cons);
    }
}

// Constraints can be added using the '+' operator
impl ops::Add for Constraints {
    type Output = Constraints;

    fn add(mut self, mut cons: Constraints) -> Self::Output {
        self.0.append(&mut cons.0);
        self
    }
}

impl From<Vec<Constraint>> for Constraints {
    fn from(constraints: Vec<Constraint>) -> Constraints {
        Constraints(constraints)
    }
}

impl From<Constraints> for Vec<Constraint> {
    fn from(constraints: Constraints) -> Vec<Constraint> {
        constraints.0
    }
}

impl From<Constraint> for Constraints {
    fn from(constraint: Constraint) -> Constraints {
        Constraints::from(vec![constraint])
    }
}

#[derive(Debug, Display, PartialEq)]
#[display(fmt = "type error {}: {}", loc, err)]
pub struct Error {
    pub loc: SourceLocation,
    pub err: types::Error,
}

impl std::error::Error for Error {}

impl Substitutable for Error {
    fn apply_ref(&self, sub: &dyn Substituter) -> Option<Self> {
        self.err.apply_ref(sub).map(|err| Error {
            loc: self.loc.clone(),
            err,
        })
    }
    fn free_vars(&self) -> Vec<Tvar> {
        self.err.free_vars()
    }
}

// Solve a set of type constraints
pub fn solve(cons: &Constraints, sub: &mut Substitution) -> Result<(), Error> {
    for constraint in &cons.0 {
        match constraint {
            Constraint::Kind { exp, act, loc } => {
                // Apply the current substitution to the type, then constrain
                log::debug!("Constraint::Kind {:?}: {} => {}", loc.source, exp, act);
                act.clone()
                    .apply(sub)
                    .constrain(*exp, sub.cons())
                    .map_err(|err| Error {
                        loc: loc.clone(),
                        err,
                    })?;
            }
            Constraint::Equal { exp, act, loc } => {
                // Apply the current substitution to the constraint, then unify
                log::debug!("Constraint::Equal {:?}: {} <===> {}", loc.source, exp, act);
                exp.unify(act, sub).map_err(|err| Error {
                    loc: loc.clone(),
                    err,
                })?;
            }
        }
    }
    Ok(())
}

// Create a parametric type from a monotype by universally quantifying
// all of its free type variables.
//
// A type variable is free in a monotype if it is **free** in the global
// type environment. Equivalently a type variable is free and can be
// quantified if has not already been quantified another type in the
// type environment.
//
pub fn generalize(env: &Environment, with: &TvarKinds, t: MonoType) -> PolyType {
    let vars = minus(&env.free_vars(), t.free_vars());
    let mut cons = TvarKinds::new();
    for tv in &vars {
        if let Some(kinds) = with.get(tv) {
            cons.insert(*tv, kinds.to_owned());
        }
    }
    PolyType {
        vars,
        cons,
        expr: t,
    }
}

// Instantiate a new monotype from a polytype by assigning all universally
// quantified type variables new fresh variables that do not exist in the
// current type environment.
//
// Instantiation is what allows for polymorphic function specialization
// based on the context in which a function is called.
pub fn instantiate(
    poly: PolyType,
    sub: &mut Substitution,
    loc: SourceLocation,
) -> (MonoType, Constraints) {
    // Substitute fresh type variables for all quantified variables
    let sub: SubstitutionMap = poly
        .vars
        .into_iter()
        .map(|tv| (tv, MonoType::Var(sub.fresh())))
        .collect();
    // Generate constraints for the new fresh type variables
    let constraints = poly
        .cons
        .into_iter()
        .fold(Constraints::empty(), |cons, (tv, kinds)| {
            cons + kinds
                .into_iter()
                .map(|kind| Constraint::Kind {
                    exp: kind,
                    act: sub.get(&tv).unwrap().clone(),
                    loc: loc.clone(),
                })
                .collect::<Vec<Constraint>>()
                .into()
        });
    // Instantiate monotype using new fresh type variables
    (poly.expr.apply(&sub), constraints)
}
