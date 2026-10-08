/** One line of a rail list: the part that does the work and the remove button beside it. */
export interface RailRow {
  main: HTMLElement;
  removeLabel: string;
  onRemove: () => void;
}

export function railText(className: string, text: string): HTMLSpanElement {
  const span = document.createElement("span");
  span.className = className;
  span.textContent = text;
  return span;
}

/** The clickable body of a rail row. */
export function railButton(parts: HTMLElement[], onClick: () => void, selected = false): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = selected ? "mark-row is-selected" : "mark-row";
  button.append(...parts);
  button.addEventListener("click", onClick);
  return button;
}

/** Fills a rail list with one row per item, or the empty-state line when there are none. */
export function renderRailList<T>(
  host: HTMLElement,
  items: readonly T[],
  empty: string,
  build: (item: T) => RailRow,
): void {
  host.replaceChildren();
  if (items.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = empty;
    host.append(note);
    return;
  }
  for (const item of items) {
    const { main, removeLabel, onRemove } = build(item);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "mark-x";
    remove.textContent = "×";
    remove.setAttribute("aria-label", removeLabel);
    remove.addEventListener("click", onRemove);
    const row = document.createElement("div");
    row.className = "mark-row-wrap";
    row.style.display = "flex";
    row.append(main, remove);
    host.append(row);
  }
}
