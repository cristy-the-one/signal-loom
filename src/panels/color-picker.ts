import { els } from "../dom";
import { colorFor, defaultColorFor } from "../model";
import { renderDirty } from "../readout";
import { draw, state } from "../state";
import {
  AMBER,
  CORAL,
  CYAN,
  GREEN,
  LIME,
  ORANGE,
  PINK,
  RED,
  SKY,
  TEAL,
  VIOLET,
  YELLOW,
} from "../theme";
import { renderSignals } from "./signals";

export const SIGNAL_COLOR_PRESETS: readonly { name: string; hex: string }[] = [
  { name: "Cyan", hex: CYAN },
  { name: "Sky", hex: SKY },
  { name: "Teal", hex: TEAL },
  { name: "Green", hex: GREEN },
  { name: "Lime", hex: LIME },
  { name: "Yellow", hex: YELLOW },
  { name: "Amber", hex: AMBER },
  { name: "Orange", hex: ORANGE },
  { name: "Coral", hex: CORAL },
  { name: "Red", hex: RED },
  { name: "Pink", hex: PINK },
  { name: "Violet", hex: VIOLET },
  { name: "White", hex: "#e7eef3" },
  { name: "Silver", hex: "#9eb0be" },
];

let activeSignal: string | null = null;
let bound = false;

export function closeColorPicker(): void {
  activeSignal = null;
  els.colorMenu.hidden = true;
}

function ensureBound(): void {
  if (bound) return;
  bound = true;
  document.addEventListener("mousedown", (event) => {
    if (!els.colorMenu.hidden && !els.colorMenu.contains(event.target as Node)) {
      closeColorPicker();
    }
  });
  document.addEventListener("keydown", (event) => {
    if (els.colorMenu.hidden) return;
    if (event.key === "Escape" || event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      closeColorPicker();
    }
  });
}

function applySignalColor(signalName: string, hex: string | null): void {
  const next = { ...state.signalColors };
  if (!hex) {
    delete next[signalName];
  } else {
    next[signalName] = hex.toLowerCase();
  }
  state.signalColors = next;
  state.dirty = true;
  renderDirty();
  renderSignals();
  draw();
}

export function openColorPicker(signalName: string, anchor: HTMLElement): void {
  ensureBound();
  if (activeSignal === signalName && !els.colorMenu.hidden) {
    closeColorPicker();
    return;
  }
  activeSignal = signalName;
  renderMenu(signalName);

  const rect = anchor.getBoundingClientRect();
  const menuW = 224;
  const menuH = 152;
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - menuW - 8));
  const preferBelow = rect.bottom + 4;
  const top =
    preferBelow + menuH <= window.innerHeight - 8
      ? preferBelow
      : Math.max(8, rect.top - menuH - 4);
  els.colorMenu.style.left = `${Math.round(left)}px`;
  els.colorMenu.style.top = `${Math.round(top)}px`;
  els.colorMenu.hidden = false;
}

function renderMenu(signalName: string): void {
  const autoHex = defaultColorFor(signalName);
  const currentHex = colorFor(signalName);
  const isCustom = state.signalColors[signalName] != null;

  const head = document.createElement("div");
  head.className = "color-menu-head";

  const title = document.createElement("span");
  title.className = "color-menu-name";
  title.textContent = signalName;
  title.title = signalName;

  const resetBtn = document.createElement("button");
  resetBtn.type = "button";
  resetBtn.className = isCustom ? "color-auto-btn" : "color-auto-btn is-on";
  resetBtn.title = `Reset to automatic color (${autoHex})`;
  const autoDot = document.createElement("i");
  autoDot.style.background = autoHex;
  resetBtn.append(autoDot, document.createTextNode("Auto"));
  resetBtn.addEventListener("click", (event) => {
    event.stopPropagation();
    applySignalColor(signalName, null);
    closeColorPicker();
  });
  head.append(title, resetBtn);

  const grid = document.createElement("div");
  grid.className = "color-menu-grid";
  for (const preset of SIGNAL_COLOR_PRESETS) {
    const dot = document.createElement("button");
    dot.type = "button";
    const isOn = currentHex.toLowerCase() === preset.hex.toLowerCase();
    dot.className = isOn ? "color-dot is-on" : "color-dot";
    dot.style.background = preset.hex;
    dot.title = `${preset.name} (${preset.hex})`;
    dot.setAttribute("aria-label", preset.name);
    dot.addEventListener("click", (event) => {
      event.stopPropagation();
      applySignalColor(signalName, preset.hex);
      closeColorPicker();
    });
    grid.append(dot);
  }

  const foot = document.createElement("label");
  foot.className = "color-menu-custom";
  const customLabel = document.createElement("span");
  customLabel.textContent = "Custom";
  const picker = document.createElement("input");
  picker.type = "color";
  picker.value = /^#[0-9a-f]{6}$/i.test(currentHex) ? currentHex : "#3ec6ff";
  const hexOut = document.createElement("span");
  hexOut.className = "color-menu-hex";
  hexOut.textContent = picker.value.toUpperCase();
  picker.addEventListener("input", () => {
    hexOut.textContent = picker.value.toUpperCase();
    applySignalColor(signalName, picker.value);
    resetBtn.classList.remove("is-on");
    grid.querySelectorAll(".color-dot.is-on").forEach((el) => el.classList.remove("is-on"));
  });
  foot.append(customLabel, picker, hexOut);

  els.colorMenu.replaceChildren(head, grid, foot);
}
