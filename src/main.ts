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
import { adoptSummary } from "./project";
import { bindRenderers, draw } from "./state";
import { paintBusy, withBusy } from "./status";
import { bindTransport } from "./transport";
import "./styles.css";

function bind(): void {
  bindRenderers({ paint, chrome: paintChrome });
  els.runtime.textContent = api.inTauri() ? "Desktop" : "Browser preview";
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

async function boot(): Promise<void> {
  bind();
  draw();
  if (api.inTauri()) {
    await loadSample();
    return;
  }
  await withBusy("Waiting for the local engine", async () => {
    await waitForEngine();
    paintBusy("Indexing cluster sample");
    adoptSummary(await api.openSample(), "fresh");
  });
}

void boot();
