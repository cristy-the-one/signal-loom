import * as api from "./api";
import { els } from "./dom";
import { errText, formatCount } from "./format";
import { state } from "./state";
import type { Summary } from "./types";

/** Who raised a banner message; only the owner's later success clears it. */
export type ErrorOwner = "query" | "project" | "action";

const errors = new Map<ErrorOwner, string>();

function paintErrors(): void {
  const text = [...errors.values()].join(" ");
  els.error.hidden = text === "";
  els.error.textContent = text;
}

export function setError(message: string, owner: ErrorOwner): void {
  errors.set(owner, message);
  paintErrors();
}

export function clearError(owner?: ErrorOwner): void {
  if (owner) errors.delete(owner);
  else errors.clear();
  paintErrors();
}

let noticeTimer: ReturnType<typeof setTimeout> | undefined;

/** A short message that is not an error, such as a saved export or the end of the log. It clears itself. */
export function setNotice(message: string): void {
  els.notice.textContent = message;
  els.notice.hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    els.notice.hidden = true;
    els.notice.textContent = "";
  }, 6000);
}

export function paintBusy(label?: string): void {
  const active = state.busy > 0;
  els.viewport.classList.toggle("is-busy", active);
  els.veil.hidden = !active;
  if (label) els.veilLabel.textContent = label;
  if (!active) {
    els.veilDetail.textContent = "";
    els.veilFill.style.width = "0";
    els.veilCancel.hidden = true;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

/** Runs a background index behind the veil, with progress and a cancel button, until its summary is ready. */
export async function withIndex(label: string, start: () => Promise<void>): Promise<Summary> {
  state.busy += 1;
  paintBusy(label);
  els.veilCancel.hidden = false;
  let stop = false;
  const onCancel = () => {
    stop = true;
    void api.cancelIndex();
    els.veilLabel.textContent = "Cancelling";
  };
  els.veilCancel.addEventListener("click", onCancel);
  try {
    await start();
    for (;;) {
      const tick = await api.indexProgress();
      if (tick.error) throw new Error(tick.error);
      if (tick.done && tick.summary) return tick.summary;
      if (tick.idle && !tick.done) throw new Error("indexing did not start");
      const pct = tick.bytesTotal
        ? Math.min(99, Math.round((100 * tick.bytesDone) / tick.bytesTotal))
        : 0;
      els.veilFill.style.width = `${pct}%`;
      els.veilLabel.textContent = stop ? "Cancelling" : `${label} · ${pct}%`;
      const skipped = tick.skipped ? ` · ${formatCount(tick.skipped)} skipped` : "";
      els.veilDetail.textContent = `${formatCount(tick.frames)} frames${skipped}`;
      await sleep(80);
    }
  } finally {
    els.veilCancel.removeEventListener("click", onCancel);
    state.busy = Math.max(0, state.busy - 1);
    paintBusy();
  }
}

let deckQueue: Promise<void> = Promise.resolve();

/** Runs deck mutations one at a time, each building its new list from the state the last one left. */
export function serialDeck(work: () => Promise<void>): Promise<void> {
  const run = deckQueue.then(work);
  deckQueue = run.catch(() => undefined);
  return run;
}

export async function withBusy(label: string, work: () => Promise<void>): Promise<void> {
  state.busy += 1;
  paintBusy(label);
  await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
  try {
    await work();
  } catch (err) {
    setError(errText(err), "action");
  } finally {
    state.busy = Math.max(0, state.busy - 1);
    paintBusy();
  }
}
