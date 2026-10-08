import { els } from "../dom";
import { formatUs } from "../format";
import { draw, state } from "../state";
import { scrubTo } from "../transport";
import type { Bookmark, Note } from "../types";
import { railButton, railText, renderRailList } from "./rail-list";

export function renderMarks(): void {
  renderRailList(els.markList, state.bookmarks, "B drops a mark at the playhead", (mark) => ({
    main: railButton(
      [
        railText("mark-dot", ""),
        railText("mark-t", formatUs(mark.tUs)),
        railText("mark-l", mark.label),
      ],
      () => {
        state.selectedMark = mark.id;
        scrubTo(mark.tUs);
        renderMarks();
      },
      state.selectedMark === mark.id,
    ),
    removeLabel: `Remove ${mark.label}`,
    onRemove: () => removeBookmark(mark.id),
  }));
}

export function renderNotes(): void {
  renderRailList(els.noteList, state.notes, "N focuses a note at the playhead", (note) => ({
    main: railButton([railText("mark-t", formatUs(note.tUs)), railText("mark-l", note.body)], () => scrubTo(note.tUs)),
    removeLabel: "Remove note",
    onRemove: () => {
      state.notes = state.notes.filter((item) => item.id !== note.id);
      state.dirty = true;
      renderNotes();
      draw();
    },
  }));
}

export function addBookmark(label?: string): void {
  if (!state.summary) return;
  const text = (label ?? els.markLabel.value).trim() || `Mark ${state.bookmarks.length + 1}`;
  const mark: Bookmark = {
    id: `m-${Date.now().toString(36)}-${state.bookmarks.length + 1}`,
    tUs: Math.round(state.playhead),
    label: text,
  };
  state.bookmarks = [...state.bookmarks, mark].sort((a, b) => a.tUs - b.tUs);
  state.selectedMark = mark.id;
  state.dirty = true;
  els.markLabel.value = "";
  renderMarks();
  draw();
}

export function removeBookmark(id: string): void {
  state.bookmarks = state.bookmarks.filter((mark) => mark.id !== id);
  if (state.selectedMark === id) state.selectedMark = null;
  state.dirty = true;
  renderMarks();
  draw();
}

function addNote(): void {
  if (!state.summary) return;
  const body = els.noteBody.value.trim();
  if (!body) return;
  const note: Note = {
    id: `n-${Date.now().toString(36)}`,
    tUs: Math.round(state.playhead),
    body,
  };
  state.notes = [...state.notes, note].sort((a, b) => a.tUs - b.tUs);
  state.dirty = true;
  els.noteBody.value = "";
  renderNotes();
  draw();
}

export function bindMarks(): void {
  els.markForm.addEventListener("submit", (event) => {
    event.preventDefault();
    addBookmark();
  });
  els.noteForm.addEventListener("submit", (event) => {
    event.preventDefault();
    addNote();
  });
}
