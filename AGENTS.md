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

# Tests (tiling, pipeline tool management, sidebar ordering/layout, and CLI parsing)
cargo test --workspace
cargo test -p strawberry-processing tiling   # run a single test module
```

## Workspace layout

Five crates with a strict dependency direction: `processing` ← `strawberry-types` ← `strawberry-backend` ← (`strawberry-frontend`, `strawberry-cli`).

- **`crates/processing`** — raw GPU compute kernels on wgpu (`GpuImageProcessor` wraps device+queue). Each operation is a `src/<op>.rs` paired with a WGSL shader in `src/shaders/<op>.wgsl`. No pipeline/tool logic here.
- **`crates/strawberry-types`** — everything shared: `PipelineCommand`, `FrontendMessage`, per-tool message enums (`messages.rs`), `ToolState` (`state.rs`), overlay types (`overlay.rs`), and the `ToolBackend`/`ToolFrontend` traits (`traits.rs`).
- **`crates/strawberry-backend`** — the `Pipeline` (`lib.rs`) plus one `ToolBackend` impl per tool in `src/tools/`. Also holds non-image tool logic: `counter/` (cell counting, XML count I/O) and `nervetracer/` (A* tracing).
- **`crates/strawberry-frontend`** — GPUI app. `main.rs` owns the `PrimaryViewer` (image canvas, pan/zoom, overlays, toasts) and hosts the pipeline sidebar (`src/gui/sidebar.rs`); `src/tools/` has one `ToolFrontend` view per tool; `src/gui/` has shared widgets (button, value_control, feedback toasts, theme).
- **`crates/strawberry-cli`** — batch runner; drives the same `Pipeline` via `PipelineCommand::CLILoadPipeline` (skips the discovery handshake), with regex file filtering and grouping modes.

Both `image` and `gpui` are custom forks (github.com/maxall41): `image` has single-channel support, `gpui-m` is pinned to a specific rev. Don't bump these to upstream versions.

## Architecture

**Message-passing everywhere; no shared state between frontend and backend.** The backend `Pipeline` runs in a tokio task polling `process_commands()` at ~30 FPS; the GPUI frontend polls its receiver on the same cadence. They exchange `PipelineCommand`s over tokio `broadcast` channels (frontend→backend commands, backend→frontend `Output`/`FrontendMessage`).

**Tool discovery handshake:** a tool view sends `DiscoverBackend(BackendToolType, flume::Sender)`; the pipeline replies with `ReturnBackendTools(indices, senders)` giving the view direct per-tool flume channels. Views send `DisconnectFrontend` when released (handled by `CleanupGuard` in frontend `tools/mod.rs`). `ToolBase` (frontend `tools/base.rs`) holds this plumbing for every view. The sidebar connects to the Manager backend and renders its tool stack in pipeline order, including disabled tools. Stack membership is independent of enabled state. Tool controls initialize lazily when expanded and stay alive when collapsed or removed. `ManagerMessage::AddTool` enables and appends a tool; `RemoveTool` removes it from the stack and disables it while retaining its settings. Pipeline files retain all stack members, including disabled tools. Dragging a row sends `ReorderTool` with backend indices, which can differ from visible row numbers.

**Pipeline processing:** tools run in `ToolManager` order over a `Vec<RgbaImage>`. Each tool caches its output; `process_from(index)` walks back to the earliest invalid cache and reprocesses from there, so state changes only rerun downstream tools. `Refresh` commands are deduplicated per poll (debouncing). Whenever more than one image remains at the end, the enabled user compositor is force-run; if it is disabled, a separate stateless compositor combines all remaining layers, ignoring the user tool's visibility settings.

**Pixel format gotcha:** images are held in **BGRA** internally because GPUI renders BGRA. Single images get their R/B channels swapped on load (`PipelineCommand::Load` in backend `lib.rs`); `GpuColorspaceConverter` converts back to RGBA on save. Multi-image loads skip the swap because the decomposer/compositor path handles channels itself — mixing these up causes the classic red/blue swap bug.

**Interaction routing:** the viewer converts screen→image pixel coords and broadcasts `HandleClick`/`InteractionStart`/`InteractionMove`/`InteractionEnd`; interactive tools (counter, nervetracer) register via `RegisterClickReceiver`. Tools draw on the viewer by returning `ToolOverlay`s (circles, points, polylines), fetched via `RequestOverlays` after every output frame.

**Pipeline save/load:** tool states serialize to TOML as `ToolState` keyed by `BackendToolType` debug name; the CLI reuses this format.

## Adding a new processing tool

Touch all of these, mirroring an existing simple tool (e.g. Sobel):
1. `processing/src/<tool>.rs` + `processing/src/shaders/<tool>.wgsl` (GPU kernel).
2. `strawberry-types`: message enum in `messages.rs`, a `PipelineObjectMessage` variant, a `BackendToolType` variant, state in `state.rs`.
3. `strawberry-backend/src/tools/<tool>.rs` implementing `ToolBackend`; register it in `Pipeline::new` (backend `lib.rs`).
4. `strawberry-frontend/src/tools/<tool>.rs` implementing `ToolFrontend`; in frontend `commands.rs` add the action to the tool `actions!` list and a `register_tool` call in `install`. Pass its `BackendToolType` to that call; it registers the sidebar controls, Add Tool option, and command-palette action together — there is no separate menu to update.

## Unit Tests

You may only add unit tests transitivley; they must be removed after you are done testing, unless otherwise informed.

## Readme & Markdown

Never edit the README, never create new markdown files/documentation.

## SVG and Icons

Do NOT write custom SVG icons. Use phosphor icons instead: https://phosphoricons.com/. Do not use text as icons, unless explictly instructed to do so.

## UI Text

Do not add superflous text to the UI describing how the internals of Strawberry function, how to interpret results, or anything else not required to make the UI clear and readable.

## Reviewing

- Make sure code is not being excessivley duplicated: if a method is defined to perform a task but is not being used in liue of custom code; flag this. This instruction, however, should not be interpreted to excess: DRY is only true up to a point and should not be taken as a maxim.
