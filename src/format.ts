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
