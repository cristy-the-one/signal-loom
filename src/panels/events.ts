import { els } from "../dom";
import { severityOf } from "../family";
import { formatUs } from "../format";
import { state } from "../state";
import { scrubTo } from "../transport";

let eventRows: { tUs: number; button: HTMLElement; selected: boolean }[] = [];

/** Marks the event rows at the playhead; cheap enough to run on every draw. */
export function syncEventSelection(): void {
  for (const row of eventRows) {
    const selected = Math.abs(row.tUs - state.playhead) <= 500;
    if (selected === row.selected) continue;
    row.selected = selected;
    row.button.classList.toggle("is-selected", selected);
    if (selected) row.button.scrollIntoView({ block: "nearest" });
  }
}

export function renderEvents(): void {
  els.eventList.replaceChildren();
  eventRows = [];
  const events = state.summary?.events ?? [];
  if (events.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "This log has no event marks";
    els.eventList.append(note);
    return;
  }
  for (const event of events) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "event-row";
    const severity = severityOf(event.label);
    button.classList.add(severity);
    eventRows.push({ tUs: event.tUs, button, selected: false });
    const dot = document.createElement("span");
    dot.className = `event-dot ${severity}`;
    const time = document.createElement("span");
    time.className = "event-t";
    time.textContent = formatUs(event.tUs);
    const label = document.createElement("span");
    label.className = "event-l";
    label.textContent = event.label;
    button.append(dot, time, label);
    button.addEventListener("click", () => scrubTo(event.tUs));
    els.eventList.append(button);
  }
  syncEventSelection();
  if (state.summary?.eventsTruncated) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "Event list truncated";
    els.eventList.append(note);
  }
}
