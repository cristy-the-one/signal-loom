import { els } from "./dom";
import { fuzzyMatch } from "./fuzzy";

export type { FuzzyMatch } from "./fuzzy";
export { fuzzyMatch } from "./fuzzy";

export interface CommandItem {
  id: string;
  title: string;
  category: string;
  shortcut?: string[];
  keywords?: string;
  active?: boolean;
  run(): void;
}

let provider: (() => CommandItem[]) | null = null;
let filtered: { item: CommandItem; titleIndices: number[] }[] = [];
let activeIdx = 0;

function appendHighlighted(parent: HTMLElement, text: string, indices: number[]): void {
  if (indices.length === 0) {
    parent.textContent = text;
    return;
  }
  const set = new Set(indices);
  let buf = "";
  let inMark = false;
  const flush = () => {
    if (!buf) return;
    if (inMark) {
      const mark = document.createElement("mark");
      mark.textContent = buf;
      parent.append(mark);
    } else {
      parent.append(document.createTextNode(buf));
    }
    buf = "";
  };
  for (let i = 0; i < text.length; i += 1) {
    const hit = set.has(i);
    if (hit !== inMark) {
      flush();
      inMark = hit;
    }
    buf += text[i];
  }
  flush();
}

function renderList(): void {
  const q = els.paletteInput.value.trim();
  const items = provider ? provider() : [];
  const scored: { item: CommandItem; score: number; titleIndices: number[]; order: number }[] = [];

  items.forEach((item, order) => {
    if (!q) {
      scored.push({ item, score: 0, titleIndices: [], order });
      return;
    }
    const titleHit = fuzzyMatch(q, item.title);
    const metaHit = fuzzyMatch(q, `${item.category} ${item.keywords ?? ""} ${(item.shortcut ?? []).join(" ")}`);
    if (!titleHit && !metaHit) return;
    const score = Math.max(titleHit ? titleHit.score + 8 : -Infinity, metaHit ? metaHit.score : -Infinity);
    scored.push({ item, score, titleIndices: titleHit?.indices ?? [], order });
  });

  if (q) {
    scored.sort((a, b) => b.score - a.score || a.order - b.order);
  }
  filtered = scored.slice(0, 60).map(({ item, titleIndices }) => ({ item, titleIndices }));
  activeIdx = Math.max(0, Math.min(activeIdx, Math.max(0, filtered.length - 1)));

  els.paletteList.replaceChildren();
  if (filtered.length === 0) {
    const empty = document.createElement("div");
    empty.className = "palette-empty";
    empty.textContent = "No matching commands or shortcuts";
    els.paletteList.append(empty);
    return;
  }

  let lastCategory = "";
  filtered.forEach((entry, idx) => {
    const { item, titleIndices } = entry;
    if (!q && item.category !== lastCategory) {
      lastCategory = item.category;
      const head = document.createElement("div");
      head.className = "palette-group";
      head.textContent = item.category;
      els.paletteList.append(head);
    }

    const row = document.createElement("button");
    row.type = "button";
    row.className = idx === activeIdx ? "palette-item is-active" : "palette-item";
    row.dataset.index = String(idx);

    const left = document.createElement("span");
    left.className = "palette-title";
    appendHighlighted(left, item.title, titleIndices);

    const right = document.createElement("span");
    right.className = "palette-right";
    if (q) {
      const cat = document.createElement("span");
      cat.className = "palette-cat";
      cat.textContent = item.category;
      right.append(cat);
    }
    if (item.shortcut?.length) {
      const keys = document.createElement("span");
      keys.className = "palette-keys";
      for (const k of item.shortcut) {
        const kbd = document.createElement("kbd");
        kbd.textContent = k;
        keys.append(kbd);
      }
      right.append(keys);
    }

    row.append(left, right);
    row.addEventListener("mouseenter", () => {
      if (activeIdx === idx) return;
      activeIdx = idx;
      syncActiveRow();
    });
    row.addEventListener("click", () => runIndex(idx));
    els.paletteList.append(row);
  });
}

function syncActiveRow(): void {
  const buttons = els.paletteList.querySelectorAll<HTMLButtonElement>(".palette-item");
  buttons.forEach((btn) => {
    const isOn = Number(btn.dataset.index) === activeIdx;
    btn.classList.toggle("is-active", isOn);
    if (isOn) btn.scrollIntoView({ block: "nearest" });
  });
}

function runIndex(idx: number): void {
  const entry = filtered[idx];
  if (!entry) return;
  closePalette();
  entry.item.run();
}

export function openPalette(initialQuery = ""): void {
  els.paletteInput.value = initialQuery;
  activeIdx = 0;
  renderList();
  if (!els.palette.open) els.palette.showModal();
  els.paletteInput.focus();
  els.paletteInput.select();
}

export function closePalette(): void {
  if (els.palette.open) els.palette.close();
}

export function bindPalette(getCommands: () => CommandItem[]): void {
  provider = getCommands;

  els.paletteInput.addEventListener("input", () => {
    activeIdx = 0;
    renderList();
  });

  els.paletteInput.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (filtered.length > 0) {
        activeIdx = (activeIdx + 1) % filtered.length;
        syncActiveRow();
      }
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (filtered.length > 0) {
        activeIdx = (activeIdx - 1 + filtered.length) % filtered.length;
        syncActiveRow();
      }
    } else if (event.key === "Enter") {
      event.preventDefault();
      runIndex(activeIdx);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closePalette();
    }
  });

  els.palette.addEventListener("click", (event) => {
    if (event.target === els.palette) closePalette();
  });
  els.paletteClose.addEventListener("click", () => closePalette());
}
