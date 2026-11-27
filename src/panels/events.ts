import { els } from "../dom";
import { severityOf } from "../family";
import { formatUs } from "../format";
import { state } from "../state";
import { scrubTo } from "../transport";
import type { LogEvent } from "../types";
import { createVirtualList, type VirtualList } from "../virtual-list";

/** Must match `.event-row { height }` in styles.css. */
const EVENT_ROW_HEIGHT = 24;
const SELECT_TOLERANCE_US = 500;

let eventVirtual: VirtualList<LogEvent> | null = null;
let currentEvents: readonly LogEvent[] = [];
let lastScrolledIndex = -1;

function firstSelectedIndex(events: readonly LogEvent[], playhead: number): number {
  let lo = 0;
  let hi = events.length;
  const target = playhead - SELECT_TOLERANCE_US;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (events[mid].tUs < target) lo = mid + 1;
    else hi = mid;
  }
  return lo < events.length && Math.abs(events[lo].tUs - playhead) <= SELECT_TOLERANCE_US ? lo : -1;
}

function buildEventRow(event: LogEvent): HTMLElement {
  const button = document.createElement("button");
  button.type = "button";
  const severity = severityOf(event.label);
  button.className = `event-row ${severity}`;
  const dot = document.createElement("span");
  dot.className = `event-dot ${severity}`;
  const time = document.createElement("span");
  time.className = "event-t";
  time.textContent = formatUs(event.tUs);
  const label = document.createElement("span");
  label.className = "event-l";
  label.textContent = event.label;
  button.title = `${formatUs(event.tUs)} — ${event.label}`;
  button.append(dot, time, label);
  button.addEventListener("click", () => scrubTo(event.tUs));
  return button;
}

function syncEventRow(el: HTMLElement, event: LogEvent): void {
  const selected = Math.abs(event.tUs - state.playhead) <= SELECT_TOLERANCE_US;
  el.classList.toggle("is-selected", selected);
}

function virtualList(): VirtualList<LogEvent> {
  eventVirtual ??= createVirtualList<LogEvent>(els.eventList, EVENT_ROW_HEIGHT, buildEventRow, syncEventRow);
  return eventVirtual;
}

/** Marks the event rows at the playhead in O(log N) + visible rows, scrolling to newly hit events. */
export function syncEventSelection(): void {
  if (!eventVirtual || currentEvents.length === 0) return;
  const hit = firstSelectedIndex(currentEvents, state.playhead);
  if (hit !== -1 && hit !== lastScrolledIndex) {
    lastScrolledIndex = hit;
    eventVirtual.scrollToIndex(hit);
  } else if (hit === -1) {
    lastScrolledIndex = -1;
  }
  eventVirtual.refresh();
}

export function renderEvents(): void {
  const events = state.summary?.events ?? [];
  currentEvents = events;
  lastScrolledIndex = -1;
  const truncated = state.summary?.eventsTruncated ?? false;
  const emptyText = truncated ? "Event list truncated" : "This log has no event marks";
  virtualList().setItems(events, emptyText, truncated ? "Event list truncated" : "");
  syncEventSelection();
}
