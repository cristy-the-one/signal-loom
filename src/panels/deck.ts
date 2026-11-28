import * as api from "../api";
import { els } from "../dom";
import { formatValue } from "../format";
import { MATH_FUNCTIONS, MATH_RECIPES, parseMath, tokenAtCursor, type MathRecipe } from "../math-eval";
import { hasData } from "../model";
import { adoptEdited } from "../project";
import { refresh, refreshOverview, setPlotted } from "../query";
import { draw, renderChrome, state } from "../state";
import { serialDeck, setError, withBusy } from "../status";
import type { MathChannel, SignalInfo, ThresholdTrigger } from "../types";
import { railText, renderRailList } from "./rail-list";

const OPERATORS = [
  { label: "+", insert: " + " },
  { label: "−", insert: " - " },
  { label: "×", insert: " * " },
  { label: "÷", insert: " / " },
  { label: "^", insert: " ^ " },
  { label: "( )", insert: "(|)" },
  { label: ">", insert: " > " },
  { label: "<", insert: " < " },
  { label: "==", insert: " == " },
  { label: "&&", insert: " && " },
  { label: "||", insert: " || " },
];

/** Channel name currently being edited (`null` when creating a new channel). */
let editingOriginalName: string | null = null;
let autoFilledName = "";
let autoFilledUnit = "";
let acItems: { insert: string; label: string; meta: string; cursorBack?: number }[] = [];
let acIndex = 0;

function physicalSignals(): SignalInfo[] {
  return (state.summary?.signals ?? [])
    .filter((sig) => hasData(sig) && sig.messageName.toLowerCase() !== "math")
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
}

export function renderMath(): void {
  const host = els.mathList;
  host.replaceChildren();
  if (state.math.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "No derived channels yet. Click + New or pick a recipe above.";
    host.append(note);
    return;
  }

  for (const channel of state.math) {
    const plotted = state.plotted.includes(channel.name);
    const wrap = document.createElement("div");
    wrap.className = plotted ? "math-card is-plotted" : "math-card";

    const main = document.createElement("button");
    main.type = "button";
    main.className = "math-card-main";
    main.title = plotted ? `${channel.name} is plotted — click to hide` : `Click to plot ${channel.name}`;
    main.addEventListener("click", () => {
      if (state.plotted.includes(channel.name)) {
        setPlotted(state.plotted.filter((n) => n !== channel.name));
      } else {
        setPlotted([...state.plotted, channel.name]);
      }
      state.dirty = true;
      renderChrome();
      draw();
      void refresh();
      void refreshOverview();
    });

    const head = document.createElement("div");
    head.className = "math-card-head";
    const dot = document.createElement("span");
    dot.className = plotted ? "math-dot is-on" : "math-dot";
    const nameEl = document.createElement("span");
    nameEl.className = "math-card-name";
    nameEl.textContent = channel.name;
    head.append(dot, nameEl);
    if (channel.unit) {
      const unitEl = document.createElement("span");
      unitEl.className = "math-card-unit";
      unitEl.textContent = channel.unit;
      head.append(unitEl);
    }

    const exprEl = document.createElement("div");
    exprEl.className = "math-card-expr";
    exprEl.textContent = channel.expr;

    main.append(head, exprEl);

    const actions = document.createElement("div");
    actions.className = "math-card-actions";

    const editBtn = document.createElement("button");
    editBtn.type = "button";
    editBtn.className = "mark-x math-edit-btn";
    editBtn.textContent = "✎";
    editBtn.title = `Edit ${channel.name}`;
    editBtn.setAttribute("aria-label", `Edit ${channel.name}`);
    editBtn.addEventListener("click", () => openMathEditor(channel));

    const removeBtn = document.createElement("button");
    removeBtn.type = "button";
    removeBtn.className = "mark-x";
    removeBtn.textContent = "×";
    removeBtn.title = `Remove ${channel.name}`;
    removeBtn.setAttribute("aria-label", `Remove ${channel.name}`);
    removeBtn.addEventListener("click", () => void removeMath(channel.name));

    actions.append(editBtn, removeBtn);
    wrap.append(main, actions);
    host.append(wrap);
  }
}

export function renderTriggers(): void {
  renderRailList(els.trigList, state.triggers, "Thresholds land on the event lane", (trigger) => {
    const label = railText("mark-l", `${trigger.signal} ${trigger.op} ${trigger.value}`);
    label.style.flex = "1";
    label.style.padding = "3px 4px";
    return {
      main: label,
      removeLabel: `Remove ${trigger.signal} ${trigger.op} ${trigger.value}`,
      onRemove: () => void removeTrigger(trigger.id),
    };
  });
}

function insertAtCaret(snippet: string): void {
  const input = els.mathExpr;
  const start = input.selectionStart ?? input.value.length;
  const end = input.selectionEnd ?? start;
  const before = input.value.slice(0, start);
  const after = input.value.slice(end);
  const pipe = snippet.indexOf("|");
  const clean = pipe >= 0 ? snippet.slice(0, pipe) + snippet.slice(pipe + 1) : snippet;

  // Add subtle space if inserting an identifier right next to another identifier/number
  const needsLeadSpace =
    /^[A-Za-z_]/.test(clean) && before.length > 0 && /[A-Za-z0-9_@)]$/.test(before);
  const prefix = needsLeadSpace ? " " : "";
  input.value = before + prefix + clean + after;

  const caret = start + prefix.length + (pipe >= 0 ? pipe : clean.length);
  input.focus();
  input.setSelectionRange(caret, caret);
  closeAutocomplete();
  updateEditorPreview();
}

function renderEditorSignals(): void {
  const host = els.mathSigList;
  host.replaceChildren();
  const q = els.mathSigFilter.value.trim().toLowerCase();
  const heldMap = new Map(state.held.map((h) => [h.name, h.value]));
  const signals = physicalSignals();

  let count = 0;
  for (const sig of signals) {
    const hay = `${sig.name} ${sig.unit} ${sig.messageName}`.toLowerCase();
    if (q && !hay.includes(q)) continue;
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "math-sig-item";
    btn.title = `Click to insert ${sig.name} into formula`;
    const top = document.createElement("span");
    top.className = "math-sig-top";
    const nameEl = document.createElement("span");
    nameEl.className = "math-sig-name";
    nameEl.textContent = sig.name;
    const valEl = document.createElement("span");
    valEl.className = "math-sig-val";
    const cur = heldMap.get(sig.name);
    valEl.textContent = cur != null ? `${formatValue(cur)}${sig.unit ? ` ${sig.unit}` : ""}` : sig.unit;
    top.append(nameEl, valEl);

    const meta = document.createElement("span");
    meta.className = "math-sig-meta";
    meta.textContent = sig.messageName || "Signal";

    btn.append(top, meta);
    btn.addEventListener("click", () => insertAtCaret(sig.name));
    host.append(btn);
    count += 1;
    if (count >= 120) break;
  }

  if (count === 0) {
    const empty = document.createElement("div");
    empty.className = "picker-empty";
    empty.textContent = signals.length === 0 ? "Open a log to pick signals" : "No matching signals";
    host.append(empty);
  }
}

function renderRecipeControls(recipe: MathRecipe | null): void {
  const host = els.mathRecipeFields;
  host.replaceChildren();
  if (!recipe) {
    host.hidden = true;
    return;
  }
  host.hidden = false;
  const signals = physicalSignals();
  const byName = new Map(signals.map((s) => [s.name, s]));

  const selectedSigs: Record<string, string> = {};
  const paramVals: Record<string, string> = {};

  const applyRecipe = () => {
    const sigInfos: Record<string, SignalInfo | undefined> = {};
    for (const f of recipe.signals) sigInfos[f.key] = byName.get(selectedSigs[f.key]);
    const built = recipe.build(sigInfos, paramVals);
    if (!built) return;
    els.mathExpr.value = built.expr;
    const curName = els.mathName.value.trim();
    if (!editingOriginalName && (!curName || curName === autoFilledName)) {
      els.mathName.value = built.name.slice(0, 64);
      autoFilledName = els.mathName.value;
    }
    const curUnit = els.mathUnit.value.trim();
    if (!editingOriginalName && (!curUnit || curUnit === autoFilledUnit)) {
      els.mathUnit.value = built.unit.slice(0, 16);
      autoFilledUnit = els.mathUnit.value;
    }
    updateEditorPreview();
  };

  // Pre-seed sensible defaults from plotted signals or first physical signals
  const defaultPool = [
    ...state.plotted.filter((n) => byName.has(n)),
    ...signals.map((s) => s.name).filter((n) => !state.plotted.includes(n)),
  ];

  recipe.signals.forEach((field, idx) => {
    const label = document.createElement("label");
    label.className = "recipe-field";
    const span = document.createElement("span");
    span.textContent = field.label;
    const select = document.createElement("select");
    for (const sig of signals) {
      const opt = document.createElement("option");
      opt.value = sig.name;
      opt.textContent = sig.unit ? `${sig.name} (${sig.unit})` : sig.name;
      select.append(opt);
    }
    const initial = defaultPool[idx] ?? defaultPool[0] ?? "";
    select.value = initial;
    selectedSigs[field.key] = select.value;
    select.addEventListener("change", () => {
      selectedSigs[field.key] = select.value;
      applyRecipe();
    });
    label.append(span, select);
    host.append(label);
  });

  recipe.params.forEach((field) => {
    const label = document.createElement("label");
    label.className = "recipe-field is-param";
    const span = document.createElement("span");
    span.textContent = field.label;
    paramVals[field.key] = field.defaultValue;

    if (field.options) {
      const select = document.createElement("select");
      for (const optVal of field.options) {
        const opt = document.createElement("option");
        opt.value = optVal;
        opt.textContent = optVal;
        select.append(opt);
      }
      select.value = field.defaultValue;
      select.addEventListener("change", () => {
        paramVals[field.key] = select.value;
        applyRecipe();
      });
      label.append(span, select);
    } else {
      const input = document.createElement("input");
      input.type = "number";
      input.step = field.step ?? "any";
      input.value = field.defaultValue;
      input.addEventListener("input", () => {
        paramVals[field.key] = input.value;
        applyRecipe();
      });
      label.append(span, input);
    }
    host.append(label);
  });

  if (signals.length > 0) applyRecipe();
}

function updateEditorPreview(): void {
  const expr = els.mathExpr.value.trim();
  const statusEl = els.mathPreview;
  if (!expr) {
    statusEl.className = "math-preview";
    statusEl.textContent = "Pick a recipe above, click signals on the right, or type a formula.";
    return;
  }

  const parsed = parseMath(expr);
  if (!parsed.ok) {
    statusEl.className = "math-preview is-err";
    statusEl.textContent = `Syntax: ${parsed.error}`;
    return;
  }

  const physNames = new Set(physicalSignals().map((s) => s.name));
  const mathNames = new Set(state.math.map((m) => m.name));
  const mathRefs = parsed.deps.filter((d) => mathNames.has(d) && !physNames.has(d));
  if (mathRefs.length > 0) {
    statusEl.className = "math-preview is-err";
    statusEl.textContent = `Cannot reference math channel ${mathRefs[0]} — use logged signals only.`;
    return;
  }

  const unknown = parsed.deps.filter((d) => !physNames.has(d));
  if (unknown.length > 0 && state.summary) {
    statusEl.className = "math-preview is-warn";
    statusEl.textContent = `Unknown signal${unknown.length > 1 ? "s" : ""}: ${unknown.join(", ")}`;
    return;
  }

  const heldMap = new Map(state.held.map((h) => [h.name, h.value]));
  const sample = parsed.evaluate(heldMap);
  const unit = els.mathUnit.value.trim();
  const valStr =
    sample == null
      ? "—"
      : `${formatValue(sample)}${unit ? ` ${unit}` : ""}`;
  const depsNote = parsed.deps.length ? `Uses: ${parsed.deps.join(", ")}` : "Constant";
  const filterNote = parsed.stateful ? " · series filter (full value on plot)" : "";
  statusEl.className = "math-preview is-ok";
  statusEl.textContent = `✓ Playhead preview: ${valStr}  ·  ${depsNote}${filterNote}`;
}

function closeAutocomplete(): void {
  acItems = [];
  acIndex = 0;
  els.mathAc.hidden = true;
}

function applyAutocomplete(index: number): void {
  const item = acItems[index];
  if (!item) return;
  const input = els.mathExpr;
  const { start, end } = tokenAtCursor(input.value, input.selectionStart ?? input.value.length);
  const before = input.value.slice(0, start);
  const after = input.value.slice(end);
  input.value = before + item.insert + after;
  const caret = start + item.insert.length - (item.cursorBack ?? 0);
  input.focus();
  input.setSelectionRange(caret, caret);
  closeAutocomplete();
  updateEditorPreview();
}

function renderAutocomplete(): void {
  const host = els.mathAc;
  if (acItems.length === 0) {
    host.hidden = true;
    return;
  }
  host.replaceChildren(
    ...acItems.map((item, idx) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = idx === acIndex ? "ac-item is-active" : "ac-item";
      const lbl = document.createElement("span");
      lbl.className = "ac-name";
      lbl.textContent = item.label;
      const meta = document.createElement("span");
      meta.className = "ac-meta";
      meta.textContent = item.meta;
      btn.append(lbl, meta);
      btn.addEventListener("mousedown", (event) => {
        event.preventDefault();
        applyAutocomplete(idx);
      });
      return btn;
    }),
  );
  host.hidden = false;
}

function updateAutocomplete(forceAll = false): void {
  const input = els.mathExpr;
  const { token } = tokenAtCursor(input.value, input.selectionStart ?? input.value.length);
  const q = token.toLowerCase();
  if (!forceAll && q.length === 0) {
    closeAutocomplete();
    return;
  }
  const items: typeof acItems = [];
  for (const sig of physicalSignals()) {
    if (!q || sig.name.toLowerCase().includes(q)) {
      items.push({
        insert: sig.name,
        label: sig.name,
        meta: [sig.unit, sig.messageName].filter(Boolean).join(" · "),
      });
      if (items.length >= 10) break;
    }
  }
  for (const fn of MATH_FUNCTIONS) {
    if (!q || fn.name.includes(q)) {
      const pipe = fn.snippet.indexOf("|");
      const clean = pipe >= 0 ? fn.snippet.slice(0, pipe) + fn.snippet.slice(pipe + 1) : fn.snippet;
      const cursorBack = pipe >= 0 ? clean.length - pipe : 0;
      items.push({
        insert: clean,
        label: fn.signature,
        meta: fn.desc,
        cursorBack,
      });
      if (items.length >= 16) break;
    }
  }
  // Hide if the only match is an exact match already typed
  if (items.length === 1 && items[0].insert === token) {
    closeAutocomplete();
    return;
  }
  acItems = items;
  acIndex = 0;
  renderAutocomplete();
}

export function openMathEditor(existing?: MathChannel | null, presetRecipeId?: string): void {
  editingOriginalName = existing ? existing.name : null;
  autoFilledName = "";
  autoFilledUnit = "";
  els.mathDialogTitle.textContent = existing ? `Edit "${existing.name}"` : "New Math Channel";
  els.mathSubmit.textContent = existing ? "Save changes" : "Add channel";
  els.mathName.value = existing?.name ?? "";
  els.mathUnit.value = existing?.unit ?? "";
  els.mathExpr.value = existing?.expr ?? "";
  els.mathSigFilter.value = "";
  els.mathRecipe.value = presetRecipeId ?? "";
  closeAutocomplete();

  renderEditorSignals();
  const recipe = presetRecipeId ? MATH_RECIPES.find((r) => r.id === presetRecipeId) ?? null : null;
  renderRecipeControls(recipe);
  updateEditorPreview();

  if (!els.mathDialog.open) els.mathDialog.showModal();
  if (existing) els.mathExpr.focus();
  else if (!presetRecipeId) els.mathName.focus();
}

async function saveMathFromEditor(): Promise<void> {
  const name = els.mathName.value.trim();
  const expr = els.mathExpr.value.trim();
  const unit = els.mathUnit.value.trim();
  if (!name || !expr) {
    els.mathPreview.className = "math-preview is-err";
    els.mathPreview.textContent = "A math channel needs both a name and an expression.";
    return;
  }
  const parsed = parseMath(expr);
  if (!parsed.ok) {
    els.mathPreview.className = "math-preview is-err";
    els.mathPreview.textContent = `Syntax: ${parsed.error}`;
    return;
  }

  const prevName = editingOriginalName;
  const duplicate = state.math.some((c) => c.name === name && c.name !== prevName);
  if (duplicate) {
    els.mathPreview.className = "math-preview is-err";
    els.mathPreview.textContent = `A math channel named "${name}" already exists.`;
    return;
  }

  let saved = false;
  await serialDeck(() =>
    withBusy("Compiling math", async () => {
      let next: MathChannel[];
      if (prevName != null) {
        const idx = state.math.findIndex((c) => c.name === prevName);
        if (idx >= 0) {
          next = state.math.map((c, i) => (i === idx ? { name, unit, expr } : c));
        } else {
          next = [...state.math, { name, unit, expr }];
        }
      } else {
        next = [...state.math, { name, unit, expr }];
      }

      const summary = await api.setMath(next);
      state.math = next;

      // Preserve plot state, lane overrides, style overrides, and cluster bindings on rename
      if (prevName != null && prevName !== name) {
        if (state.plotted.includes(prevName)) {
          setPlotted(state.plotted.map((n) => (n === prevName ? name : n)));
        }
        if (state.solo === prevName) state.solo = name;
        if (prevName in state.signalLanes) {
          const lanes = { ...state.signalLanes };
          lanes[name] = lanes[prevName];
          delete lanes[prevName];
          state.signalLanes = lanes;
        }
        if (prevName in state.signalStyles) {
          const styles = { ...state.signalStyles };
          styles[name] = styles[prevName];
          delete styles[prevName];
          state.signalStyles = styles;
        }
        if (state.clusterWidgets.some((w) => w.signal === prevName)) {
          state.clusterWidgets = state.clusterWidgets.map((w) =>
            w.signal === prevName ? { ...w, signal: name } : w,
          );
        }
      } else if (prevName == null && !state.plotted.includes(name)) {
        setPlotted([...state.plotted, name]);
      }

      adoptEdited(summary);
      saved = true;
    }),
  );

  if (saved) els.mathDialog.close();
}

async function removeMath(name: string): Promise<void> {
  await serialDeck(() =>
    withBusy("Compiling math", async () => {
      const next = state.math.filter((channel) => channel.name !== name);
      const summary = await api.setMath(next);
      state.math = next;
      setPlotted(state.plotted.filter((plotted) => plotted !== name));
      adoptEdited(summary);
    }),
  );
}

async function syncTriggers(change: (current: ThresholdTrigger[]) => ThresholdTrigger[]): Promise<void> {
  await serialDeck(() =>
    withBusy("Arming triggers", async () => {
      const next = change(state.triggers);
      const summary = await api.setTriggers(next);
      state.triggers = next;
      adoptEdited(summary);
    }),
  );
}

async function addTrigger(): Promise<void> {
  const signal = els.trigSignal.value.trim();
  const op = els.trigOp.value;
  const value = Number(els.trigValue.value);
  if (!signal || !Number.isFinite(value)) {
    setError("A trigger needs a signal and a finite level.", "action");
    return;
  }
  const trigger: ThresholdTrigger = { id: `t-${Date.now().toString(36)}`, signal, op, value };
  await syncTriggers((current) => [...current, trigger]);
}

async function removeTrigger(id: string): Promise<void> {
  await syncTriggers((current) => current.filter((item) => item.id !== id));
}

async function applyTimeoutFactor(): Promise<void> {
  const factor = Number(els.timeoutFactor.value);
  const current = state.summary?.timeoutFactor;
  if (!state.summary || factor === current) return;
  if (!Number.isFinite(factor) || factor < 1 || factor > 100) {
    setError("The timeout must be between 1 and 100 cycle times.", "action");
    els.timeoutFactor.value = String(current);
    return;
  }
  await serialDeck(async () => {
    await withBusy("Re-indexing timeouts", async () => {
      adoptEdited(await api.setTimeoutFactor(factor));
    });
    // A refused value leaves the field showing the factor in effect.
    els.timeoutFactor.value = String(state.summary?.timeoutFactor ?? current);
  });
}

function initMathStudioControls(): void {
  // Populate recipes dropdown
  const blankOpt = document.createElement("option");
  blankOpt.value = "";
  blankOpt.textContent = "Choose a formula recipe (optional)…";
  els.mathRecipe.replaceChildren(
    blankOpt,
    ...MATH_RECIPES.map((r) => {
      const opt = document.createElement("option");
      opt.value = r.id;
      opt.textContent = `${r.label} — ${r.desc}`;
      return opt;
    }),
  );

  // Operator buttons
  els.mathOps.replaceChildren(
    ...OPERATORS.map((op) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "chip-btn";
      btn.textContent = op.label;
      btn.addEventListener("click", () => insertAtCaret(op.insert));
      return btn;
    }),
  );

  // Function buttons
  els.mathFns.replaceChildren(
    ...MATH_FUNCTIONS.map((fn) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = `chip-btn is-${fn.category}`;
      btn.textContent = fn.name;
      btn.title = `${fn.signature} — ${fn.desc}`;
      btn.addEventListener("click", () => insertAtCaret(fn.snippet));
      return btn;
    }),
  );
}

export function bindDeck(): void {
  initMathStudioControls();

  els.btnMathNew.addEventListener("click", () => openMathEditor(null));
  els.mathQuickDiff.addEventListener("click", () => openMathEditor(null, "diff"));
  els.mathQuickLp.addEventListener("click", () => openMathEditor(null, "lp"));
  els.mathQuickDeriv.addEventListener("click", () => openMathEditor(null, "deriv"));
  els.mathQuickScale.addEventListener("click", () => openMathEditor(null, "scale"));

  els.mathClose.addEventListener("click", () => els.mathDialog.close());
  els.mathCancel.addEventListener("click", () => els.mathDialog.close());

  els.mathRecipe.addEventListener("change", () => {
    const recipe = MATH_RECIPES.find((r) => r.id === els.mathRecipe.value) ?? null;
    renderRecipeControls(recipe);
  });

  els.mathSigFilter.addEventListener("input", () => renderEditorSignals());
  els.mathUnit.addEventListener("input", () => updateEditorPreview());
  els.mathExpr.addEventListener("input", () => {
    updateEditorPreview();
    updateAutocomplete(false);
  });
  els.mathExpr.addEventListener("blur", () => {
    window.setTimeout(closeAutocomplete, 120);
  });
  els.mathExpr.addEventListener("keydown", (event) => {
    if (event.ctrlKey && event.code === "Space") {
      event.preventDefault();
      updateAutocomplete(true);
      return;
    }
    if (!els.mathAc.hidden && acItems.length > 0) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        acIndex = (acIndex + 1) % acItems.length;
        renderAutocomplete();
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        acIndex = (acIndex - 1 + acItems.length) % acItems.length;
        renderAutocomplete();
        return;
      }
      if (event.key === "Tab" || event.key === "Enter") {
        event.preventDefault();
        applyAutocomplete(acIndex);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        closeAutocomplete();
        return;
      }
    }
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      void saveMathFromEditor();
    }
  });

  els.mathForm.addEventListener("submit", (event) => {
    event.preventDefault();
    void saveMathFromEditor();
  });
  els.trigForm.addEventListener("submit", (event) => {
    event.preventDefault();
    void addTrigger();
  });
  els.timeoutFactor.addEventListener("change", () => void applyTimeoutFactor());
}
