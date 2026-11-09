# AGENTS.md

## Overview

Strawberry is a GPU-accelerated scientific image editor (macOS desktop app) written in Rust. `TODO.md` is the roadmap and known-issues list.

## Commands

```sh
cargo run                    # launches the GUI (strawberry-frontend is the default workspace member)
cargo check --workspace      # type-check everything
cargo build --release        # debug builds are much slower for image processing; some bugs only reproduce in debug

# Headless batch processing
cargo run -p strawberry-cli -- --cli --pipeline pipeline.toml --input <file-or-dir> --output <path>

# Tests (only a few exist: tiling in processing, filename/grouping logic in the CLI)
cargo test --workspace
cargo test -p strawberry-processing tiling   # run a single test module
```

## Workspace layout

Five crates with a strict dependency direction: `processing` ← `strawberry-types` ← `strawberry-backend` ← (`strawberry-frontend`, `strawberry-cli`).

- **`crates/processing`** — raw GPU compute kernels on wgpu (`GpuImageProcessor` wraps device+queue). Each operation is a `src/<op>.rs` paired with a WGSL shader in `src/shaders/<op>.wgsl`. No pipeline/tool logic here.
- **`crates/strawberry-types`** — everything shared: `PipelineCommand`, `FrontendMessage`, per-tool message enums (`messages.rs`), `ToolState` (`state.rs`), overlay types (`overlay.rs`), and the `ToolBackend`/`ToolFrontend` traits (`traits.rs`).
- **`crates/strawberry-backend`** — the `Pipeline` (`lib.rs`) plus one `ToolBackend` impl per tool in `src/tools/`. Also holds non-image tool logic: `counter/` (cell counting, XML count I/O) and `nervetracer/` (A* tracing).
- **`crates/strawberry-frontend`** — GPUI app. `main.rs` owns the `PrimaryViewer` (image canvas, pan/zoom, overlays, toasts) and registers every tool window; `src/tools/` has one `ToolFrontend` view per tool; `src/gui/` has shared widgets (button, value_control, feedback toasts, theme).
- **`crates/strawberry-cli`** — batch runner; drives the same `Pipeline` via `PipelineCommand::CLILoadPipeline` (skips the discovery handshake), with regex file filtering and grouping modes.

Both `image` and `gpui` are custom forks (github.com/maxall41): `image` has single-channel support, `gpui-m` is pinned to a specific rev. Don't bump these to upstream versions.

## Architecture

**Message-passing everywhere; no shared state between frontend and backend.** The backend `Pipeline` runs in a tokio task polling `process_commands()` at ~30 FPS; the GPUI frontend polls its receiver on the same cadence. They exchange `PipelineCommand`s over tokio `broadcast` channels (frontend→backend commands, backend→frontend `Output`/`FrontendMessage`).

**Tool discovery handshake:** a tool window sends `DiscoverBackend(BackendToolType, flume::Sender)`; the pipeline replies with `ReturnBackendTools(indices, senders)` giving the view direct per-tool flume channels. Windows send `DisconnectFrontend` on close (handled by `CleanupGuard` in frontend `tools/mod.rs`). `ToolBase` (frontend `tools/base.rs`) holds this plumbing for every view.

**Pipeline processing:** tools run in `ToolManager` order over a `Vec<RgbaImage>`. Each tool caches its output; `process_from(index)` walks back to the earliest invalid cache and reprocesses from there, so state changes only rerun downstream tools. `Refresh` commands are deduplicated per poll (debouncing). Whenever more than one image remains at the end, the enabled user compositor is force-run; if it is disabled, a separate stateless compositor combines all remaining layers, ignoring the user tool's visibility settings.

**Pixel format gotcha:** images are held in **BGRA** internally because GPUI renders BGRA. Single images get their R/B channels swapped on load (`PipelineCommand::Load` in backend `lib.rs`); `GpuColorspaceConverter` converts back to RGBA on save. Multi-image loads skip the swap because the decomposer/compositor path handles channels itself — mixing these up causes the classic red/blue swap bug.

**Interaction routing:** the viewer converts screen→image pixel coords and broadcasts `HandleClick`/`InteractionStart`/`InteractionMove`/`InteractionEnd`; interactive tools (counter, nervetracer) register via `RegisterClickReceiver`. Tools draw on the viewer by returning `ToolOverlay`s (circles, points, polylines), fetched via `RequestOverlays` after every output frame.

**Pipeline save/load:** tool states serialize to TOML as `ToolState` keyed by `BackendToolType` debug name; the CLI reuses this format.

## Adding a new processing tool

Touch all of these, mirroring an existing simple tool (e.g. Sobel):
1. `processing/src/<tool>.rs` + `processing/src/shaders/<tool>.wgsl` (GPU kernel).
2. `strawberry-types`: message enum in `messages.rs`, a `PipelineObjectMessage` variant, a `BackendToolType` variant, state in `state.rs`.
3. `strawberry-backend/src/tools/<tool>.rs` implementing `ToolBackend`; register it in `Pipeline::new` (backend `lib.rs`).
4. `strawberry-frontend/src/tools/<tool>.rs` implementing `ToolFrontend`; in frontend `commands.rs` add the action to the tool `actions!` list and a `register_tool` call in `install`. That one call registers the action handler and creates the command-palette entry — there is no menu to update.
