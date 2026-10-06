# Signal Loom

Signal Loom is a desktop log, CAN, and telemetry browser. Drop a recording on the deck, scrub the timeline, decode signals, overlay them, and bookmark events. It is built for instrument-cluster and vehicle work: a DAW for buses, not a slicer.

The heavy work stays in Rust. The window is a Tauri 2 shell. The UI is TypeScript and Vite, and it only asks the indexer for the samples currently on screen.

## Run it

Requirements: Node.js 22+, a current stable Rust toolchain, and the [Tauri Linux prerequisites](https://tauri.app/start/prerequisites/) (`webkit2gtk`, `gtk`, `librsvg`, `patchelf`) when you are on Linux.

```bash
npm install
npm run tauri dev
```

That builds the Rust core, starts Vite on [http://127.0.0.1:43127](http://127.0.0.1:43127), and opens the Signal Loom window. The first launch compiles Tauri, so it takes a while. Later launches are ordinary.

### Browser preview

The same UI can run against a localhost copy of the indexer. This is for layout checks. The shipping app is the Tauri shell. Nothing listens on a public interface.

```bash
npm run dev:preview
```

Vite stays on port 43127 and proxies `/api` to `loom-serve` on `127.0.0.1:43128`. The page shows a **Browser preview** badge. File open uses the browser file picker; the bytes are indexed by the local Rust process.

## What's on the deck

The app opens `fixtures/cluster_drive.slog`, a 45 second synthetic key-on drive, decoded with `fixtures/cluster.map.json`. It is not a capture from a vehicle. `fixtures/demo.loom` is a project that points at those files and stores three bookmarks. `fixtures/decoded_snippet.csv` is a tiny pre-decoded trace.

Regenerate the drive with:

```bash
python3 scripts/gen_fixture.py
```

## What is real

- **SLOGv1 text**, **SLB1 binary**, **CAN CSV** (`t_us,id,data`), and **decoded CSV** (`t_us,signal,value`) are parsed and indexed.
- The index stores checkpoint offsets (every 256 frames), not every sample. A plot asks for a time window and a point budget. Min/max buckets keep spikes while the point count stays capped.
- The JSON signal map is decoded for real: little-endian (Intel) and big-endian (Motorola) bit layouts, signed values, factor, and offset. It is a DBC-shaped subset, not a Vector `.dbc` importer.
- `.loom` projects store the log path, map path, bookmarks, playhead, span, and plotted signals.
- Scrub, frame step, event step, playback, and overlay plots call that indexer.
- While a log is indexing, the viewport blurs and dims. The signal list stays put.

## What is not in this build

- Vector `.dbc` import. Bring a JSON signal map instead. There is no stub that pretends to import DBC.
- Live CAN hardware, sockets, or a cloud account. The app is offline.
- Multi-log compare and arbitrary math channels.

No decode path is mocked. The browser preview uses the same `loom-core` crate over localhost.

## Keyboard

| Action | Keys |
| --- | --- |
| Open log | Ctrl/Cmd+O |
| Open project | Ctrl/Cmd+Shift+O |
| Save / save as | Ctrl/Cmd+S, Ctrl/Cmd+Shift+S |
| Load sample | Ctrl/Cmd+Shift+L |
| Frame step | Left / Right |
| Event or bookmark step | Shift+Left / Shift+Right |
| Play / pause | Space |
| Bookmark the playhead | B |
| Zoom | `[` `]` or the wheel |
| Pan | Shift+wheel |
| Filter signals | `/` |

Transport buttons are slim on purpose.

## Log formats

SLOGv1, times in microseconds:

```text
SLOGv1
F 0 1A0 800C881378640000
E 2000000 Pullaway
```

`F` is a frame: timestamp, hex CAN id, hex payload (1–8 bytes). `E` is an event label. `#` comments are ignored.

SLB1 is a little-endian binary: 16-byte header (`SLB1`, version 1), then records tagged `1` (frame: `u64` time, `u32` id, `u8` dlc, 8 data bytes) or `2` (event: `u64` time, `u16` length, utf-8 label).

CAN CSV ids are decimal unless they contain `A–F` or a `0x` prefix. Timestamps are always integer microseconds. Logs must be time-sorted.

A signal map looks like `fixtures/cluster.map.json`: messages, a CAN id, and signals with `startBit`, `bitLength`, `factor`, `offset`, `endian` (`little` or `big`). A log named `drive.slog` will pick up `drive.map.json` beside it.

## Project file

`.loom` is JSON:

```json
{
  "format": "signal-loom",
  "version": 1,
  "logPath": "fixtures/cluster_drive.slog",
  "signalMapPath": "fixtures/cluster.map.json",
  "bookmarks": [{ "id": "pullaway", "tUs": 2000000, "label": "Pullaway" }],
  "view": { "playheadUs": 2000000, "spanUs": 45000000, "plotted": ["VehicleSpeed"] }
}
```

Paths resolve relative to the project file, then by walking parent directories. The built-in sample still opens if a project names `cluster_drive.slog` and the file is not on disk.

## Layout

```text
crates/loom-core     parse, index, decode, window query
crates/loom-serve    localhost preview of that crate
src-tauri            Tauri 2 shell, unsigned builds
src                  Vite + TypeScript UI
fixtures             sample log, map, project, CSV snippet
```

## Checks

```bash
npm run typecheck
npm run build
cargo test -p loom-core
cargo clippy -p loom-core -p loom-serve --all-targets -- -D warnings
cargo check -p signal-loom
```

`.github/workflows/ci.yml` keeps three job names stable: `frontend`, `rust`, and `tauri-smoke`.

## Installers

`.github/workflows/release.yml` builds unsigned installers on `workflow_dispatch` and on `v*` tags. Windows is an NSIS setup (`Signal Loom_0.1.0_x64-setup.exe` at the current version). Linux builds a `.deb` and an AppImage. macOS builds two dmgs, Apple silicon and Intel, after installing that job's Rust target on the stable toolchain pinned in `rust-toolchain.toml`. The installer name and wizard use the product name **Signal Loom**, with `src-tauri/icons/icon.ico` (Windows) and `icon.icns` (macOS).

This repository is hosted on Origin and is not mirrored to GitHub, and no Depot or Buildkite app is installed on it. Origin does not run GitHub Actions by itself, and it has no GitHub Release API. The draft release is created when the workflow runs on GitHub Actions. Connect Depot or Buildkite from the repo's Apps tab if you want the same file to build on Origin; those runs still upload the installers as workflow artifacts.
