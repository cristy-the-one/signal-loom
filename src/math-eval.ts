/**
 * Client-side mirror of `crates/loom-core/src/analyze.rs`:
 * - Live syntax & dependency checking as the user edits a formula
 * - Instant single-sample playhead evaluation (for editor preview & cluster widgets)
 * - Function catalog & guided formula recipes so users don't have to hand-write formulas
 */

import type { SignalInfo } from "./types";

const MAX_SMA_WINDOW = 1024;

export interface MathFnInfo {
  name: string;
  signature: string;
  /** Snippet inserted when clicked; cursor placed at first `|` if present. */
  snippet: string;
  category: "filter" | "math" | "logic";
  desc: string;
}

export const MATH_FUNCTIONS: readonly MathFnInfo[] = [
  { name: "lp", signature: "lp(x, alpha)", snippet: "lp(|, 0.2)", category: "filter", desc: "One-pole low-pass filter (0 < alpha <= 1)" },
  { name: "sma", signature: "sma(x, n)", snippet: "sma(|, 10)", category: "filter", desc: "Simple moving average over last n samples" },
  { name: "deriv", signature: "deriv(x)", snippet: "deriv(|)", category: "filter", desc: "Rate of change dx/dt (units per second)" },
  { name: "delta", signature: "delta(x)", snippet: "delta(|)", category: "filter", desc: "Change since previous sample" },
  { name: "integ", signature: "integ(x)", snippet: "integ(|)", category: "filter", desc: "Trapezoidal time integral (units·s)" },
  { name: "abs", signature: "abs(x)", snippet: "abs(|)", category: "math", desc: "Absolute value" },
  { name: "min", signature: "min(a, b, …)", snippet: "min(|, )", category: "math", desc: "Minimum of 2 or more values" },
  { name: "max", signature: "max(a, b, …)", snippet: "max(|, )", category: "math", desc: "Maximum of 2 or more values" },
  { name: "avg", signature: "avg(a, b, …)", snippet: "avg(|, )", category: "math", desc: "Average of 2 or more values" },
  { name: "clamp", signature: "clamp(x, lo, hi)", snippet: "clamp(|, 0, 100)", category: "math", desc: "Constrain x to [lo, hi]" },
  { name: "sqrt", signature: "sqrt(x)", snippet: "sqrt(|)", category: "math", desc: "Square root" },
  { name: "pow", signature: "pow(x, y)", snippet: "pow(|, 2)", category: "math", desc: "Raise x to power y (or use x ^ y)" },
  { name: "hypot", signature: "hypot(a, b)", snippet: "hypot(|, )", category: "math", desc: "Euclidean magnitude sqrt(a² + b²)" },
  { name: "round", signature: "round(x)", snippet: "round(|)", category: "math", desc: "Nearest integer" },
  { name: "floor", signature: "floor(x)", snippet: "floor(|)", category: "math", desc: "Round down" },
  { name: "ceil", signature: "ceil(x)", snippet: "ceil(|)", category: "math", desc: "Round up" },
  { name: "sign", signature: "sign(x)", snippet: "sign(|)", category: "math", desc: "-1, 0, or 1" },
  { name: "sin", signature: "sin(x)", snippet: "sin(|)", category: "math", desc: "Sine (radians)" },
  { name: "cos", signature: "cos(x)", snippet: "cos(|)", category: "math", desc: "Cosine (radians)" },
  { name: "tan", signature: "tan(x)", snippet: "tan(|)", category: "math", desc: "Tangent (radians)" },
  { name: "atan2", signature: "atan2(y, x)", snippet: "atan2(|, )", category: "math", desc: "Two-argument arctangent (radians)" },
  { name: "exp", signature: "exp(x)", snippet: "exp(|)", category: "math", desc: "eˣ" },
  { name: "ln", signature: "ln(x)", snippet: "ln(|)", category: "math", desc: "Natural logarithm" },
  { name: "log10", signature: "log10(x)", snippet: "log10(|)", category: "math", desc: "Base-10 logarithm" },
  { name: "if", signature: "if(cond, then, else)", snippet: "if(| > 0, 1, 0)", category: "logic", desc: "Conditional branch (lazy)" },
];

export interface RecipeSignalField {
  key: string;
  label: string;
}

export interface RecipeParamField {
  key: string;
  label: string;
  defaultValue: string;
  step?: string;
  options?: string[];
}

export interface MathRecipe {
  id: string;
  label: string;
  desc: string;
  signals: RecipeSignalField[];
  params: RecipeParamField[];
  build(
    signals: Record<string, SignalInfo | undefined>,
    params: Record<string, string>,
  ): { expr: string; name: string; unit: string } | null;
}

function numParam(params: Record<string, string>, key: string, fallback: string): string {
  const raw = (params[key] ?? fallback).trim();
  return raw || fallback;
}

export const MATH_RECIPES: readonly MathRecipe[] = [
  {
    id: "diff",
    label: "Difference (A − B)",
    desc: "Subtract one signal from another",
    signals: [
      { key: "a", label: "Signal A" },
      { key: "b", label: "Signal B" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? { expr: `${s.a.name} - ${s.b.name}`, name: `${s.a.name}_diff`, unit: s.a.unit || s.b.unit }
        : null,
  },
  {
    id: "abs_diff",
    label: "Abs Difference",
    desc: "Absolute difference |A − B|",
    signals: [
      { key: "a", label: "Signal A" },
      { key: "b", label: "Signal B" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? { expr: `abs(${s.a.name} - ${s.b.name})`, name: `${s.a.name}_absdiff`, unit: s.a.unit || s.b.unit }
        : null,
  },
  {
    id: "sum",
    label: "Sum (A + B)",
    desc: "Add two signals together",
    signals: [
      { key: "a", label: "Signal A" },
      { key: "b", label: "Signal B" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? { expr: `${s.a.name} + ${s.b.name}`, name: `${s.a.name}_sum`, unit: s.a.unit || s.b.unit }
        : null,
  },
  {
    id: "avg",
    label: "Average",
    desc: "Mean of two signals",
    signals: [
      { key: "a", label: "Signal A" },
      { key: "b", label: "Signal B" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? { expr: `avg(${s.a.name}, ${s.b.name})`, name: `${s.a.name}_avg`, unit: s.a.unit || s.b.unit }
        : null,
  },
  {
    id: "scale",
    label: "Scale & Offset",
    desc: "Unit conversion: A × k + b",
    signals: [{ key: "a", label: "Signal" }],
    params: [
      { key: "k", label: "Factor (k)", defaultValue: "1", step: "any" },
      { key: "b", label: "Offset (b)", defaultValue: "0", step: "any" },
    ],
    build: (s, p) => {
      if (!s.a) return null;
      const k = numParam(p, "k", "1");
      const b = numParam(p, "b", "0");
      const bNum = Number(b);
      const expr =
        bNum === 0
          ? `${s.a.name} * ${k}`
          : bNum < 0
            ? `${s.a.name} * ${k} - ${Math.abs(bNum)}`
            : `${s.a.name} * ${k} + ${b}`;
      return { expr, name: `${s.a.name}_scaled`, unit: s.a.unit };
    },
  },
  {
    id: "ratio",
    label: "Safe Ratio (A / B)",
    desc: "Divide A by B, returning 0 when B is near zero",
    signals: [
      { key: "a", label: "Numerator (A)" },
      { key: "b", label: "Denominator (B)" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? { expr: `if(abs(${s.b.name}) > 0.001, ${s.a.name} / ${s.b.name}, 0)`, name: `${s.a.name}_ratio`, unit: "" }
        : null,
  },
  {
    id: "slip",
    label: "Percent Diff / Slip",
    desc: "100 × (A − B) / B, guarded against zero",
    signals: [
      { key: "a", label: "Signal A" },
      { key: "b", label: "Reference (B)" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? {
            expr: `if(abs(${s.b.name}) > 0.001, (${s.a.name} - ${s.b.name}) / ${s.b.name} * 100, 0)`,
            name: `${s.a.name}_slip`,
            unit: "%",
          }
        : null,
  },
  {
    id: "lp",
    label: "Low-Pass Smooth",
    desc: "One-pole exponential smoothing filter",
    signals: [{ key: "a", label: "Signal" }],
    params: [{ key: "alpha", label: "Alpha (0..1)", defaultValue: "0.2", step: "0.05" }],
    build: (s, p) =>
      s.a
        ? { expr: `lp(${s.a.name}, ${numParam(p, "alpha", "0.2")})`, name: `${s.a.name}_lp`, unit: s.a.unit }
        : null,
  },
  {
    id: "sma",
    label: "Moving Average",
    desc: "Rolling mean over last N samples",
    signals: [{ key: "a", label: "Signal" }],
    params: [{ key: "n", label: "Samples (N)", defaultValue: "10", step: "1" }],
    build: (s, p) =>
      s.a
        ? { expr: `sma(${s.a.name}, ${numParam(p, "n", "10")})`, name: `${s.a.name}_sma`, unit: s.a.unit }
        : null,
  },
  {
    id: "deriv",
    label: "Rate of Change (d/dt)",
    desc: "Time derivative in signal units per second",
    signals: [{ key: "a", label: "Signal" }],
    params: [],
    build: (s) =>
      s.a
        ? {
            expr: `deriv(${s.a.name})`,
            name: `${s.a.name}_rate`,
            unit: s.a.unit ? `${s.a.unit}/s` : "/s",
          }
        : null,
  },
  {
    id: "delta",
    label: "Sample Delta (Δ)",
    desc: "Difference from previous sample",
    signals: [{ key: "a", label: "Signal" }],
    params: [],
    build: (s) =>
      s.a ? { expr: `delta(${s.a.name})`, name: `${s.a.name}_delta`, unit: s.a.unit } : null,
  },
  {
    id: "integ",
    label: "Time Integral (∫ dt)",
    desc: "Accumulated area under the signal over time",
    signals: [{ key: "a", label: "Signal" }],
    params: [],
    build: (s) =>
      s.a
        ? {
            expr: `integ(${s.a.name})`,
            name: `${s.a.name}_int`,
            unit: s.a.unit ? `${s.a.unit}·s` : "s",
          }
        : null,
  },
  {
    id: "clamp",
    label: "Clamp Range",
    desc: "Limit signal between min and max bounds",
    signals: [{ key: "a", label: "Signal" }],
    params: [
      { key: "lo", label: "Min", defaultValue: "0", step: "any" },
      { key: "hi", label: "Max", defaultValue: "100", step: "any" },
    ],
    build: (s, p) =>
      s.a
        ? {
            expr: `clamp(${s.a.name}, ${numParam(p, "lo", "0")}, ${numParam(p, "hi", "100")})`,
            name: `${s.a.name}_clamped`,
            unit: s.a.unit,
          }
        : null,
  },
  {
    id: "hypot",
    label: "Vector Magnitude",
    desc: "Combined magnitude sqrt(A² + B²)",
    signals: [
      { key: "a", label: "Axis X (A)" },
      { key: "b", label: "Axis Y (B)" },
    ],
    params: [],
    build: (s) =>
      s.a && s.b
        ? { expr: `hypot(${s.a.name}, ${s.b.name})`, name: `${s.a.name}_mag`, unit: s.a.unit || s.b.unit }
        : null,
  },
  {
    id: "flag",
    label: "Threshold Flag (0 / 1)",
    desc: "1 when condition holds, 0 otherwise — great for lamps",
    signals: [{ key: "a", label: "Signal" }],
    params: [
      { key: "op", label: "Op", defaultValue: ">", options: [">", ">=", "<", "<=", "==", "!="] },
      { key: "val", label: "Threshold", defaultValue: "0", step: "any" },
    ],
    build: (s, p) =>
      s.a
        ? {
            expr: `${s.a.name} ${numParam(p, "op", ">")} ${numParam(p, "val", "0")}`,
            name: `${s.a.name}_flag`,
            unit: "",
          }
        : null,
  },
  {
    id: "cond",
    label: "Conditional (If)",
    desc: "Choose between two values based on a condition",
    signals: [{ key: "a", label: "Signal" }],
    params: [
      { key: "op", label: "Op", defaultValue: ">", options: [">", ">=", "<", "<=", "==", "!="] },
      { key: "val", label: "Threshold", defaultValue: "0", step: "any" },
      { key: "thenV", label: "When true", defaultValue: "1", step: "any" },
      { key: "elseV", label: "When false", defaultValue: "0", step: "any" },
    ],
    build: (s, p) =>
      s.a
        ? {
            expr: `if(${s.a.name} ${numParam(p, "op", ">")} ${numParam(p, "val", "0")}, ${numParam(p, "thenV", "1")}, ${numParam(p, "elseV", "0")})`,
            name: `${s.a.name}_cond`,
            unit: "",
          }
        : null,
  },
];

type AstNode =
  | { kind: "num"; value: number }
  | { kind: "var"; name: string }
  | { kind: "neg"; inner: AstNode }
  | { kind: "not"; inner: AstNode }
  | { kind: "unary"; fn: string; inner: AstNode }
  | { kind: "bin"; op: string; left: AstNode; right: AstNode }
  | { kind: "variadic"; fn: "min" | "max" | "avg"; args: AstNode[] }
  | { kind: "clamp"; val: AstNode; lo: AstNode; hi: AstNode }
  | { kind: "if"; cond: AstNode; thenNode: AstNode; elseNode: AstNode }
  | { kind: "stateful"; fn: "lp" | "sma" | "delta" | "deriv" | "integ"; inner: AstNode };

export type ParseResult =
  | {
      ok: true;
      deps: string[];
      stateful: boolean;
      evaluate(values: ReadonlyMap<string, number>): number | null;
    }
  | { ok: false; error: string };

const UNARY_FNS = new Set([
  "abs",
  "sqrt",
  "round",
  "floor",
  "ceil",
  "sign",
  "sin",
  "cos",
  "tan",
  "exp",
  "ln",
  "log10",
]);

class Parser {
  private pos = 0;
  readonly deps: string[] = [];
  stateful = false;

  constructor(private readonly text: string) {}

  parse(): AstNode {
    const node = this.parseOr();
    this.skip();
    if (this.pos !== this.text.length) {
      throw new Error(`could not parse the rest of '${this.text}'`);
    }
    return node;
  }

  private parseOr(): AstNode {
    let node = this.parseAnd();
    while (true) {
      this.skip();
      if (this.peek2() === "||") {
        this.pos += 2;
        node = { kind: "bin", op: "||", left: node, right: this.parseAnd() };
      } else break;
    }
    return node;
  }

  private parseAnd(): AstNode {
    let node = this.parseCmp();
    while (true) {
      this.skip();
      if (this.peek2() === "&&") {
        this.pos += 2;
        node = { kind: "bin", op: "&&", left: node, right: this.parseCmp() };
      } else break;
    }
    return node;
  }

  private parseCmp(): AstNode {
    let node = this.parseExpr();
    while (true) {
      this.skip();
      const two = this.peek2();
      if (two === "==" || two === "!=" || two === "<=" || two === ">=") {
        this.pos += 2;
        node = { kind: "bin", op: two, left: node, right: this.parseExpr() };
      } else {
        const one = this.peek();
        if (one === "<" || one === ">") {
          this.pos += 1;
          node = { kind: "bin", op: one, left: node, right: this.parseExpr() };
        } else break;
      }
    }
    return node;
  }

  private parseExpr(): AstNode {
    let node = this.parseTerm();
    while (true) {
      this.skip();
      const ch = this.peek();
      if (ch === "+" || ch === "-") {
        this.pos += 1;
        node = { kind: "bin", op: ch, left: node, right: this.parseTerm() };
      } else break;
    }
    return node;
  }

  private parseTerm(): AstNode {
    let node = this.parseUnary();
    while (true) {
      this.skip();
      const ch = this.peek();
      if (ch === "*" || ch === "/" || ch === "%") {
        this.pos += 1;
        node = { kind: "bin", op: ch, left: node, right: this.parseUnary() };
      } else break;
    }
    return node;
  }

  private parseUnary(): AstNode {
    this.skip();
    const ch = this.peek();
    if (ch === "-") {
      this.pos += 1;
      return { kind: "neg", inner: this.parseUnary() };
    }
    if (ch === "+") {
      this.pos += 1;
      return this.parseUnary();
    }
    if (ch === "!" && this.peek2() !== "!=") {
      this.pos += 1;
      return { kind: "not", inner: this.parseUnary() };
    }
    return this.parsePow();
  }

  private parsePow(): AstNode {
    const base = this.parsePrimary();
    this.skip();
    if (this.peek() === "^") {
      this.pos += 1;
      return { kind: "bin", op: "^", left: base, right: this.parseUnary() };
    }
    return base;
  }

  private parsePrimary(): AstNode {
    this.skip();
    if (this.peek() === "(") {
      this.pos += 1;
      const node = this.parseOr();
      this.expectClose();
      return node;
    }
    const ch = this.peek();
    if (ch != null && ((ch >= "0" && ch <= "9") || ch === ".")) {
      return this.parseNumber();
    }
    const name = this.parseIdent();
    this.skip();
    if (this.peek() === "(") {
      this.pos += 1;
      return this.parseCall(name);
    }
    if (!this.deps.includes(name)) this.deps.push(name);
    return { kind: "var", name };
  }

  private parseCall(name: string): AstNode {
    if (UNARY_FNS.has(name)) {
      const inner = this.parseOr();
      this.expectClose();
      return { kind: "unary", fn: name, inner };
    }
    if (name === "pow" || name === "atan2" || name === "hypot") {
      const a = this.parseOr();
      this.expectComma(name);
      const b = this.parseOr();
      this.expectClose();
      return { kind: "bin", op: name, left: a, right: b };
    }
    if (name === "min" || name === "max" || name === "avg") {
      const args = [this.parseOr()];
      while (true) {
        this.skip();
        if (this.peek() === ",") {
          this.pos += 1;
          args.push(this.parseOr());
        } else break;
      }
      if (args.length < 2) throw new Error(`${name}() needs at least 2 arguments`);
      this.expectClose();
      return { kind: "variadic", fn: name, args };
    }
    if (name === "clamp") {
      const val = this.parseOr();
      this.expectComma("clamp");
      const lo = this.parseOr();
      this.expectComma("clamp");
      const hi = this.parseOr();
      this.expectClose();
      return { kind: "clamp", val, lo, hi };
    }
    if (name === "if") {
      const cond = this.parseOr();
      this.expectComma("if");
      const thenNode = this.parseOr();
      this.expectComma("if");
      const elseNode = this.parseOr();
      this.expectClose();
      return { kind: "if", cond, thenNode, elseNode };
    }
    if (name === "lp") {
      const inner = this.parseOr();
      this.skip();
      if (this.peek() !== ",") throw new Error("lp(signal, alpha) needs a comma");
      this.pos += 1;
      const alpha = this.parseConstNumber("lp alpha");
      if (alpha < 0 || alpha > 1) throw new Error("lp alpha must be between 0 and 1");
      this.expectClose();
      this.stateful = true;
      return { kind: "stateful", fn: "lp", inner };
    }
    if (name === "sma") {
      const inner = this.parseOr();
      this.expectComma("sma");
      const n = this.parseConstNumber("sma window");
      if (!Number.isInteger(n) || n < 1 || n > MAX_SMA_WINDOW) {
        throw new Error(`sma window must be an integer between 1 and ${MAX_SMA_WINDOW}`);
      }
      this.expectClose();
      this.stateful = true;
      return { kind: "stateful", fn: "sma", inner };
    }
    if (name === "delta" || name === "deriv" || name === "integ") {
      const inner = this.parseOr();
      this.expectClose();
      this.stateful = true;
      return { kind: "stateful", fn: name, inner };
    }
    throw new Error(`unknown function ${name}`);
  }

  private parseConstNumber(label: string): number {
    const node = this.parseUnary();
    if (node.kind === "num") return node.value;
    if (node.kind === "neg" && node.inner.kind === "num") return -node.inner.value;
    throw new Error(`${label} must be a number`);
  }

  private parseNumber(): AstNode {
    const start = this.pos;
    while (this.pos < this.text.length && /[0-9.]/.test(this.text[this.pos])) this.pos += 1;
    if (this.peek() === "e" || this.peek() === "E") {
      const save = this.pos;
      this.pos += 1;
      if (this.peek() === "+" || this.peek() === "-") this.pos += 1;
      const expStart = this.pos;
      while (this.pos < this.text.length && /[0-9]/.test(this.text[this.pos])) this.pos += 1;
      if (this.pos === expStart) this.pos = save;
    }
    const raw = this.text.slice(start, this.pos);
    const value = Number(raw);
    if (!Number.isFinite(value) || raw === "" || raw === ".") {
      throw new Error(`bad number '${raw}'`);
    }
    return { kind: "num", value };
  }

  private parseIdent(): string {
    const start = this.pos;
    const first = this.peek();
    if (first == null) throw new Error("math expression ended early");
    if (!/[A-Za-z_]/.test(first)) throw new Error("expected a signal name");
    this.pos += 1;
    while (this.pos < this.text.length && /[A-Za-z0-9_@]/.test(this.text[this.pos])) {
      this.pos += 1;
    }
    return this.text.slice(start, this.pos);
  }

  private expectComma(name: string): void {
    this.skip();
    if (this.peek() !== ",") throw new Error(`${name}() needs a comma`);
    this.pos += 1;
  }

  private expectClose(): void {
    this.skip();
    if (this.peek() !== ")") throw new Error("math expression is missing ')'");
    this.pos += 1;
  }

  private skip(): void {
    while (this.pos < this.text.length && /\s/.test(this.text[this.pos])) this.pos += 1;
  }

  private peek(): string | null {
    return this.pos < this.text.length ? this.text[this.pos] : null;
  }

  private peek2(): string | null {
    return this.pos + 1 < this.text.length ? this.text.slice(this.pos, this.pos + 2) : null;
  }
}

function evalNode(node: AstNode, values: ReadonlyMap<string, number>): number {
  switch (node.kind) {
    case "num":
      return node.value;
    case "var": {
      const v = values.get(node.name);
      return v == null ? Number.NaN : v;
    }
    case "neg":
      return -evalNode(node.inner, values);
    case "not": {
      const v = evalNode(node.inner, values);
      return Number.isFinite(v) ? (v === 0 ? 1 : 0) : Number.NaN;
    }
    case "unary": {
      const x = evalNode(node.inner, values);
      switch (node.fn) {
        case "abs": return Math.abs(x);
        case "sqrt": return Math.sqrt(x);
        case "round": return Math.round(x);
        case "floor": return Math.floor(x);
        case "ceil": return Math.ceil(x);
        case "sign": return Number.isFinite(x) ? (x > 0 ? 1 : x < 0 ? -1 : 0) : Number.NaN;
        case "sin": return Math.sin(x);
        case "cos": return Math.cos(x);
        case "tan": return Math.tan(x);
        case "exp": return Math.exp(x);
        case "ln": return Math.log(x);
        case "log10": return Math.log10(x);
        default: return Number.NaN;
      }
    }
    case "bin": {
      const a = evalNode(node.left, values);
      const b = evalNode(node.right, values);
      switch (node.op) {
        case "+": return a + b;
        case "-": return a - b;
        case "*": return a * b;
        case "/": return a / b;
        case "%": return a % b;
        case "^":
        case "pow": return Math.pow(a, b);
        case "atan2": return Math.atan2(a, b);
        case "hypot": return Math.hypot(a, b);
        case "<": return Number.isFinite(a) && Number.isFinite(b) ? (a < b ? 1 : 0) : Number.NaN;
        case "<=": return Number.isFinite(a) && Number.isFinite(b) ? (a <= b ? 1 : 0) : Number.NaN;
        case ">": return Number.isFinite(a) && Number.isFinite(b) ? (a > b ? 1 : 0) : Number.NaN;
        case ">=": return Number.isFinite(a) && Number.isFinite(b) ? (a >= b ? 1 : 0) : Number.NaN;
        case "==": return Number.isFinite(a) && Number.isFinite(b) ? (Math.abs(a - b) <= 1e-12 ? 1 : 0) : Number.NaN;
        case "!=": return Number.isFinite(a) && Number.isFinite(b) ? (Math.abs(a - b) > 1e-12 ? 1 : 0) : Number.NaN;
        case "&&": return Number.isFinite(a) && Number.isFinite(b) ? (a !== 0 && b !== 0 ? 1 : 0) : Number.NaN;
        case "||": return Number.isFinite(a) && Number.isFinite(b) ? (a !== 0 || b !== 0 ? 1 : 0) : Number.NaN;
        default: return Number.NaN;
      }
    }
    case "variadic": {
      let acc = evalNode(node.args[0], values);
      if (!Number.isFinite(acc)) return Number.NaN;
      for (let i = 1; i < node.args.length; i += 1) {
        const v = evalNode(node.args[i], values);
        if (!Number.isFinite(v)) return Number.NaN;
        if (node.fn === "min") acc = Math.min(acc, v);
        else if (node.fn === "max") acc = Math.max(acc, v);
        else acc += v;
      }
      return node.fn === "avg" ? acc / node.args.length : acc;
    }
    case "clamp": {
      const x = evalNode(node.val, values);
      const lo = evalNode(node.lo, values);
      const hi = evalNode(node.hi, values);
      if (!Number.isFinite(x) || !Number.isFinite(lo) || !Number.isFinite(hi)) return Number.NaN;
      const a = Math.min(lo, hi);
      const b = Math.max(lo, hi);
      return Math.min(b, Math.max(a, x));
    }
    case "if": {
      const c = evalNode(node.cond, values);
      if (!Number.isFinite(c)) return Number.NaN;
      return c !== 0 ? evalNode(node.thenNode, values) : evalNode(node.elseNode, values);
    }
    case "stateful": {
      const x = evalNode(node.inner, values);
      if (!Number.isFinite(x)) return Number.NaN;
      if (node.fn === "lp" || node.fn === "sma") return x;
      return 0;
    }
  }
}

export function parseMath(expr: string): ParseResult {
  const trimmed = expr.trim();
  if (!trimmed) return { ok: false, error: "Expression is empty" };
  try {
    const parser = new Parser(trimmed);
    const root = parser.parse();
    return {
      ok: true,
      deps: parser.deps,
      stateful: parser.stateful,
      evaluate(values) {
        const v = evalNode(root, values);
        return Number.isFinite(v) ? v : null;
      },
    };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

/** Identifier token immediately surrounding/before `cursor` in `text`, for inline autocomplete. */
export function tokenAtCursor(
  text: string,
  cursor: number,
): { token: string; start: number; end: number } {
  const safe = Math.max(0, Math.min(cursor, text.length));
  let start = safe;
  while (start > 0 && /[A-Za-z0-9_@]/.test(text[start - 1])) start -= 1;
  let end = safe;
  while (end < text.length && /[A-Za-z0-9_@]/.test(text[end])) end += 1;
  return { token: text.slice(start, safe), start, end };
}
