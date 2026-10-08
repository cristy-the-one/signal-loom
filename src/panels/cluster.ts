import { els } from "../dom";
import { slotAt, slotSpec, type SlotId } from "../gauges";
import { hasData } from "../model";
import { draw, renderChrome, state } from "../state";

/** The slot under a mouse event on the cluster canvas. */
function slotFromEvent(event: MouseEvent): SlotId | null {
  const rect = els.gauges.getBoundingClientRect();
  return slotAt(event.clientX - rect.left, event.clientY - rect.top, rect.width, rect.height);
}

function closeSlotChooser(): void {
  els.slotChooser.hidden = true;
}

/** Let the user pick the signal for one gauge or lamp: any signal with data, or the default. */
function openSlotChooser(slot: SlotId, clientX: number, clientY: number): void {
  if (!state.summary) return;
  const select = els.slotSignal;
  els.slotTitle.textContent = slotSpec(slot).title;
  const fallback = document.createElement("option");
  fallback.value = "";
  fallback.textContent = `Default (${slotSpec(slot).defaults[0]})`;
  const options = [fallback];
  const signals = state.summary.signals
    .filter(hasData)
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
  for (const signal of signals) {
    const option = document.createElement("option");
    option.value = signal.name;
    option.textContent = signal.messageName ? `${signal.name} · ${signal.messageName}` : signal.name;
    options.push(option);
  }
  select.replaceChildren(...options);
  select.value = state.cluster[slot] ?? "";
  select.onchange = () => {
    if (select.value) state.cluster = { ...state.cluster, [slot]: select.value };
    else {
      const next = { ...state.cluster };
      delete next[slot];
      state.cluster = next;
    }
    state.dirty = true;
    closeSlotChooser();
    renderChrome();
    draw();
  };
  els.slotChooser.style.left = `${Math.min(clientX, window.innerWidth - 240)}px`;
  els.slotChooser.style.top = `${clientY + 12}px`;
  els.slotChooser.hidden = false;
  select.focus();
}

export function bindCluster(): void {
  els.gauges.addEventListener("click", (event) => {
    const slot = slotFromEvent(event);
    if (slot) openSlotChooser(slot, event.clientX, event.clientY);
  });
  els.gauges.addEventListener("mousemove", (event) => {
    els.gauges.classList.toggle("on-slot", state.summary != null && slotFromEvent(event) != null);
  });
  document.addEventListener("mousedown", (event) => {
    if (!els.slotChooser.hidden && !els.slotChooser.contains(event.target as Node) && event.target !== els.gauges) {
      closeSlotChooser();
    }
  });
  els.slotSignal.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeSlotChooser();
  });
}
