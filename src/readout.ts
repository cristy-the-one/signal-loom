import { els } from "./dom";
import { basename, formatCount, formatHexBytes, formatSpan, formatTimeForSpan, hexId } from "./format";
import { laneModeLabel } from "./lanes";
import { state } from "./state";

/** The project name and the unsaved marker; the part of the chrome a signal toggle changes. */
export function renderDirty(): void {
  const project = state.projectPath ? basename(state.projectPath) : "Untitled";
  els.projectName.textContent = state.dirty ? `${project} ·` : project;
  els.save.classList.toggle("is-dirty", state.dirty);
}

/** The transport bar: time, span, play state, rate, cursor stats and the frame under the playhead. */
export function renderTransport(): void {
  els.timeReadout.textContent = formatTimeForSpan(state.playhead, state.span);
  els.spanReadout.textContent = `span ${formatSpan(state.span)}`;
  els.laneMode.textContent = laneModeLabel(state.laneMode);
  els.laneMode.classList.toggle("is-active", state.laneMode !== "overlay");
  els.play.textContent = state.playing ? "❚❚" : "▶";
  els.rate.textContent = `${state.rate}×`;
  els.cursorRead.textContent = state.cursorText;
  const frame = state.frame;
  if (!frame) {
    els.frameReadout.textContent = "";
    els.frameReadout.title = "";
  } else if (frame.messageId != null) {
    const id = frame.extended ? `${hexId(frame.messageId)}x` : hexId(frame.messageId);
    const shortHex = formatHexBytes(frame.dataHex, 8);
    const hex = shortHex ? ` [${frame.dlc}] ${shortHex}` : "";
    const msg = frame.messageName ? ` ${frame.messageName}` : "";
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${id}${msg}${hex}`;
    els.frameReadout.title = shortHex
      ? `${id}${msg} [${frame.dlc}] ${formatHexBytes(frame.dataHex, 64)}`
      : "";
  } else {
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${frame.messageName}`;
    els.frameReadout.title = "";
  }
}
