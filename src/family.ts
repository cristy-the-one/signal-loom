import { TRACE_COLORS } from "./plot";
import { AMBER, CORAL, CYAN, GREEN, LIME, PINK, RED, SKY, VIOLET, YELLOW } from "./theme";

export type Severity = "fault" | "warn" | "info";

/**
 * The terms a name or label can be matched by: its words, lowercased, and each pair of
 * neighbouring words run together. Words break at spaces, `_`, `.`, `@`, `-` and camelCase,
 * so `ABS_Wheels` gives `abs` and `wheels`, and `PackVoltage` also gives `packvoltage`.
 */
function termsOf(text: string): Set<string> {
  const words = text
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1 $2")
    .toLowerCase()
    .split(/[\s_.@-]+/)
    .filter(Boolean);
  const terms = new Set(words);
  for (let i = 0; i + 1 < words.length; i += 1) terms.add(words[i] + words[i + 1]);
  return terms;
}

/** True when a whole term matches a key. A key never matches inside a word: `turn` is not in `ReturnTemp`. */
function hasTerm(terms: Set<string>, keys: string[]): boolean {
  return keys.some((key) => terms.has(key));
}

/** Colour a signal by the ECU family it belongs to. */
export function familyColor(name: string, message = "", index = 0): string {
  const nameTerms = termsOf(name);
  const terms = new Set([...nameTerms, ...termsOf(message)]);
  if (message.toLowerCase() === "math") return LIME;
  if (hasTerm(terms, ["checksum", "dtc", "mil", "timeout", "missing"])) return CORAL;
  if (hasTerm(nameTerms, ["counter"])) return PINK;
  if (hasTerm(terms, ["bms", "soc", "packvoltage", "packcurrent", "celltemp", "fuel"])) return VIOLET;
  if (hasTerm(terms, ["door", "turn", "lamp", "telltale", "bcm"])) return GREEN;
  if (hasTerm(terms, ["abs", "esc", "brake", "wheel", "wheels", "steer", "steering", "latg", "yaw"])) return AMBER;
  if (hasTerm(terms, ["vehiclespeed", "displayed", "cluster", "ic"])) return YELLOW;
  if (hasTerm(terms, ["rpm", "torque", "throttle", "coolant", "oil", "gear", "pedal", "clutch", "ecm", "tcu"])) return CYAN;
  return TRACE_COLORS[Math.abs(index) % TRACE_COLORS.length];
}

export function severityOf(label: string): Severity {
  const terms = termsOf(label);
  if (hasTerm(terms, ["checksum", "busoff", "errorframe", "dtc", "timeout", "missing"])) return "fault";
  if (hasTerm(terms, ["counter", "abs", "trigger", "esc"])) return "warn";
  return "info";
}

export function severityColor(kind: Severity): string {
  if (kind === "fault") return RED;
  if (kind === "warn") return AMBER;
  return SKY;
}
