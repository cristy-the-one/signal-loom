export interface FuzzyMatch {
  score: number;
  indices: number[];
}

/**
 * Case-insensitive subsequence fuzzy matcher.
 * Rewards exact substrings, prefixes, consecutive matches, and word boundaries.
 */
export function fuzzyMatch(query: string, target: string): FuzzyMatch | null {
  const q = query.trim().toLowerCase();
  if (!q) return { score: 0, indices: [] };
  const t = target.toLowerCase();

  let qi = 0;
  let score = 0;
  let prev = -2;
  const indices: number[] = [];

  for (let ti = 0; ti < t.length && qi < q.length; ti += 1) {
    if (t[ti] !== q[qi]) continue;
    indices.push(ti);
    score += 4;
    if (ti === 0) score += 12;
    if (ti === prev + 1) score += 8;
    else if (ti > 0) {
      const ch = target[ti - 1];
      const cur = target[ti];
      const isBoundary =
        ch === " " ||
        ch === "_" ||
        ch === "-" ||
        ch === "/" ||
        ch === "." ||
        (ch >= "a" && ch <= "z" && cur >= "A" && cur <= "Z");
      if (isBoundary) score += 10;
    }
    prev = ti;
    qi += 1;
  }

  if (qi !== q.length) return null;
  if (t.includes(q)) score += 20;
  if (t.startsWith(q)) score += 15;
  score -= Math.min(10, (t.length - q.length) * 0.15);
  return { score, indices };
}
