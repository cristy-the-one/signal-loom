import * as api from "./api";
import { els } from "./dom";
import { errText, formatCount } from "./format";
import { state } from "./state";
import type { ProjectFile, ProjectOpen, Summary } from "./types";

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

/** What a finished background job hands over: the summary, and the project when the job opened one. */
interface IndexResult {
  summary: Summary;
  project?: { project: ProjectFile; warnings: string[] };
}

/** Runs a background job behind the veil, with progress and a cancel button, until its result is ready. */
async function runIndex(label: string, start: () => Promise<void>): Promise<IndexResult> {
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
      if (tick.done && tick.summary) return { summary: tick.summary, project: tick.project };
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

/** Runs a background index behind the veil until its summary is ready. */
export async function withIndex(label: string, start: () => Promise<void>): Promise<Summary> {
  return (await runIndex(label, start)).summary;
}

/** Like `withIndex` for a job that opens a project: resolves to what the project open reports. */
export async function withProjectIndex(label: string, start: () => Promise<void>): Promise<ProjectOpen> {
  const { summary, project } = await runIndex(label, start);
  if (!project) throw new Error("the project open returned no project");
  return { project: project.project, warnings: project.warnings, summary };
}

/** Runs a background job behind the veil, then hands its summary to `adopt`. A failure or a cancel is reported, not thrown. */
export async function indexThen(
  label: string,
  start: () => Promise<void>,
  adopt: (summary: Summary) => void,
): Promise<void> {
  try {
    adopt(await withIndex(label, start));
  } catch (err) {
    setError(errText(err), "action");
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
