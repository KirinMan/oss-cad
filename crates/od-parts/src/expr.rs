//! A tiny expression evaluator for parametric part definitions.
//!
//! Parts are parametric rather than enumerated, and that is the whole strategy
//! for the catalogue: a vendor ships tens of thousands of fixed-size parts,
//! while one definition of a rectangular elbow whose dimensions are expressions
//! in `W`, `H` and `R` covers every size anyone will ever draw. The library can
//! therefore be useful on day one without a single manufacturer agreement
//! (`docs/04-mep.md`, R-1).
//!
//! The grammar is deliberately small — arithmetic, parentheses, a handful of
//! functions, and named parameters. It is not a scripting language: a part
//! definition that needs branching or state belongs in a plugin, where it can be
//! sandboxed.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExprError {
    #[error("unexpected `{0}` in expression")]
    Unexpected(String),
    #[error("expression ended early")]
    UnexpectedEnd,
    #[error("unknown parameter `{0}`")]
    UnknownParam(String),
    #[error("unknown function `{0}`")]
    UnknownFunction(String),
    #[error("`{0}` takes {1} argument(s)")]
    WrongArity(String, usize),
    #[error("division by zero")]
    DivideByZero,
}

pub type Result<T> = std::result::Result<T, ExprError>;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(f64),
    Param(String),
    Neg(Box<Expr>),
    Binary {
        op: Op,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Expr {
    /// Parses an expression. A bare number is the common case and stays cheap.
    pub fn parse(src: &str) -> Result<Self> {
        let tokens = tokenize(src)?;
        let mut p = Parser { tokens, at: 0 };
        let e = p.expr()?;
        if p.at < p.tokens.len() {
            return Err(ExprError::Unexpected(format!("{:?}", p.tokens[p.at])));
        }
        Ok(e)
    }

    pub fn eval(&self, params: &HashMap<String, f64>) -> Result<f64> {
        match self {
            Expr::Number(v) => Ok(*v),
            Expr::Param(name) => params
                .get(name)
                .copied()
                .ok_or_else(|| ExprError::UnknownParam(name.clone())),
            Expr::Neg(e) => Ok(-e.eval(params)?),
            Expr::Binary { op, lhs, rhs } => {
                let a = lhs.eval(params)?;
                let b = rhs.eval(params)?;
                match op {
                    Op::Add => Ok(a + b),
                    Op::Sub => Ok(a - b),
                    Op::Mul => Ok(a * b),
                    Op::Div => {
                        if b == 0.0 {
                            Err(ExprError::DivideByZero)
                        } else {
                            Ok(a / b)
                        }
                    }
                }
            }
            Expr::Call { name, args } => {
                let v: Result<Vec<f64>> = args.iter().map(|a| a.eval(params)).collect();
                let v = v?;
                let arity = |n: usize| -> Result<()> {
                    if v.len() == n {
                        Ok(())
                    } else {
                        Err(ExprError::WrongArity(name.clone(), n))
                    }
                };
                match name.as_str() {
                    "min" => {
                        arity(2)?;
                        Ok(v[0].min(v[1]))
                    }
                    "max" => {
                        arity(2)?;
                        Ok(v[0].max(v[1]))
                    }
                    "abs" => {
                        arity(1)?;
                        Ok(v[0].abs())
                    }
                    "sqrt" => {
                        arity(1)?;
                        Ok(v[0].max(0.0).sqrt())
                    }
                    "round" => {
                        arity(1)?;
                        Ok(v[0].round())
                    }
                    // Rounds up to the next multiple — how duct and pipe sizes
                    // are snapped to stocked increments.
                    "ceil_to" => {
                        arity(2)?;
                        if v[1] == 0.0 {
                            Err(ExprError::DivideByZero)
                        } else {
                            Ok((v[0] / v[1]).ceil() * v[1])
                        }
                    }
                    other => Err(ExprError::UnknownFunction(other.to_owned())),
                }
            }
        }
    }

    /// Every parameter this expression reads, for validating a part definition
    /// before it is ever instantiated.
    pub fn params(&self, out: &mut Vec<String>) {
        match self {
            Expr::Number(_) => {}
            Expr::Param(n) => {
                if !out.contains(n) {
                    out.push(n.clone());
                }
            }
            Expr::Neg(e) => e.params(out),
            Expr::Binary { lhs, rhs, .. } => {
                lhs.params(out);
                rhs.params(out);
            }
            Expr::Call { args, .. } => {
                for a in args {
                    a.params(out);
                }
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Number(v) => write!(f, "{v}"),
            Expr::Param(n) => f.write_str(n),
            Expr::Neg(e) => write!(f, "-({e})"),
            Expr::Binary { op, lhs, rhs } => {
                let o = match op {
                    Op::Add => '+',
                    Op::Sub => '-',
                    Op::Mul => '*',
                    Op::Div => '/',
                };
                write!(f, "({lhs} {o} {rhs})")
            }
            Expr::Call { name, args } => {
                let list: Vec<String> = args.iter().map(ToString::to_string).collect();
                write!(f, "{name}({})", list.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
}

fn tokenize(src: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let v = text
                    .parse::<f64>()
                    .map_err(|_| ExprError::Unexpected(text.clone()))?;
                out.push(Token::Num(v));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(ExprError::Unexpected(other.to_string())),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn expr(&mut self) -> Result<Expr> {
        let mut lhs = self.term()?;
        while let Some(op) = match self.peek() {
            Some(Token::Plus) => Some(Op::Add),
            Some(Token::Minus) => Some(Op::Sub),
            _ => None,
        } {
            self.at += 1;
            let rhs = self.term()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Expr> {
        let mut lhs = self.unary()?;
        while let Some(op) = match self.peek() {
            Some(Token::Star) => Some(Op::Mul),
            Some(Token::Slash) => Some(Op::Div),
            _ => None,
        } {
            self.at += 1;
            let rhs = self.unary()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        if matches!(self.peek(), Some(Token::Minus)) {
            self.at += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Expr> {
        match self.peek().cloned() {
            Some(Token::Num(v)) => {
                self.at += 1;
                Ok(Expr::Number(v))
            }
            Some(Token::Ident(name)) => {
                self.at += 1;
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.at += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Token::RParen)) {
                        loop {
                            args.push(self.expr()?);
                            if matches!(self.peek(), Some(Token::Comma)) {
                                self.at += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    if !matches!(self.peek(), Some(Token::RParen)) {
                        return Err(ExprError::UnexpectedEnd);
                    }
                    self.at += 1;
                    Ok(Expr::Call { name, args })
                } else {
                    Ok(Expr::Param(name))
                }
            }
            Some(Token::LParen) => {
                self.at += 1;
                let e = self.expr()?;
                if !matches!(self.peek(), Some(Token::RParen)) {
                    return Err(ExprError::UnexpectedEnd);
                }
                self.at += 1;
                Ok(e)
            }
            Some(other) => Err(ExprError::Unexpected(format!("{other:?}"))),
            None => Err(ExprError::UnexpectedEnd),
        }
    }
}

/// A value in a part definition: either a fixed number or an expression.
///
/// Stored as an untagged enum so JSON can write `400` or `"W * 1.5"` in the
/// same field, which is what makes part files readable by the engineers who
/// maintain them.
#[derive(Debug, Clone, PartialEq)]
pub struct Value(pub Expr);

impl Value {
    pub fn eval(&self, params: &HashMap<String, f64>) -> Result<f64> {
        self.0.eval(params)
    }

    #[must_use]
    pub fn constant(v: f64) -> Self {
        Self(Expr::Number(v))
    }
}

impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match &self.0 {
            Expr::Number(v) => s.serialize_f64(*v),
            other => s.serialize_str(&other.to_string()),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Value {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(f64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(v) => Ok(Value(Expr::Number(v))),
            Raw::Text(t) => Expr::parse(&t).map(Value).map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    }

    fn eval(src: &str, p: &[(&str, f64)]) -> Result<f64> {
        Expr::parse(src)?.eval(&params(p))
    }

    #[test]
    fn arithmetic_respects_precedence() {
        assert_eq!(eval("2 + 3 * 4", &[]), Ok(14.0));
        assert_eq!(eval("(2 + 3) * 4", &[]), Ok(20.0));
        assert_eq!(eval("10 / 4", &[]), Ok(2.5));
        assert_eq!(eval("-5 + 2", &[]), Ok(-3.0));
        assert_eq!(eval("2 * -3", &[]), Ok(-6.0));
    }

    #[test]
    fn parameters_come_from_the_instance() {
        assert_eq!(eval("W / 2", &[("W", 400.0)]), Ok(200.0));
        assert_eq!(eval("W + H", &[("W", 400.0), ("H", 300.0)]), Ok(700.0));
        assert_eq!(
            eval("W", &[]),
            Err(ExprError::UnknownParam("W".into())),
            "a missing parameter must be an error, not zero"
        );
    }

    #[test]
    fn duct_sizing_helpers_work() {
        // A 430 mm requirement snaps up to the next 50 mm increment.
        assert_eq!(eval("ceil_to(430, 50)", &[]), Ok(450.0));
        assert_eq!(eval("ceil_to(450, 50)", &[]), Ok(450.0));
        // Elbow radius: one duct width, but never below 150.
        assert_eq!(eval("max(W * 1.0, 150)", &[("W", 100.0)]), Ok(150.0));
        assert_eq!(eval("max(W * 1.0, 150)", &[("W", 400.0)]), Ok(400.0));
    }

    #[test]
    fn division_by_zero_is_refused_rather_than_producing_infinity() {
        assert_eq!(eval("1 / 0", &[]), Err(ExprError::DivideByZero));
        assert_eq!(eval("ceil_to(1, 0)", &[]), Err(ExprError::DivideByZero));
    }

    #[test]
    fn unknown_functions_are_rejected() {
        assert_eq!(
            eval("frobnicate(1)", &[]),
            Err(ExprError::UnknownFunction("frobnicate".into()))
        );
        assert_eq!(
            eval("min(1)", &[]),
            Err(ExprError::WrongArity("min".into(), 2))
        );
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(Expr::parse("2 +").is_err());
        assert!(Expr::parse("(2 + 3").is_err());
        assert!(Expr::parse("2 3").is_err());
        assert!(Expr::parse("$").is_err());
        assert!(Expr::parse("").is_err());
    }

    #[test]
    fn required_parameters_can_be_discovered_before_use() {
        let e = Expr::parse("max(W, H) + R / 2").expect("parses");
        let mut found = Vec::new();
        e.params(&mut found);
        found.sort();
        assert_eq!(found, vec!["H", "R", "W"]);
    }

    #[test]
    fn values_round_trip_through_json_in_both_forms() {
        let num: Value = serde_json::from_str("400").expect("number form");
        assert_eq!(num.eval(&params(&[])), Ok(400.0));
        assert_eq!(serde_json::to_string(&num).expect("writes"), "400.0");

        let expr: Value = serde_json::from_str("\"W * 1.5\"").expect("expression form");
        assert_eq!(expr.eval(&params(&[("W", 200.0)])), Ok(300.0));
        let json = serde_json::to_string(&expr).expect("writes");
        let back: Value = serde_json::from_str(&json).expect("re-reads");
        assert_eq!(back.eval(&params(&[("W", 200.0)])), Ok(300.0));
    }

    #[test]
    fn a_bad_expression_in_json_fails_at_load_not_at_use() {
        let r: std::result::Result<Value, _> = serde_json::from_str("\"W * \"");
        assert!(r.is_err(), "the part file must be rejected when it is read");
    }
}
