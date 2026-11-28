//! Derived channels. Supports identifiers, numbers, arithmetic (`+ - * / % ^`),
//! comparisons (`> >= < <= == !=`), logic (`&& || !`), parentheses, stateless math
//! functions (`abs`, `sqrt`, `pow`, `min`, `max`, `avg`, `clamp`, `round`, `floor`,
//! `ceil`, `sign`, `sin`, `cos`, `tan`, `atan2`, `exp`, `ln`, `log10`, `hypot`,
//! `if`), and stateful signal filters (`lp`, `sma`, `delta`, `deriv`, `integ`).

use crate::error::{Error, Result};
use std::collections::VecDeque;

const MAX_SMA_WINDOW: usize = 1024;

/// An expression with its signal names resolved to indices: variable `i` is
/// `dependencies()[i]`.
#[derive(Debug, Clone)]
pub struct Compiled {
    root: Node,
    names: Vec<String>,
    state_slots: usize,
}

/// Per-evaluation memory for stateful operators (`lp`, `sma`, `delta`, `deriv`, `integ`).
#[derive(Debug, Default, Clone)]
pub struct EvalState {
    slots: Vec<SlotState>,
}

#[derive(Debug, Clone)]
enum SlotState {
    Empty,
    Lp(f64),
    Sma {
        buf: VecDeque<f64>,
        sum: f64,
    },
    Delta(f64),
    Deriv {
        prev_t: u64,
        prev_x: f64,
        last_rate: f64,
    },
    Integ {
        prev_t: u64,
        prev_x: f64,
        acc: f64,
    },
}

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    Var(usize),
    Neg(Box<Node>),
    Not(Box<Node>),
    Unary(UnaryFn, Box<Node>),
    Binary(BinOp, Box<Node>, Box<Node>),
    Variadic(VariadicFn, Vec<Node>),
    Clamp(Box<Node>, Box<Node>, Box<Node>),
    If(Box<Node>, Box<Node>, Box<Node>),
    Lp {
        inner: Box<Node>,
        alpha: f64,
        slot: usize,
    },
    Sma {
        inner: Box<Node>,
        window: usize,
        slot: usize,
    },
    Delta {
        inner: Box<Node>,
        slot: usize,
    },
    Deriv {
        inner: Box<Node>,
        slot: usize,
    },
    Integ {
        inner: Box<Node>,
        slot: usize,
    },
}

#[derive(Debug, Clone, Copy)]
enum UnaryFn {
    Abs,
    Sqrt,
    Round,
    Floor,
    Ceil,
    Sign,
    Sin,
    Cos,
    Tan,
    Exp,
    Ln,
    Log10,
}

#[derive(Debug, Clone, Copy)]
enum VariadicFn {
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Atan2,
    Hypot,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

pub fn compile(expr: &str) -> Result<Compiled> {
    let mut parser = Parser {
        bytes: expr.as_bytes(),
        pos: 0,
        names: Vec::new(),
        state_slots: 0,
    };
    let root = parser.parse_or()?;
    parser.skip();
    if parser.pos != parser.bytes.len() {
        return Err(Error::invalid(format!(
            "could not parse the rest of '{expr}'"
        )));
    }
    Ok(Compiled {
        root,
        names: parser.names,
        state_slots: parser.state_slots,
    })
}

impl Compiled {
    /// Signal names in order of first use.
    pub fn dependencies(&self) -> &[String] {
        &self.names
    }

    /// Evaluate one sample at timestamp `t_us`. `vars[i]` is the value of
    /// `dependencies()[i]`. A non-finite result, such as a division by zero, is
    /// `None`: that sample is a gap in the channel, not an error for the whole query.
    pub fn eval_at(&self, t_us: u64, vars: &[f64], state: &mut EvalState) -> Option<f64> {
        if state.slots.len() < self.state_slots {
            state.slots.resize(self.state_slots, SlotState::Empty);
        }
        let value = self.root.eval(t_us, vars, &mut state.slots);
        value.is_finite().then_some(value)
    }

    /// Evaluate one sample without a timestamp, keeping `lp` state in `lp_state`.
    #[cfg(test)]
    pub fn eval(&self, vars: &[f64], lp_state: &mut Vec<f64>) -> Option<f64> {
        let mut state = EvalState {
            slots: lp_state.iter().map(|&v| SlotState::Lp(v)).collect(),
        };
        let out = self.eval_at(0, vars, &mut state);
        lp_state.clear();
        for slot in state.slots {
            if let SlotState::Lp(v) = slot {
                lp_state.push(v);
            }
        }
        out
    }

    /// Evaluate over time-sorted dependency series, `inputs[i]` being the
    /// points of `dependencies()[i]`. Each series holds its last value, and a
    /// sample is produced at every timestamp of any input once all of them
    /// have a value.
    pub fn eval_series(&self, inputs: &[&[(u64, f64)]]) -> Vec<(u64, f64)> {
        debug_assert_eq!(inputs.len(), self.names.len());
        let mut cursors = vec![0usize; inputs.len()];
        let mut vars = vec![0.0; inputs.len()];
        let mut held = 0usize;
        let mut state = EvalState::default();
        let mut points = Vec::new();
        while let Some(t) = inputs
            .iter()
            .zip(&cursors)
            .filter_map(|(points, cursor)| points.get(*cursor).map(|(t, _)| *t))
            .min()
        {
            for ((points, cursor), var) in inputs.iter().zip(&mut cursors).zip(&mut vars) {
                while let Some(&(_, value)) = points.get(*cursor).filter(|(stamp, _)| *stamp <= t) {
                    if *cursor == 0 {
                        held += 1;
                    }
                    *var = value;
                    *cursor += 1;
                }
            }
            if held == inputs.len() {
                if let Some(value) = self.eval_at(t, &vars, &mut state) {
                    points.push((t, value));
                }
            }
        }
        points
    }
}

impl Node {
    fn eval(&self, t_us: u64, vars: &[f64], slots: &mut [SlotState]) -> f64 {
        match self {
            Node::Num(value) => *value,
            Node::Var(index) => vars[*index],
            Node::Neg(inner) => -inner.eval(t_us, vars, slots),
            Node::Not(inner) => {
                let v = inner.eval(t_us, vars, slots);
                if !v.is_finite() {
                    return f64::NAN;
                }
                if v == 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Node::Unary(func, inner) => {
                let x = inner.eval(t_us, vars, slots);
                match func {
                    UnaryFn::Abs => x.abs(),
                    UnaryFn::Sqrt => x.sqrt(),
                    UnaryFn::Round => x.round(),
                    UnaryFn::Floor => x.floor(),
                    UnaryFn::Ceil => x.ceil(),
                    UnaryFn::Sign => {
                        if !x.is_finite() {
                            f64::NAN
                        } else if x > 0.0 {
                            1.0
                        } else if x < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    UnaryFn::Sin => x.sin(),
                    UnaryFn::Cos => x.cos(),
                    UnaryFn::Tan => x.tan(),
                    UnaryFn::Exp => x.exp(),
                    UnaryFn::Ln => x.ln(),
                    UnaryFn::Log10 => x.log10(),
                }
            }
            Node::Binary(op, left, right) => {
                let a = left.eval(t_us, vars, slots);
                let b = right.eval(t_us, vars, slots);
                match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    BinOp::Div => a / b,
                    BinOp::Rem => a % b,
                    BinOp::Pow => a.powf(b),
                    BinOp::Atan2 => a.atan2(b),
                    BinOp::Hypot => a.hypot(b),
                    BinOp::Lt => cmp_num(a, b, |x, y| x < y),
                    BinOp::Le => cmp_num(a, b, |x, y| x <= y),
                    BinOp::Gt => cmp_num(a, b, |x, y| x > y),
                    BinOp::Ge => cmp_num(a, b, |x, y| x >= y),
                    BinOp::Eq => cmp_num(a, b, |x, y| (x - y).abs() <= 1e-12),
                    BinOp::Ne => cmp_num(a, b, |x, y| (x - y).abs() > 1e-12),
                    BinOp::And => {
                        if !a.is_finite() || !b.is_finite() {
                            f64::NAN
                        } else if a != 0.0 && b != 0.0 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinOp::Or => {
                        if !a.is_finite() || !b.is_finite() {
                            f64::NAN
                        } else if a != 0.0 || b != 0.0 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                }
            }
            Node::Variadic(func, args) => {
                let mut acc = args[0].eval(t_us, vars, slots);
                if !acc.is_finite() {
                    return f64::NAN;
                }
                for arg in &args[1..] {
                    let v = arg.eval(t_us, vars, slots);
                    if !v.is_finite() {
                        return f64::NAN;
                    }
                    match func {
                        VariadicFn::Min => acc = acc.min(v),
                        VariadicFn::Max => acc = acc.max(v),
                        VariadicFn::Avg => acc += v,
                    }
                }
                match func {
                    VariadicFn::Avg => acc / args.len() as f64,
                    VariadicFn::Min | VariadicFn::Max => acc,
                }
            }
            Node::Clamp(val, lo, hi) => {
                let x = val.eval(t_us, vars, slots);
                let min = lo.eval(t_us, vars, slots);
                let max = hi.eval(t_us, vars, slots);
                if !x.is_finite() || !min.is_finite() || !max.is_finite() {
                    return f64::NAN;
                }
                let (a, b) = if min <= max { (min, max) } else { (max, min) };
                x.clamp(a, b)
            }
            Node::If(cond, then_node, else_node) => {
                let c = cond.eval(t_us, vars, slots);
                if !c.is_finite() {
                    return f64::NAN;
                }
                if c != 0.0 {
                    then_node.eval(t_us, vars, slots)
                } else {
                    else_node.eval(t_us, vars, slots)
                }
            }
            Node::Lp { inner, alpha, slot } => {
                let sample = inner.eval(t_us, vars, slots);
                if !sample.is_finite() {
                    // Keep the filter's memory clean; this sample is a gap.
                    return sample;
                }
                match &mut slots[*slot] {
                    SlotState::Lp(prev) => {
                        let next = *prev + alpha * (sample - *prev);
                        *prev = next;
                        next
                    }
                    other => {
                        *other = SlotState::Lp(sample);
                        sample
                    }
                }
            }
            Node::Sma {
                inner,
                window,
                slot,
            } => {
                let sample = inner.eval(t_us, vars, slots);
                if !sample.is_finite() {
                    return sample;
                }
                match &mut slots[*slot] {
                    SlotState::Sma { buf, sum } => {
                        buf.push_back(sample);
                        *sum += sample;
                        if buf.len() > *window {
                            if let Some(old) = buf.pop_front() {
                                *sum -= old;
                            }
                        }
                        *sum / buf.len() as f64
                    }
                    other => {
                        let mut buf = VecDeque::with_capacity(*window);
                        buf.push_back(sample);
                        *other = SlotState::Sma { buf, sum: sample };
                        sample
                    }
                }
            }
            Node::Delta { inner, slot } => {
                let sample = inner.eval(t_us, vars, slots);
                if !sample.is_finite() {
                    return sample;
                }
                match &mut slots[*slot] {
                    SlotState::Delta(prev) => {
                        let diff = sample - *prev;
                        *prev = sample;
                        diff
                    }
                    other => {
                        *other = SlotState::Delta(sample);
                        0.0
                    }
                }
            }
            Node::Deriv { inner, slot } => {
                let sample = inner.eval(t_us, vars, slots);
                if !sample.is_finite() {
                    return sample;
                }
                match &mut slots[*slot] {
                    SlotState::Deriv {
                        prev_t,
                        prev_x,
                        last_rate,
                    } => {
                        if t_us > *prev_t {
                            let dt_s = (t_us - *prev_t) as f64 / 1_000_000.0;
                            let rate = (sample - *prev_x) / dt_s;
                            *prev_t = t_us;
                            *prev_x = sample;
                            *last_rate = rate;
                            rate
                        } else {
                            *prev_x = sample;
                            *last_rate
                        }
                    }
                    other => {
                        *other = SlotState::Deriv {
                            prev_t: t_us,
                            prev_x: sample,
                            last_rate: 0.0,
                        };
                        0.0
                    }
                }
            }
            Node::Integ { inner, slot } => {
                let sample = inner.eval(t_us, vars, slots);
                if !sample.is_finite() {
                    return sample;
                }
                match &mut slots[*slot] {
                    SlotState::Integ {
                        prev_t,
                        prev_x,
                        acc,
                    } => {
                        if t_us > *prev_t {
                            let dt_s = (t_us - *prev_t) as f64 / 1_000_000.0;
                            *acc += 0.5 * (*prev_x + sample) * dt_s;
                            *prev_t = t_us;
                            *prev_x = sample;
                        } else {
                            *prev_x = sample;
                        }
                        *acc
                    }
                    other => {
                        *other = SlotState::Integ {
                            prev_t: t_us,
                            prev_x: sample,
                            acc: 0.0,
                        };
                        0.0
                    }
                }
            }
        }
    }
}

fn cmp_num(a: f64, b: f64, f: impl FnOnce(f64, f64) -> bool) -> f64 {
    if !a.is_finite() || !b.is_finite() {
        f64::NAN
    } else if f(a, b) {
        1.0
    } else {
        0.0
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    names: Vec<String>,
    state_slots: usize,
}

impl<'a> Parser<'a> {
    fn alloc_slot(&mut self) -> usize {
        let slot = self.state_slots;
        self.state_slots += 1;
        slot
    }

    fn parse_or(&mut self) -> Result<Node> {
        let mut node = self.parse_and()?;
        loop {
            self.skip();
            if self.peek2() == Some((b'|', b'|')) {
                self.pos += 2;
                let right = self.parse_and()?;
                node = Node::Binary(BinOp::Or, Box::new(node), Box::new(right));
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> Result<Node> {
        let mut node = self.parse_cmp()?;
        loop {
            self.skip();
            if self.peek2() == Some((b'&', b'&')) {
                self.pos += 2;
                let right = self.parse_cmp()?;
                node = Node::Binary(BinOp::And, Box::new(node), Box::new(right));
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_cmp(&mut self) -> Result<Node> {
        let mut node = self.parse_expr()?;
        loop {
            self.skip();
            let (op, len) = match self.peek2() {
                Some((b'=', b'=')) => (BinOp::Eq, 2),
                Some((b'!', b'=')) => (BinOp::Ne, 2),
                Some((b'<', b'=')) => (BinOp::Le, 2),
                Some((b'>', b'=')) => (BinOp::Ge, 2),
                _ => match self.peek() {
                    Some(b'<') => (BinOp::Lt, 1),
                    Some(b'>') => (BinOp::Gt, 1),
                    _ => break,
                },
            };
            self.pos += len;
            let right = self.parse_expr()?;
            node = Node::Binary(op, Box::new(node), Box::new(right));
        }
        Ok(node)
    }

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
            node = Node::Binary(op, Box::new(node), Box::new(right));
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
                Some(b'%') => BinOp::Rem,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            node = Node::Binary(op, Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> Result<Node> {
        self.skip();
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                Ok(Node::Neg(Box::new(self.parse_unary()?)))
            }
            Some(b'+') => {
                self.pos += 1;
                self.parse_unary()
            }
            Some(b'!') if self.peek2() != Some((b'!', b'=')) => {
                self.pos += 1;
                Ok(Node::Not(Box::new(self.parse_unary()?)))
            }
            _ => self.parse_pow(),
        }
    }

    fn parse_pow(&mut self) -> Result<Node> {
        let base = self.parse_primary()?;
        self.skip();
        if self.peek() == Some(b'^') {
            self.pos += 1;
            let exp = self.parse_unary()?;
            return Ok(Node::Binary(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn parse_primary(&mut self) -> Result<Node> {
        self.skip();
        if self.peek() == Some(b'(') {
            self.pos += 1;
            let node = self.parse_or()?;
            self.expect_close()?;
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
            return self.parse_call(&name);
        }
        let index = match self.names.iter().position(|seen| *seen == name) {
            Some(index) => index,
            None => {
                self.names.push(name);
                self.names.len() - 1
            }
        };
        Ok(Node::Var(index))
    }

    fn parse_call(&mut self, name: &str) -> Result<Node> {
        let unary = match name {
            "abs" => Some(UnaryFn::Abs),
            "sqrt" => Some(UnaryFn::Sqrt),
            "round" => Some(UnaryFn::Round),
            "floor" => Some(UnaryFn::Floor),
            "ceil" => Some(UnaryFn::Ceil),
            "sign" => Some(UnaryFn::Sign),
            "sin" => Some(UnaryFn::Sin),
            "cos" => Some(UnaryFn::Cos),
            "tan" => Some(UnaryFn::Tan),
            "exp" => Some(UnaryFn::Exp),
            "ln" => Some(UnaryFn::Ln),
            "log10" => Some(UnaryFn::Log10),
            _ => None,
        };
        if let Some(func) = unary {
            let inner = self.parse_or()?;
            self.expect_close()?;
            return Ok(Node::Unary(func, Box::new(inner)));
        }
        match name {
            "pow" | "atan2" | "hypot" => {
                let a = self.parse_or()?;
                self.expect_comma(name)?;
                let b = self.parse_or()?;
                self.expect_close()?;
                let op = match name {
                    "pow" => BinOp::Pow,
                    "atan2" => BinOp::Atan2,
                    _ => BinOp::Hypot,
                };
                Ok(Node::Binary(op, Box::new(a), Box::new(b)))
            }
            "min" | "max" | "avg" => {
                let mut args = vec![self.parse_or()?];
                loop {
                    self.skip();
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                        args.push(self.parse_or()?);
                    } else {
                        break;
                    }
                }
                if args.len() < 2 {
                    return Err(Error::invalid(format!(
                        "{name}() needs at least 2 arguments"
                    )));
                }
                self.expect_close()?;
                let func = match name {
                    "min" => VariadicFn::Min,
                    "max" => VariadicFn::Max,
                    _ => VariadicFn::Avg,
                };
                Ok(Node::Variadic(func, args))
            }
            "clamp" => {
                let val = self.parse_or()?;
                self.expect_comma("clamp")?;
                let lo = self.parse_or()?;
                self.expect_comma("clamp")?;
                let hi = self.parse_or()?;
                self.expect_close()?;
                Ok(Node::Clamp(Box::new(val), Box::new(lo), Box::new(hi)))
            }
            "if" => {
                let cond = self.parse_or()?;
                self.expect_comma("if")?;
                let then_node = self.parse_or()?;
                self.expect_comma("if")?;
                let else_node = self.parse_or()?;
                self.expect_close()?;
                Ok(Node::If(
                    Box::new(cond),
                    Box::new(then_node),
                    Box::new(else_node),
                ))
            }
            "lp" => {
                let inner = self.parse_or()?;
                self.skip();
                if self.peek() != Some(b',') {
                    return Err(Error::invalid("lp(signal, alpha) needs a comma"));
                }
                self.pos += 1;
                let alpha = self.parse_const_number("lp alpha")?;
                if !(0.0..=1.0).contains(&alpha) {
                    return Err(Error::invalid("lp alpha must be between 0 and 1"));
                }
                self.expect_close()?;
                let slot = self.alloc_slot();
                Ok(Node::Lp {
                    inner: Box::new(inner),
                    alpha,
                    slot,
                })
            }
            "sma" => {
                let inner = self.parse_or()?;
                self.expect_comma("sma")?;
                let n = self.parse_const_number("sma window")?;
                if !n.is_finite() || n.fract() != 0.0 || n < 1.0 || n > MAX_SMA_WINDOW as f64 {
                    return Err(Error::invalid(format!(
                        "sma window must be an integer between 1 and {MAX_SMA_WINDOW}"
                    )));
                }
                self.expect_close()?;
                let slot = self.alloc_slot();
                Ok(Node::Sma {
                    inner: Box::new(inner),
                    window: n as usize,
                    slot,
                })
            }
            "delta" => {
                let inner = self.parse_or()?;
                self.expect_close()?;
                let slot = self.alloc_slot();
                Ok(Node::Delta {
                    inner: Box::new(inner),
                    slot,
                })
            }
            "deriv" => {
                let inner = self.parse_or()?;
                self.expect_close()?;
                let slot = self.alloc_slot();
                Ok(Node::Deriv {
                    inner: Box::new(inner),
                    slot,
                })
            }
            "integ" => {
                let inner = self.parse_or()?;
                self.expect_close()?;
                let slot = self.alloc_slot();
                Ok(Node::Integ {
                    inner: Box::new(inner),
                    slot,
                })
            }
            _ => Err(Error::invalid(format!(
                "unknown function {name}. Supported: abs, sqrt, pow, min, max, avg, clamp, round, floor, ceil, sign, sin, cos, tan, atan2, exp, ln, log10, hypot, if, lp, sma, delta, deriv, integ."
            ))),
        }
    }

    fn parse_const_number(&mut self, label: &str) -> Result<f64> {
        match self.parse_unary()? {
            Node::Num(value) => Ok(value),
            Node::Neg(inner) => match *inner {
                Node::Num(value) => Ok(-value),
                _ => Err(Error::invalid(format!("{label} must be a number"))),
            },
            _ => Err(Error::invalid(format!("{label} must be a number"))),
        }
    }

    fn parse_number(&mut self) -> Result<Node> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            self.pos += 1;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            let exp_start = self.pos;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.pos == exp_start {
                self.pos = save;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        let value: f64 = text
            .parse()
            .map_err(|_| Error::invalid(format!("bad number '{text}'")))?;
        if !value.is_finite() {
            return Err(Error::invalid("numbers must be finite"));
        }
        Ok(Node::Num(value))
    }

    fn parse_ident(&mut self) -> Result<String> {
        let start = self.pos;
        let Some(first) = self.peek() else {
            return Err(Error::invalid("math expression ended early"));
        };
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return Err(Error::invalid("expected a signal name"));
        }
        self.pos += 1;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'@')
        {
            self.pos += 1;
        }
        Ok(std::str::from_utf8(&self.bytes[start..self.pos])
            .unwrap_or("")
            .to_string())
    }

    fn expect_comma(&mut self, name: &str) -> Result<()> {
        self.skip();
        if self.peek() != Some(b',') {
            return Err(Error::invalid(format!("{name}() needs a comma")));
        }
        self.pos += 1;
        Ok(())
    }

    fn expect_close(&mut self) -> Result<()> {
        self.skip();
        if self.peek() != Some(b')') {
            return Err(Error::invalid("math expression is missing ')'"));
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

    fn peek2(&self) -> Option<(u8, u8)> {
        Some((*self.bytes.get(self.pos)?, *self.bytes.get(self.pos + 1)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtracts_wheel_speeds_and_filters() {
        let compiled = compile("abs(WheelFL - WheelFR)").unwrap();
        assert_eq!(compiled.dependencies(), ["WheelFL", "WheelFR"]);
        let mut state = Vec::new();
        let value = compiled.eval(&[100.0, 98.5], &mut state).unwrap();
        assert!((value - 1.5).abs() < 1e-9);

        let filtered = compile("lp(EngineRPM, 0.5)").unwrap();
        let mut state = Vec::new();
        assert!((filtered.eval(&[0.0], &mut state).unwrap() - 0.0).abs() < 1e-9);
        let mid = filtered.eval(&[100.0], &mut state).unwrap();
        assert!((mid - 50.0).abs() < 1e-6, "{mid}");
    }

    #[test]
    fn a_zero_divisor_is_a_gap_and_keeps_the_filter_clean() {
        let ratio = compile("lp(Torque / Speed, 0.5)").unwrap();
        assert_eq!(ratio.dependencies(), ["Torque", "Speed"]);
        let mut state = Vec::new();
        assert_eq!(ratio.eval(&[10.0, 5.0], &mut state), Some(2.0));
        assert_eq!(ratio.eval(&[10.0, 0.0], &mut state), None);
        // 2 + 0.5 * (4 - 2): the gap did not reach the filter.
        assert_eq!(ratio.eval(&[10.0, 2.5], &mut state), Some(3.0));
    }

    #[test]
    fn a_repeated_name_is_one_variable() {
        let squared = compile("A * A + B").unwrap();
        assert_eq!(squared.dependencies(), ["A", "B"]);
        assert_eq!(squared.eval(&[3.0, 1.0], &mut Vec::new()), Some(10.0));
    }

    #[test]
    fn series_evaluate_once_every_input_has_a_value() {
        let diff = compile("A - B").unwrap();
        let a = [(0, 1.0), (10, 5.0), (30, 2.0)];
        let b = [(5, 1.0), (20, 4.0)];
        assert_eq!(
            diff.eval_series(&[&a[..], &b[..]]),
            [(5, 0.0), (10, 4.0), (20, 1.0), (30, -2.0)]
        );
    }

    #[test]
    fn rejects_a_trailing_token() {
        let err = compile("WheelFL +").unwrap_err();
        assert!(
            err.to_string().contains("ended") || err.to_string().contains("expected"),
            "{err}"
        );
    }

    #[test]
    fn at_tagged_signal_names_parse_in_math_expressions() {
        let compiled = compile("abs(BrakePressure@1 - BrakePressure@2_1A0)").unwrap();
        assert_eq!(
            compiled.dependencies(),
            ["BrakePressure@1", "BrakePressure@2_1A0"]
        );
        assert_eq!(compiled.eval(&[42.0, 40.5], &mut Vec::new()), Some(1.5));
    }

    #[test]
    fn supports_power_comparisons_logic_and_stateless_functions() {
        let mut s = EvalState::default();
        assert_eq!(
            compile("2 ^ 3 ^ 2").unwrap().eval_at(0, &[], &mut s),
            Some(512.0)
        );
        assert_eq!(
            compile("-2 ^ 2").unwrap().eval_at(0, &[], &mut s),
            Some(-4.0)
        );
        assert_eq!(
            compile("2 ^ -2").unwrap().eval_at(0, &[], &mut s),
            Some(0.25)
        );
        assert_eq!(
            compile("10 % 3").unwrap().eval_at(0, &[], &mut s),
            Some(1.0)
        );
        assert_eq!(
            compile("if(Speed > 0, Torque / Speed, 0)")
                .unwrap()
                .eval_at(0, &[0.0, 100.0], &mut s),
            Some(0.0)
        );
        assert_eq!(
            compile("if(Speed > 0 && Torque >= 50, Torque / Speed, -1)")
                .unwrap()
                .eval_at(0, &[10.0, 50.0], &mut s),
            Some(5.0)
        );
        assert_eq!(
            compile("clamp(A, 0, 100) + min(A, B, 20) + max(A, B) + avg(A, B)")
                .unwrap()
                .eval_at(0, &[120.0, 10.0], &mut s),
            Some(100.0 + 10.0 + 120.0 + 65.0)
        );
        assert_eq!(
            compile("hypot(3, 4) + sqrt(16) + round(2.6) + floor(2.9) + ceil(2.1) + sign(-5)")
                .unwrap()
                .eval_at(0, &[], &mut s),
            Some(5.0 + 4.0 + 3.0 + 2.0 + 3.0 - 1.0)
        );
    }

    #[test]
    fn supports_sma_delta_deriv_and_integ() {
        let a = [
            (0, 0.0),
            (1_000_000, 10.0),
            (2_000_000, 30.0),
            (3_000_000, 30.0),
        ];

        let sma = compile("sma(A, 2)").unwrap();
        assert_eq!(
            sma.eval_series(&[&a[..]]),
            [
                (0, 0.0),
                (1_000_000, 5.0),
                (2_000_000, 20.0),
                (3_000_000, 30.0)
            ]
        );

        let delta = compile("delta(A)").unwrap();
        assert_eq!(
            delta.eval_series(&[&a[..]]),
            [
                (0, 0.0),
                (1_000_000, 10.0),
                (2_000_000, 20.0),
                (3_000_000, 0.0)
            ]
        );

        let deriv = compile("deriv(A)").unwrap();
        assert_eq!(
            deriv.eval_series(&[&a[..]]),
            [
                (0, 0.0),
                (1_000_000, 10.0),
                (2_000_000, 20.0),
                (3_000_000, 0.0)
            ]
        );

        let integ = compile("integ(A)").unwrap();
        // 0..1s: avg 5 * 1s = 5; 1..2s: avg 20 * 1s = 25; 2..3s: avg 30 * 1s = 55
        assert_eq!(
            integ.eval_series(&[&a[..]]),
            [
                (0, 0.0),
                (1_000_000, 5.0),
                (2_000_000, 25.0),
                (3_000_000, 55.0)
            ]
        );
    }
}
