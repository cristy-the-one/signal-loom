/**
 * Fixed-row-height virtual list for scrollable rail containers (`#sig-list`, `#event-list`).
 * Below `VIRTUAL_THRESHOLD` items it renders plain DOM children (zero spacer overhead);
 * above `VIRTUAL_THRESHOLD` it mounts only the visible viewport slice plus `OVERSCAN` rows.
 */

const VIRTUAL_THRESHOLD = 100;
const OVERSCAN = 8;

export interface VirtualList<T> {
  /** Replace the backing items and repaint the visible slice. */
  setItems(items: readonly T[], emptyText?: string): void;
  /** Re-run row state updates on currently mounted rows (e.g. selection / checkbox sync). */
  refresh(): void;
  /** Scroll just enough so `index` is inside the visible viewport. */
  scrollToIndex(index: number): void;
}

export function createVirtualList<T>(
  container: HTMLElement,
  rowHeight: number,
  renderRow: (item: T, index: number) => HTMLElement,
  syncRow?: (el: HTMLElement, item: T, index: number) => void,
): VirtualList<T> {
  let items: readonly T[] = [];
  let emptyMessage = "";
  let mountedStart = -1;
  let mountedEnd = -1;
  let mountedNodes: Map<number, HTMLElement> = new Map();
  let rafId = 0;

  const topSpacer = document.createElement("div");
  const bottomSpacer = document.createElement("div");
  topSpacer.style.flex = "none";
  bottomSpacer.style.flex = "none";

  function viewportHeight(): number {
    return container.clientHeight || 360;
  }

  function paintSlice(force: boolean): void {
    if (items.length === 0) {
      mountedNodes.clear();
      mountedStart = -1;
      mountedEnd = -1;
      container.replaceChildren();
      if (emptyMessage) {
        const note = document.createElement("p");
        note.className = "empty-note";
        note.textContent = emptyMessage;
        container.append(note);
      }
      return;
    }

    if (items.length <= VIRTUAL_THRESHOLD) {
      if (force || mountedStart !== 0 || mountedEnd !== items.length) {
        mountedNodes.clear();
        const frag = document.createDocumentFragment();
        for (let i = 0; i < items.length; i += 1) {
          const el = renderRow(items[i], i);
          if (syncRow) syncRow(el, items[i], i);
          mountedNodes.set(i, el);
          frag.append(el);
        }
        mountedStart = 0;
        mountedEnd = items.length;
        container.replaceChildren(frag);
      } else if (syncRow) {
        for (const [idx, el] of mountedNodes) {
          syncRow(el, items[idx], idx);
        }
      }
      return;
    }

    const scrollTop = container.scrollTop;
    const vh = viewportHeight();
    const start = Math.max(0, Math.floor(scrollTop / rowHeight) - OVERSCAN);
    const end = Math.min(items.length, Math.ceil((scrollTop + vh) / rowHeight) + OVERSCAN);

    if (!force && start === mountedStart && end === mountedEnd) {
      if (syncRow) {
        for (const [idx, el] of mountedNodes) {
          syncRow(el, items[idx], idx);
        }
      }
      return;
    }

    const nextNodes = new Map<number, HTMLElement>();
    const frag = document.createDocumentFragment();
    topSpacer.style.height = `${start * rowHeight}px`;
    bottomSpacer.style.height = `${(items.length - end) * rowHeight}px`;
    frag.append(topSpacer);

    for (let i = start; i < end; i += 1) {
      let el = !force ? mountedNodes.get(i) : undefined;
      if (!el) el = renderRow(items[i], i);
      if (syncRow) syncRow(el, items[i], i);
      nextNodes.set(i, el);
      frag.append(el);
    }

    frag.append(bottomSpacer);
    mountedNodes = nextNodes;
    mountedStart = start;
    mountedEnd = end;
    container.replaceChildren(frag);
  }

  container.addEventListener(
    "scroll",
    () => {
      if (items.length <= VIRTUAL_THRESHOLD || rafId) return;
      rafId = requestAnimationFrame(() => {
        rafId = 0;
        paintSlice(false);
      });
    },
    { passive: true },
  );

  return {
    setItems(next: readonly T[], emptyText = ""): void {
      items = next;
      emptyMessage = emptyText;
      if (container.scrollTop > next.length * rowHeight) {
        container.scrollTop = 0;
      }
      paintSlice(true);
    },
    refresh(): void {
      paintSlice(false);
    },
    scrollToIndex(index: number): void {
      if (index < 0 || index >= items.length) return;
      if (items.length <= VIRTUAL_THRESHOLD) {
        mountedNodes.get(index)?.scrollIntoView({ block: "nearest" });
        return;
      }
      const itemTop = index * rowHeight;
      const itemBottom = itemTop + rowHeight;
      const viewTop = container.scrollTop;
      const viewBottom = viewTop + viewportHeight();
      if (itemTop < viewTop) {
        container.scrollTop = itemTop;
        paintSlice(false);
      } else if (itemBottom > viewBottom) {
        container.scrollTop = itemBottom - viewportHeight();
        paintSlice(false);
      }
    },
  };
}
