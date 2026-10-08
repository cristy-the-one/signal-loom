import { els } from "./dom";
import { basename, formatCount, formatSpan, formatUs, hexId } from "./format";
import { state } from "./state";

/** The project name and the unsaved marker; the part of the chrome a signal toggle changes. */
export function renderDirty(): void {
  const project = state.projectPath ? basename(state.projectPath) : "Untitled";
  els.projectName.textContent = state.dirty ? `${project} ·` : project;
  els.save.classList.toggle("is-dirty", state.dirty);
}

/** The transport bar: time, span, play state, rate, cursor stats and the frame under the playhead. */
export function renderTransport(): void {
  els.timeReadout.textContent = formatUs(state.playhead);
  els.spanReadout.textContent = `span ${formatSpan(state.span)}`;
  els.play.textContent = state.playing ? "❚❚" : "▶";
  els.rate.textContent = `${state.rate}×`;
  els.cursorRead.textContent = state.cursorText;
  const frame = state.frame;
  if (!frame) {
    els.frameReadout.textContent = "";
  } else if (frame.messageId != null) {
    const id = frame.extended ? `${hexId(frame.messageId)}x` : hexId(frame.messageId);
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${id} ${frame.messageName}`;
  } else {
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${frame.messageName}`;
  }
}
