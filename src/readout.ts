import { els } from "./dom";
import { familyColor } from "./family";
import { basename, formatCount, formatHexBytes, formatSpan, formatTimeForSpan, hexId } from "./format";
import { colorFor } from "./model";
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
  els.spanReadout.textContent = formatSpan(state.span);
  els.spanReadout.title = `Visible window span: ${formatSpan(state.span)} (scroll, W/S, or [ / ] to zoom)`;
  const modes: [HTMLButtonElement, "overlay" | "split" | "grouped"][] = [
    [els.laneModeOverlay, "overlay"],
    [els.laneModeSplit, "split"],
    [els.laneModeGrouped, "grouped"],
  ];
  for (const [btn, mode] of modes) {
    const active = state.laneMode === mode;
    btn.classList.toggle("is-active", active);
    btn.setAttribute("aria-checked", active ? "true" : "false");
  }
  els.laneLoupe.classList.toggle("is-active", state.laneLoupe);
  els.laneLoupe.setAttribute("aria-pressed", state.laneLoupe ? "true" : "false");
  els.laneLoupe.title = state.laneLoupe
    ? "Hover lane magnifier: ON — click or press V to disable"
    : "Hover lane magnifier: OFF — click or press V to enable";
  els.play.textContent = state.playing ? "❚❚" : "▶";
  els.rate.textContent = `${state.rate}×`;
  els.rate.classList.toggle("is-active", state.rate !== 1);
  els.cursorRead.textContent = state.cursorText;

  const frame = state.frame;
  if (!frame) {
    els.frameReadout.replaceChildren();
    els.frameReadout.title = "";
    return;
  }

  const ordWidth = formatCount(Math.max(frame.ordinal, state.summary?.frameCount ?? 0)).length;
  const ordText = formatCount(frame.ordinal).padStart(ordWidth);

  if (frame.messageId != null) {
    const rawId = frame.extended ? `${hexId(frame.messageId)}x` : hexId(frame.messageId);
    const idText = rawId.padEnd(frame.extended ? 11 : 5);
    const shortHex = formatHexBytes(frame.dataHex, 8);
    const dlcText = `[${frame.dlc}]`;
    const color =
      frame.values.length > 0
        ? colorFor(frame.values[0].name)
        : familyColor(frame.messageName || rawId, frame.messageName || "", 0);

    // Fixed-width columns come first so the raw hex payload stays rock-steady
    // horizontally; variable-length message name sits at the end.
    const prefix = document.createElement("span");
    prefix.className = "frame-meta";
    prefix.textContent = `f ${ordText} · ${idText} ${dlcText} `;

    const hex = document.createElement("span");
    hex.className = "frame-hex";
    hex.style.color = color;
    hex.textContent = shortHex || "—";

    const nodes: Node[] = [prefix, hex];
    if (frame.messageName) {
      const msg = document.createElement("span");
      msg.className = "frame-msg";
      msg.textContent = ` · ${frame.messageName}`;
      nodes.push(msg);
    }

    els.frameReadout.replaceChildren(...nodes);
    els.frameReadout.title = shortHex
      ? `${rawId}${frame.messageName ? ` ${frame.messageName}` : ""} ${dlcText} ${formatHexBytes(frame.dataHex, 64)}`
      : "";
  } else {
    els.frameReadout.textContent = `f ${ordText} · ${frame.messageName}`;
    els.frameReadout.title = "";
  }
}
