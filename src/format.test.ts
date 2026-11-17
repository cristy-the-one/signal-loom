import { describe, expect, it } from "vitest";
import {
  basename,
  clamp,
  errText,
  formatBytes,
  formatCount,
  formatReading,
  formatSpan,
  formatUs,
  formatValue,
  hexId,
  readoutFor,
  tickStep,
} from "./format";

describe("formatUs", () => {
  it("prints zero and sub-millisecond times as 0.000", () => {
    expect(formatUs(0)).toBe("0.000");
    expect(formatUs(500)).toBe("0.000");
  });

  it("prints milliseconds, seconds, and padded minutes", () => {
    expect(formatUs(1_000)).toBe("0.001");
    expect(formatUs(1_500_000)).toBe("1.500");
    expect(formatUs(61_250_000)).toBe("1:01.250");
  });

  it("rounds to the nearest microsecond and keeps the sign", () => {
    expect(formatUs(999_999.5)).toBe("1.000");
    expect(formatUs(-1_500)).toBe("-0.001");
    expect(formatUs(-61_250_000)).toBe("-1:01.250");
  });
});

describe("formatSpan", () => {
  it("uses microseconds, then milliseconds, then seconds", () => {
    expect(formatSpan(0)).toBe("0 µs");
    expect(formatSpan(999.4)).toBe("999 µs");
    expect(formatSpan(1_000)).toBe("1 ms");
    expect(formatSpan(1_400)).toBe("1 ms");
    expect(formatSpan(1_500)).toBe("2 ms");
    expect(formatSpan(1_000_000)).toBe("1.00 s");
    expect(formatSpan(1_500_000)).toBe("1.50 s");
    expect(formatSpan(9_500_000)).toBe("9.50 s");
    expect(formatSpan(10_000_000)).toBe("10.0 s");
  });
});

describe("formatBytes", () => {
  it("switches at the kibibyte and mebibyte", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023.4)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(1.5 * 1024 * 1024)).toBe("1.5 MB");
  });
});

describe("formatValue", () => {
  it("replaces non-finite numbers", () => {
    expect(formatValue(Number.NaN)).toBe("—");
    expect(formatValue(Number.POSITIVE_INFINITY)).toBe("—");
    expect(formatValue(Number.NEGATIVE_INFINITY)).toBe("—");
  });

  it("prints near-integers without a decimal and scales the rest by magnitude", () => {
    expect(formatValue(0)).toBe("0");
    expect(formatValue(1.0000001)).toBe("1");
    expect(formatValue(3.14159)).toBe("3.142");
    expect(formatValue(-3.5)).toBe("-3.500");
    expect(formatValue(10)).toBe("10");
    expect(formatValue(10.126)).toBe("10.13");
    expect(formatValue(100.126)).toBe("100.1");
    expect(formatValue(1_000_000)).toBe("1000000.0");
  });
});

describe("readoutFor", () => {
  it("leaves room for a sign and three digits when the signal has no range", () => {
    expect(readoutFor(undefined)).toEqual({ decimals: 3, width: 8 });
    expect(readoutFor({ min: null, max: null, step: 0.01 })).toEqual({ decimals: 2, width: 7 });
  });

  it("takes decimals from the step and width from the wider endpoint", () => {
    expect(readoutFor({ min: 0, max: 10, step: 0.1 })).toEqual({ decimals: 1, width: 4 });
    expect(readoutFor({ min: -100, max: 50, step: null })).toEqual({ decimals: 1, width: 6 });
  });
});

describe("formatReading", () => {
  const readout = { decimals: 3, width: 8 };

  it("pads to the readout width", () => {
    expect(formatReading(1.2, readout)).toBe("   1.200");
  });

  it("collapses negative zero and pads a missing value", () => {
    expect(formatReading(-0.0004, readout)).toBe("   0.000");
    expect(formatReading(-0.4, { decimals: 0, width: 2 })).toBe(" 0");
    expect(formatReading(Number.NaN, readout)).toBe("       —");
  });
});

describe("formatCount", () => {
  it("groups digits for en-US", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(1_234_567)).toBe("1,234,567");
  });
});

describe("hexId", () => {
  it("prints an uppercase hex id", () => {
    expect(hexId(0)).toBe("0x0");
    expect(hexId(255)).toBe("0xFF");
    expect(hexId(291)).toBe("0x123");
  });
});

describe("basename", () => {
  it("keeps the last path segment for either separator", () => {
    expect(basename("a/b/c.slog")).toBe("c.slog");
    expect(basename("a\\b\\c.slog")).toBe("c.slog");
    expect(basename("file.slog")).toBe("file.slog");
    expect(basename("dir/")).toBe("dir/");
    expect(basename("")).toBe("");
  });
});

describe("clamp", () => {
  it("limits a value to the inclusive bounds", () => {
    expect(clamp(5, 0, 10)).toBe(5);
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
  });
});

describe("errText", () => {
  it("reads an Error or a string and replaces anything else", () => {
    expect(errText(new Error("nope"))).toBe("nope");
    expect(errText("plain")).toBe("plain");
    expect(errText("")).toBe("");
    expect(errText(3)).toBe("Something went wrong");
    expect(errText(null)).toBe("Something went wrong");
    expect(errText(undefined)).toBe("Something went wrong");
  });
});

describe("tickStep", () => {
  it("picks the first tick at least a sixth of the span, else one minute", () => {
    expect(tickStep(0)).toBe(1_000);
    expect(tickStep(6_000)).toBe(1_000);
    expect(tickStep(6_006)).toBe(2_000);
    expect(tickStep(360_000_000)).toBe(60_000_000);
    expect(tickStep(360_000_006)).toBe(60_000_000);
  });
});
