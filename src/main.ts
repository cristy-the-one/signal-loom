import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import * as api from "./api";
import { paintChrome } from "./chrome";
import { els } from "./dom";
import { bindFiles, loadSample } from "./files";
import { bindKeys } from "./keys";
import { bindCluster } from "./panels/cluster";
import { bindDeck } from "./panels/deck";
import { bindDrive } from "./panels/drive";
import { bindMarks } from "./panels/marks";
import { bindSignals } from "./panels/signals";
import { bindTabs } from "./panels/tabs";
import { bindPlotView, paint } from "./plot-view";
import { errText } from "./format";
import { adoptSummary, applyProject } from "./project";
import { bindRenderers, draw } from "./state";
import { followIndex, paintBusy, setError, withBusy } from "./status";
import { bindTransport } from "./transport";
import type { IndexStatus } from "./types";
import "./styles.css";

function bindWindowControls(): void {
  if (!api.inTauri()) {
    els.winControls.hidden = true;
    return;
  }
  els.winControls.hidden = false;
  void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
    const win = getCurrentWindow();
    const syncMaxIcon = async () => {
      try {
        const maxed = await win.isMaximized();
        els.winMax.textContent = maxed ? "❐" : "☐";
        els.winMax.title = maxed ? "Restore window" : "Maximize window";
        els.winMax.setAttribute("aria-label", maxed ? "Restore window" : "Maximize window");
      } catch {
        // Ignore if window handle is unavailable
      }
    };
    void syncMaxIcon();
    void win.onResized(() => void syncMaxIcon());
    els.winMin.addEventListener("click", () => void win.minimize());
    els.winMax.addEventListener("click", () => void win.toggleMaximize());
    els.winClose.addEventListener("click", () => void win.close());
    els.topbar.addEventListener("dblclick", (event) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest("button, input, label, select, a")) return;
      void win.toggleMaximize();
    });
  });
}

function bind(): void {
  bindRenderers({ paint, chrome: paintChrome });
  els.runtime.textContent = api.inTauri() ? "Desktop" : "Browser preview";
  bindWindowControls();
  bindFiles();
  bindKeys();
  bindTabs();
  bindSignals();
  bindMarks();
  bindDeck();
  bindDrive();
  bindCluster();
  bindTransport();
  bindPlotView();
}

async function waitForEngine(): Promise<void> {
  for (let attempt = 0; attempt < 180; attempt += 1) {
    if (await api.health()) return;
    if (attempt === 2) paintBusy("Compiling the local indexer…");
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    "Local engine is not running. Use npm run dev:preview, or open the desktop shell with npm run tauri dev.",
  );
}

function adoptStartupResult(
  summary: import("./types").Summary,
  project?: { project: import("./types").ProjectFile; warnings: string[]; compareOpened: boolean },
): void {
  if (project) {
    applyProject(
      { project: project.project, warnings: project.warnings, compareOpened: project.compareOpened, summary },
      null,
    );
  } else {
    adoptSummary(summary, "fresh");
  }
}

async function boot(): Promise<void> {
  bind();
  draw();
  if (api.inTauri()) {
    await loadSample();
    return;
  }
  let initial: IndexStatus | null = null;
  await withBusy("Waiting for the local engine", async () => {
    await waitForEngine();
    initial = await api.indexProgress();
    if (initial.running || initial.done) return;
    try {
      adoptSummary(await api.summary(), "fresh");
      return;
    } catch (err) {
      if (!(err instanceof api.ApiError && err.kind === "not_found")) throw err;
    }
    paintBusy("Indexing cluster sample");
    adoptSummary(await api.openSample(), "fresh");
  });
  const tick = initial as IndexStatus | null;
  if (!tick) return;
  if (tick.running) {
    try {
      const result = await followIndex("Indexing recording", tick);
      adoptStartupResult(result.summary, result.project);
    } catch (err) {
      setError(errText(err), "action");
    }
    return;
  }
  if (tick.error) {
    setError(tick.error, "action");
    return;
  }
  if (tick.summary) {
    adoptStartupResult(tick.summary, tick.project);
  }
}

void boot();
