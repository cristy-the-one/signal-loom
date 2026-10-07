//! Derived channels. The grammar is identifiers, numbers, + - * /, parentheses,
//! `abs(x)`, and a one-pole `lp(x, alpha)`.

use crate::error::{Error, Result};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Compiled {
    root: Node,
}

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    Var(String),
    Neg(Box<Node>),
    Abs(Box<Node>),
    Lp(Box<Node>, f64),
    Bin(BinOp, Box<Node>, Box<Node>),
}

#[derive(Debug, Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

pub fn compile(expr: &str) -> Result<Compiled> {
    let mut parser = Parser {
        bytes: expr.as_bytes(),
        pos: 0,
    };
    let root = parser.parse_expr()?;
    parser.skip();
    if parser.pos != parser.bytes.len() {
        return Err(Error::msg(format!("could not parse the rest of '{expr}'")));
    }
    Ok(Compiled { root })
}

impl Compiled {
    pub fn dependencies(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        self.root.deps(&mut names, &mut seen);
        names
    }

    /// Evaluate one sample. `lp_state` keeps one-pole memory in walk order.
    /// A non-finite result, such as a division by zero, is `None`: that sample
    /// is a gap in the channel, not an error for the whole query.
    pub fn eval(
        &self,
        vars: &HashMap<String, f64>,
        lp_state: &mut Vec<f64>,
    ) -> Result<Option<f64>> {
        let mut slot = 0usize;
        let value = self.root.eval(vars, lp_state, &mut slot)?;
        Ok(value.is_finite().then_some(value))
    }
}

impl Node {
    fn deps(&self, names: &mut Vec<String>, seen: &mut HashSet<String>) {
        match self {
            Node::Num(_) => {}
            Node::Var(name) => {
                if seen.insert(name.clone()) {
                    names.push(name.clone());
                }
            }
            Node::Neg(inner) | Node::Abs(inner) | Node::Lp(inner, _) => inner.deps(names, seen),
            Node::Bin(_, left, right) => {
                left.deps(names, seen);
                right.deps(names, seen);
            }
        }
    }

    fn eval(
        &self,
        vars: &HashMap<String, f64>,
        lp_state: &mut Vec<f64>,
        slot: &mut usize,
    ) -> Result<f64> {
        match self {
            Node::Num(value) => Ok(*value),
            Node::Var(name) => vars
                .get(name)
                .copied()
                .ok_or_else(|| Error::msg(format!("math channel has no value for {name} yet"))),
            Node::Neg(inner) => Ok(-inner.eval(vars, lp_state, slot)?),
            Node::Abs(inner) => Ok(inner.eval(vars, lp_state, slot)?.abs()),
            Node::Lp(inner, alpha) => {
                let sample = inner.eval(vars, lp_state, slot)?;
                let index = *slot;
                *slot += 1;
                if !sample.is_finite() {
                    // Keep the filter's memory clean; this sample is a gap.
                    return Ok(sample);
                }
                if lp_state.len() <= index {
                    lp_state.resize(index + 1, sample);
                    return Ok(sample);
                }
                let next = lp_state[index] + alpha * (sample - lp_state[index]);
                lp_state[index] = next;
                Ok(next)
            }
            Node::Bin(op, left, right) => {
                let a = left.eval(vars, lp_state, slot)?;
                let b = right.eval(vars, lp_state, slot)?;
                Ok(match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    BinOp::Div => a / b,
                })
            }
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn parse_expr(&mut self) -> Result<Node> {
        let mut node = self.parse_term()?;
        loop {
            self.skip();
            let op = match self.peek() {
                Some(b'+') => BinOp::Add,
                Some(b'-') => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_term()?;
            node = Node::Bin(op, Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_term(&mut self) -> Result<Node> {
        let mut node = self.parse_unary()?;
        loop {
            self.skip();
            let op = match self.peek() {
                Some(b'*') => BinOp::Mul,
                Some(b'/') => BinOp::Div,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            node = Node::Bin(op, Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> Result<Node> {
        self.skip();
        if self.peek() == Some(b'-') {
            self.pos += 1;
            return Ok(Node::Neg(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Node> {
        self.skip();
        if self.peek() == Some(b'(') {
            self.pos += 1;
            let node = self.parse_expr()?;
            self.skip();
            if self.peek() != Some(b')') {
                return Err(Error::msg("math expression is missing ')'"));
            }
            self.pos += 1;
            return Ok(node);
        }
        if self
            .peek()
            .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            return self.parse_number();
        }
        let name = self.parse_ident()?;
        self.skip();
        if self.peek() == Some(b'(') {
            self.pos += 1;
            if name == "abs" {
                let inner = self.parse_expr()?;
                self.expect_close()?;
                return Ok(Node::Abs(Box::new(inner)));
            }
            if name == "lp" {
                let inner = self.parse_expr()?;
                self.skip();
                if self.peek() != Some(b',') {
                    return Err(Error::msg("lp(signal, alpha) needs a comma"));
                }
                self.pos += 1;
                let alpha = match self.parse_unary()? {
                    Node::Num(value) => value,
                    _ => return Err(Error::msg("lp alpha must be a number")),
                };
                if !(0.0..=1.0).contains(&alpha) {
                    return Err(Error::msg("lp alpha must be between 0 and 1"));
                }
                self.expect_close()?;
                return Ok(Node::Lp(Box::new(inner), alpha));
            }
            return Err(Error::msg(format!(
                "unknown function {name}. Use abs or lp."
            )));
        }
        Ok(Node::Var(name))
    }

    fn parse_number(&mut self) -> Result<Node> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        let value: f64 = text
            .parse()
            .map_err(|_| Error::msg(format!("bad number '{text}'")))?;
        if !value.is_finite() {
            return Err(Error::msg("numbers must be finite"));
        }
        Ok(Node::Num(value))
    }

    fn parse_ident(&mut self) -> Result<String> {
        let start = self.pos;
        let Some(first) = self.peek() else {
            return Err(Error::msg("math expression ended early"));
        };
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return Err(Error::msg("expected a signal name"));
        }
        self.pos += 1;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.pos += 1;
        }
        Ok(std::str::from_utf8(&self.bytes[start..self.pos])
            .unwrap_or("")
            .to_string())
    }

    fn expect_close(&mut self) -> Result<()> {
        self.skip();
        if self.peek() != Some(b')') {
            return Err(Error::msg("math expression is missing ')'"));
        }
        self.pos += 1;
        Ok(())
    }

    fn skip(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtracts_wheel_speeds_and_filters() {
        let compiled = compile("abs(WheelFL - WheelFR)").unwrap();
        assert_eq!(
            compiled.dependencies(),
            vec!["WheelFL".to_string(), "WheelFR".to_string()]
        );
        let mut vars = HashMap::from([
            ("WheelFL".to_string(), 100.0),
            ("WheelFR".to_string(), 98.5),
        ]);
        let mut state = Vec::new();
        let value = compiled.eval(&vars, &mut state).unwrap().unwrap();
        assert!((value - 1.5).abs() < 1e-9);

        let filtered = compile("lp(EngineRPM, 0.5)").unwrap();
        vars.insert("EngineRPM".to_string(), 0.0);
        let mut state = Vec::new();
        assert!((filtered.eval(&vars, &mut state).unwrap().unwrap() - 0.0).abs() < 1e-9);
        vars.insert("EngineRPM".to_string(), 100.0);
        let mid = filtered.eval(&vars, &mut state).unwrap().unwrap();
        assert!((mid - 50.0).abs() < 1e-6, "{mid}");
    }

    #[test]
    fn a_zero_divisor_is_a_gap_and_keeps_the_filter_clean() {
        let ratio = compile("lp(Torque / Speed, 0.5)").unwrap();
        let mut state = Vec::new();
        let mut vars = HashMap::from([("Torque".to_string(), 10.0), ("Speed".to_string(), 5.0)]);
        assert_eq!(ratio.eval(&vars, &mut state).unwrap(), Some(2.0));
        vars.insert("Speed".to_string(), 0.0);
        assert_eq!(ratio.eval(&vars, &mut state).unwrap(), None);
        vars.insert("Speed".to_string(), 2.5);
        // 2 + 0.5 * (4 - 2): the gap did not reach the filter.
        assert_eq!(ratio.eval(&vars, &mut state).unwrap(), Some(3.0));
    }

    #[test]
    fn rejects_a_trailing_token() {
        let err = compile("WheelFL +").unwrap_err();
        assert!(
            err.to_string().contains("ended") || err.to_string().contains("expected"),
            "{err}"
        );
    }
}
