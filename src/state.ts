import { defaultClusterWidgets, type ClusterBindings } from "./gauges";
import type { RailTab } from "./dom";
import type { LaneMode } from "./lanes";
import type {
  Bookmark,
  BusLoad,
  ClusterWidget,
  FrameHit,
  MathChannel,
  Note,
  Point,
  Series,
  Summary,
  ThresholdTrigger,
  TraceStyle,
  ValueRead,
} from "./types";

export interface View {
  t0: number;
  t1: number;
}

interface State {
  summary: Summary | null;
  playhead: number;
  span: number;
  plotted: string[];
  bookmarks: Bookmark[];
  selectedMark: string | null;
  filter: string;
  projectPath: string | null;
  dirty: boolean;
  playing: boolean;
  rate: number;
  busy: number;
  series: Series[];
  /** The stretch of the log `series` covers; traces are not drawn past it. */
  seriesRange: View | null;
  view: View;
  /** Explicit camera center when panned/zoomed independently of the playhead; null tracks playhead. */
  viewCenter: number | null;
  /** Active Shift+drag box-zoom selection on the plot, if any. */
  boxZoom: View | null;
  /** Plotted signal temporarily soloed from the scope legend, if any. */
  solo: string | null;
  /** Scope track layout: single overlay, split per-signal tracks, or grouped tracks. */
  laneMode: LaneMode;
  /** Manual signal -> track group number (`1..N`) overrides used in `grouped` mode. */
  signalLanes: Record<string, number>;
  /** Per-signal scope visualization style override (`auto`, `step`, `line`, `area`, `state`, `flag`, `heat`). */
  signalStyles: Record<string, TraceStyle>;
  frame: FrameHit | null;
  overview: Point[] | null;
  overviewName: string | null;
  held: ValueRead[];
  bus: BusLoad | null;
  hoverT: number | null;
  hoverX: number;
  cursorA: number | null;
  cursorB: number | null;
  cursorText: string;
  math: MathChannel[];
  triggers: ThresholdTrigger[];
  notes: Note[];
  compareOn: boolean;
  comparePath: string | null;
  compareOffsetUs: number;
  /** Legacy cluster bindings kept for backward compatibility. */
  cluster: ClusterBindings;
  /** Ordered customizable cluster gadgets. */
  clusterWidgets: ClusterWidget[];
  /** True once the user edits/reorders/adds/removes gadgets (so saves persist `clusterWidgets`). */
  clusterCustomized: boolean;
  tab: RailTab;
}

export const state: State = {
  summary: null,
  playhead: 0,
  span: 1,
  plotted: [],
  bookmarks: [],
  selectedMark: null,
  filter: "",
  projectPath: null,
  dirty: false,
  playing: false,
  rate: 1,
  busy: 0,
  series: [],
  seriesRange: null,
  view: { t0: 0, t1: 1 },
  viewCenter: null,
  boxZoom: null,
  solo: null,
  laneMode: "overlay",
  signalLanes: {},
  signalStyles: {},
  frame: null,
  overview: null,
  overviewName: null,
  held: [],
  bus: null,
  hoverT: null,
  hoverX: 0,
  cursorA: null,
  cursorB: null,
  cursorText: "",
  math: [],
  triggers: [],
  notes: [],
  compareOn: false,
  comparePath: null,
  compareOffsetUs: 0,
  cluster: {},
  clusterWidgets: defaultClusterWidgets(),
  clusterCustomized: false,
  tab: "marks",
};

let dataGeneration = 0;

/** Bumped when the data a plot response belongs to changes: a new summary or a new plotted set. */
export function bumpDataGen(): void {
  dataGeneration += 1;
}

export function dataGen(): number {
  return dataGeneration;
}

/** What the render requests below call; bound once by the composition root so this module imports no views. */
interface Renderers {
  paint(): void;
  chrome(): void;
}

let renderers: Renderers | null = null;
let drawFrame = 0;

export function bindRenderers(next: Renderers): void {
  renderers = next;
}

function bound(): Renderers {
  if (!renderers) throw new Error("renderers are not bound");
  return renderers;
}

/** Paints on the next animation frame; any number of requests before it share one paint. */
export function requestDraw(): void {
  // While playing, tick paints every frame.
  if (drawFrame || (state.playing && state.summary)) return;
  drawFrame = requestAnimationFrame(() => {
    drawFrame = 0;
    draw();
  });
}

/** Paints now, and drops any paint already requested. */
export function draw(): void {
  if (drawFrame) {
    cancelAnimationFrame(drawFrame);
    drawFrame = 0;
  }
  bound().paint();
}

/** Rebuilds the header, warnings and every rail list from the state. */
export function renderChrome(): void {
  bound().chrome();
}
