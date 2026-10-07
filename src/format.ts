export function formatUs(us: number): string {
  const neg = us < 0;
  let rest = Math.abs(Math.round(us));
  const min = Math.floor(rest / 60_000_000);
  rest %= 60_000_000;
  const sec = Math.floor(rest / 1_000_000);
  const ms = Math.floor((rest % 1_000_000) / 1000);
  const body =
    min > 0
      ? `${min}:${sec.toString().padStart(2, "0")}.${ms.toString().padStart(3, "0")}`
      : `${sec}.${ms.toString().padStart(3, "0")}`;
  return neg ? `-${body}` : body;
}

export function formatSpan(us: number): string {
  if (us >= 1_000_000) {
    const digits = us >= 10_000_000 ? 1 : 2;
    return `${(us / 1_000_000).toFixed(digits)} s`;
  }
  if (us >= 1_000) return `${(us / 1_000).toFixed(0)} ms`;
  return `${Math.round(us)} µs`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatValue(v: number): string {
  if (!Number.isFinite(v)) return "—";
  if (Math.abs(v - Math.round(v)) < 1e-6 && Math.abs(v) < 1_000_000) {
    return String(Math.round(v));
  }
  const mag = Math.abs(v);
  if (mag >= 100) return v.toFixed(1);
  if (mag >= 10) return v.toFixed(2);
  return v.toFixed(3);
}

/** How one signal's live value is printed. Fixed per signal, so a reading never changes width. */
export interface Readout {
  decimals: number;
  width: number;
}

const READOUT_DIGITS = 4;

/**
 * Decimals come from the decoder step (0.1 shows one), else from the range,
 * capped at four significant digits. The width fits the widest value in the log.
 */
export function readoutFor(signal: { min: number | null; max: number | null; step: number | null } | undefined): Readout {
  const lo = signal?.min ?? 0;
  const hi = signal?.max ?? 0;
  const mag = Math.max(Math.abs(lo), Math.abs(hi));
  const intDigits = mag >= 1 ? Math.floor(Math.log10(mag)) + 1 : 1;
  const wanted = signal?.step ? stepDecimals(signal.step) : mag >= 100 ? 1 : mag >= 10 ? 2 : 3;
  const decimals = Math.min(wanted, Math.max(0, READOUT_DIGITS - intDigits));
  const sized = { decimals, width: 0 };
  // Math channels have no range yet: leave room for a sign and three whole digits.
  const floor = signal?.min == null || signal?.max == null ? 5 + decimals : 0;
  return {
    decimals,
    width: Math.max(floor, formatReading(lo, sized).length, formatReading(hi, sized).length),
  };
}

/** A value in its signal's readout, left-padded so it keeps its width in a monospace font. */
export function formatReading(value: number, readout: Readout): string {
  if (!Number.isFinite(value)) return "—".padStart(readout.width);
  const text = value.toFixed(readout.decimals);
  return (/^-0(\.0+)?$/.test(text) ? text.slice(1) : text).padStart(readout.width);
}

function stepDecimals(step: number): number {
  for (let decimals = 0; decimals < 6; decimals++) {
    const scaled = step * 10 ** decimals;
    if (Math.abs(scaled - Math.round(scaled)) < 1e-6 * Math.max(1, scaled)) return decimals;
  }
  return 6;
}

export function formatCount(n: number): string {
  return new Intl.NumberFormat("en-US").format(n);
}

export function hexId(id: number): string {
  return `0x${id.toString(16).toUpperCase()}`;
}

export function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

export function clamp(value: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, value));
}

export function errText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Something went wrong";
}

const TICKS = [
  1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000, 200_000, 500_000, 1_000_000, 2_000_000,
  5_000_000, 10_000_000, 15_000_000, 30_000_000, 60_000_000,
];

export function tickStep(span: number): number {
  const target = span / 6;
  for (const step of TICKS) {
    if (step >= target) return step;
  }
  return 60_000_000;
}
