import * as api from "../api";
import { els } from "../dom";
import { adoptEdited } from "../project";
import { refresh, refreshOverview, setPlotted } from "../query";
import { draw, renderChrome, state } from "../state";
import { serialDeck, setError, withBusy } from "../status";
import type { ThresholdTrigger } from "../types";
import { railButton, railText, renderRailList } from "./rail-list";

export function renderMath(): void {
  renderRailList(els.mathList, state.math, "No derived channels", (channel) => ({
    main: railButton(
      [
        railText(
          "mark-l",
          channel.unit ? `${channel.name} = ${channel.expr} ${channel.unit}` : `${channel.name} = ${channel.expr}`,
        ),
      ],
      () => {
        if (state.plotted.includes(channel.name)) return;
        setPlotted([...state.plotted, channel.name]);
        state.dirty = true;
        renderChrome();
        draw();
        void refresh();
        void refreshOverview();
      },
    ),
    removeLabel: `Remove ${channel.name}`,
    onRemove: () => void removeMath(channel.name),
  }));
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

async function addMath(): Promise<void> {
  const name = els.mathName.value.trim();
  const expr = els.mathExpr.value.trim();
  const unit = els.mathUnit.value.trim();
  if (!name || !expr) {
    setError("A math channel needs a name and an expression.", "action");
    return;
  }
  await serialDeck(() =>
    withBusy("Compiling math", async () => {
      const next = [...state.math.filter((channel) => channel.name !== name), { name, unit, expr }];
      const summary = await api.setMath(next);
      state.math = next;
      if (!state.plotted.includes(name)) setPlotted([...state.plotted, name]);
      adoptEdited(summary);
      els.mathName.value = "";
      els.mathExpr.value = "";
    }),
  );
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

export function bindDeck(): void {
  els.mathForm.addEventListener("submit", (event) => {
    event.preventDefault();
    void addMath();
  });
  els.trigForm.addEventListener("submit", (event) => {
    event.preventDefault();
    void addTrigger();
  });
  els.timeoutFactor.addEventListener("change", () => void applyTimeoutFactor());
}
