import { els } from "./dom";
import { loadSample, openLog, openProject, saveProject } from "./files";
import { addBookmark, removeBookmark } from "./panels/marks";
import { dropCursor } from "./panels/drive";
import { showTab } from "./panels/tabs";
import { state } from "./state";
import { scrubTo, stepEvent, stepFrame, togglePlay, zoom } from "./transport";

function typingTarget(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
}

function onKey(event: KeyboardEvent): void {
  const mod = event.metaKey || event.ctrlKey;
  if (event.key === "Escape") {
    els.help.close();
    els.sigFilter.blur();
    return;
  }
  if (mod && event.key.toLowerCase() === "o" && !event.shiftKey) {
    event.preventDefault();
    void openLog();
    return;
  }
  if (mod && event.key.toLowerCase() === "o" && event.shiftKey) {
    event.preventDefault();
    void openProject();
    return;
  }
  if (mod && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void saveProject(event.shiftKey);
    return;
  }
  if (mod && event.shiftKey && event.key.toLowerCase() === "l") {
    event.preventDefault();
    void loadSample();
    return;
  }
  if (els.help.open) return;
  if (typingTarget(event.target)) return;
  if (event.key === "?" || (event.shiftKey && event.key === "/")) {
    event.preventDefault();
    els.help.showModal();
    return;
  }
  if (event.key === "/") {
    event.preventDefault();
    els.sigFilter.focus();
    return;
  }
  if (event.key === " ") {
    event.preventDefault();
    togglePlay();
    return;
  }
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    const dir = event.key === "ArrowRight" ? 1 : -1;
    if (event.shiftKey) stepEvent(dir as 1 | -1);
    else void stepFrame(dir > 0 ? "next" : "prev");
    return;
  }
  if (event.key === "b" || event.key === "B") {
    event.preventDefault();
    addBookmark();
    return;
  }
  if (event.key === "n" || event.key === "N") {
    event.preventDefault();
    showTab("notes");
    els.noteBody.focus();
    return;
  }
  if (event.key === "1") {
    event.preventDefault();
    dropCursor("a");
    return;
  }
  if (event.key === "2") {
    event.preventDefault();
    dropCursor("b");
    return;
  }
  if ((event.key === "Delete" || event.key === "Backspace") && state.selectedMark) {
    event.preventDefault();
    removeBookmark(state.selectedMark);
    return;
  }
  if (event.key === "[" || event.key === "]") {
    event.preventDefault();
    zoom(event.key === "]" ? 0.7 : 1.4);
    return;
  }
  if (event.key === "Home" && state.summary) {
    event.preventDefault();
    scrubTo(state.summary.tStartUs);
  }
  if (event.key === "End" && state.summary) {
    event.preventDefault();
    scrubTo(state.summary.tEndUs);
  }
}

export function bindKeys(): void {
  els.btnHelp.addEventListener("click", () => {
    if (!els.help.open) els.help.showModal();
  });
  window.addEventListener("keydown", onKey);
}
