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

# Tests (tiling, pipeline tool management, sidebar ordering/layout, CLI parsing, plane
# offloading, and the memory probe)
cargo test --workspace
cargo test -p strawberry-processing tiling   # run a single test module

# Offload every cached plane that is not in use, whatever memory looks like; `never` turns it off
STRAWBERRY_OFFLOAD=always cargo run
```

## Workspace layout

Six crates share the processing engine. Dependencies flow from the frontend and CLI through the backend to shared types and processing; crates also depend directly on lower layers where needed. `strawberry-frontend` is the default workspace member.

- **`crates/strawberry-memory`** — reads physical memory and free disk space from the operating system (`src/macos.rs`, `src/windows.rs`, `src/linux.rs`). It depends on no other workspace crate and is the only one that may contain `unsafe`, which the workspace lints deny everywhere else.

- **`crates/strawberry-processing`** — image algorithms and GPU infrastructure. `src/lib.rs` owns `GpuImageProcessor`, checked GPU operations, buffers, and readback; operation modules use WGSL in `src/shaders/` and shared tiling helpers. Also contains CPU nerve tracing and segmentation feature extraction, model training, and inference. Pipeline state and frontend controls live in other crates.
- **`crates/strawberry-types`** — the shared protocol and state. `src/messages.rs` defines commands, replies, events, and output frames; `src/types.rs` defines tool kinds, instance IDs, pipeline summaries, and processing requests; `src/state.rs` defines tool settings, persistence, and validation. `src/images.rs` defines hyperstacks and `src/plane.rs` the `Plane` they are made of, with the `SpillFile` its pixels can move to. Backend/frontend traits and canvas overlays are in `src/traits.rs` and `src/overlay.rs`.
- **`crates/strawberry-backend`** — owns the mutable `Pipeline` in `src/lib.rs`. `src/tools/manager.rs` creates and orders tool instances; other tool modules implement `ToolBackend`, using shared lifecycle code in `base.rs` and `cache.rs`. `src/segmentation/` coordinates annotations, datasets, predictions, and training; `src/counter/` and `src/nervetracer/` hold counting and trace I/O helpers. `src/offload.rs` judges memory pressure and moves cached planes to disk. `src/utils/pipeline_io.rs` reads and writes pipeline archives.
- **`crates/strawberry-frontend`** — the GPUI desktop app. `src/main.rs` hosts `PrimaryViewer`, image interaction, overlays, and feedback. `src/commands.rs` registers tools and palette actions; `src/tools/` contains tool views and state subscriptions. `src/gui/` contains the pipeline sidebar, workspace layout, pickers, and shared controls, icons, and theme.
- **`crates/strawberry-cli`** — headless batch processing with file filtering and grouping. It drives the same backend with `LoadPipeline { cli: true, .. }`, `Load`, and `SaveOutput`, using command replies and output-frame generations to track completion.

Preserve the custom dependencies: `image` uses the `maxall41/image` single-channel-support branch, GPUI packages use a pinned revision of `maxall41/gpui-m`, and `smartcore` uses the `maxall41/smartcore` parallel branch. Do not replace these forks with upstream packages.

## Architecture

**Ownership and communication:** the backend owns mutable tool state; frontend views maintain local snapshots. All frontend and CLI mutations enter one ordered tokio unbounded `mpsc` queue as `PipelineCommand`s. Commands requiring a result carry `oneshot` replies. `SubscribePipeline` and `SubscribeTool` return `watch` receivers for current pipeline and tool state; the displayed plane uses a separate latest-only `watch` channel carrying `OutputFrame { load_generation, coordinates, shape, image, is_current }`. Transient feedback, progress, tool events, and overlays use a `broadcast` channel. The backend polls `process_commands()` about every 33 ms; tool views await watched state changes.

**Hyperstacks:** loaded images form an `ImageCollection`, an `Array5<Arc<Plane>>` stored in T, Z, C, Y, X order. A `Plane` is identified by its address; its pixels are an `RgbaImage` that `Plane::load` returns, reading it back from disk if it was offloaded. `Coordinates` address one plane; a `Position` is the X, Y, Z, T part shared by every channel there. X and Y number mosaic tiles, not pixels. `Load` places files by an `ImageArrangement`: as the channels of one image, as a Z stack, or at explicit hyperstack coordinates produced by `HyperstackLayout` from a Rhai parser script (`hyperstack/`). `synthesize_parser_template` infers a starting script from the file names, falling back to a Z stack in name order. Positions are fixed at load; tools may change only the channel count, as the Compositor does by merging channels into one. The viewer shows one plane; the hyperstack bar sends `SelectView { coordinates, load_generation }`, and requests made for an earlier load are ignored.

**Tool instances and sidebar:** `ToolManager` stores an ordered `IndexMap<ToolId, ToolInstance>`. A `ToolId` identifies one instance, so multiple tools of the same `BackendToolType` can coexist. `AddTool` constructs, enables, appends, and selects an instance; `RemoveTool` destroys it. Disabling preserves its settings and stack position. `MoveTool { id, before }` reorders by identity, with `None` meaning the end. The sidebar renders all instances, including disabled ones, from watched `PipelineState`. Controls initialize when expanded, survive collapse, and are released when their instance disappears.

**Processing and reconciliation:** each enabled tool processes the whole `ImageCollection` in manager order. At each processing boundary, `flush_processing` polls background jobs, reconciles enabled tool order, and reconciles tool settings. `PendingProcessing` coalesces requests at the earliest affected index; saving output first flushes pending work. `ToolCache` keys each stage's output on its settings, its input shape, and the identity of each input plane, and keeps the last reconciled settings separately; `SyncState::processing_differs` determines which changes require image processing. Publishing state to a frontend does not acknowledge processing. `process_from` restores the valid cached prefix, lets bypassed tools drop retained input whose upstream changed (`reconcile_bypassed_input`), and caches each successful stage. A failed stage leaves later caches alone, since they cannot match a different input. Stages that work plane by plane report progress with `PlaneProgressExt::report_progress`, one plane per output plane. Tools computing overlays in the background report `has_pending_overlay`, which the viewer shows as pipeline activity. The fallback compositor is used only to export the composite of the displayed position's channels. When a stage recomputes, `release_unreachable_caches` drops the caches of later stages whose input planes no longer exist, since nothing can match them again.

**Memory offloading:** when physical memory runs short, `PlaneOffloader` moves the pixels of cached planes into one temporary `SpillFile`, without changing which planes they are, so caches, annotations, and predictions keyed on a plane keep matching. Pixels leave memory only while the plane holds the sole `Arc<RgbaImage>`; anything holding loaded pixels, such as the displayed frame or a stage mid-plane, pins them. `Pipeline::rank_offload_candidates` orders planes coldest first: disabled tools' caches, then stages by distance from the images the edited tool reads (`edit_point`), then planes far from the viewed position. The planes read by the edited tool and by the stage in flight, and those at the viewed position, only leave under critical pressure. Offloading runs in short slices after each batch of commands, per plane while a stage processes, and per plane while every view is saved. The first release in a spell of pressure sends a `FeedbackLevel::Warning` toast; warnings never fail a CLI run. A plane's disk copy is freed when the plane is dropped, and the file shrinks back as its tail empties. Pressure is judged from `strawberry-memory`, which counts compressed memory at the RAM it occupies. The output of the stage in flight is in no cache yet, so one stack must still fit in memory.

**GPU failures and cache lifetime:** GPU construction, submission, and readback use the checked helpers on `GpuImageProcessor` and return errors. Uncaptured GPU errors clear `output_is_current` and invalidate cached outputs before being reported or propagated. The viewer may retain the last successful frame, but saving requires current output. `invalidate_cached_output` must preserve CPU input needed for canvas interaction; `invalidate_cache` performs full invalidation when inputs or processing state change. Nerve tracing uses this distinction to keep clicks working after a failure without reusing failed pipeline output.

**Pixel format:** pipeline images contain **BGRA** bytes even though their Rust type is `RgbaImage`. `load_images` swaps red and blue for every loaded image, including multi-image loads. Processing and compositor/decomposer code must preserve that convention. `save_output` uses `GpuColorspaceConverter` to convert back to RGBA before writing a file.

**Canvas interaction and overlays:** the viewer converts screen positions to image pixel coordinates and sends `HandleClick` and `InteractionStart`/`InteractionMove`/`InteractionEnd` commands. Clicks go to the selected enabled instance. Gestures capture their owning `ToolId` at the start; changing selection, disabling, or removing that instance cancels routing. Tools handle `ToolInput`, expose `ToolOverlay`s, and signal overlay changes through state publication or `take_overlay_changes`. The pipeline publishes merged overlays for enabled tools; the viewer also requests them after displaying a new frame.

**Per-position tools:** the pipeline's view is the single source of the selected position. `Pipeline::set_view` passes a changed position to every tool through `select_position` and ends gestures begun on the previous position. The counter and nerve tracer keep annotations in a `PerPosition` map, and segmentation annotates the selected position's input. On load, `replace_input` clears per-position annotations and records the loaded shape, which annotation files use to number positions: depth slices count through time points, and only the selected tile is exported.

**Pipeline persistence:** `.sbp` files are versioned ZIP archives containing `pipeline.toml` and optional assets under `tools/<index>/<name>`. The TOML stores an ordered list of tagged `ToolState` entries, including disabled and repeated tool kinds. Runtime `ToolId`s are not persisted, nor are counts and traces, which have their own XML files. Loading builds detached candidates, loads settings and assets, validates them, and replaces the live stack only after every entry succeeds. `#[serde(skip)]` marks runtime-only state; in-place deserialization preserves constructor values for those fields. Renamed settings keep their old names as `#[serde(alias)]`es. The CLI loads the same archive format and uses `OutputFrame::load_generation` to match results to each image group.

## Adding a new processing tool

Use Sobel as a reference for a simple GPU filter. Paths below are relative to `crates/`.

1. **Implement the algorithm.** Add `strawberry-processing/src/<tool>.rs`, any WGSL under `src/shaders/`, and a module export in processing `src/lib.rs`. Reuse `GpuImageProcessor` helpers for checked resource creation, dispatch, submission, and readback. Preserve BGRA layout, handle GPU limits with the shared tiling helpers where needed, and propagate failures with `Result`.
2. **Define the protocol and settings.** Add a `BackendToolType` variant in `strawberry-types/src/types.rs`, a settings struct and `ToolState` variant in `src/state.rs`, and any required per-tool message enum and `PipelineObjectMessage` variant in `src/messages.rs`. Settings should support cloning, equality, defaults, and serde; keep runtime-only fields out of archives with `#[serde(skip)]`. Validate parameter ranges in message handling and add archive validation in `ToolState::validate` where applicable. Keep the tool kind and `<Tool>State` tag consistent with persistence loading.
3. **Implement and register the backend.** Add `strawberry-backend/src/tools/<tool>.rs`, export it through `src/tools/mod.rs`, and add its constructor to `ToolManager::create` in `src/tools/manager.rs`. Implement `ToolBackend` using `ToolBase`, `ToolCache`, and `impl_tool_backend_basics!` when suitable. Read each plane's pixels with `Plane::load` and replace it with `Plane::new`; keep `Arc<Plane>`, not loaded pixels, between runs, because held pixels cannot be offloaded. Register the settings in `processing_states!` in `src/tools/cache.rs`, or implement `SyncState` explicitly when only some settings affect processing. Interactive tools must keep output-cache invalidation separate from clearing the input used for clicks or gestures.
4. **Wire archive loading.** Add the serialized state tag to `LoadedEntry::kind` in `strawberry-backend/src/utils/pipeline_io.rs`. Expose settings through `persisted_state_mut` (provided by the backend basics macro). Tools with embedded files or special loading behavior should implement `pipeline_assets` and the relevant loading hooks on `ToolBackend`; construct and validate candidates before changing the live pipeline.
5. **Implement the frontend view.** Add `strawberry-frontend/src/tools/<tool>.rs` and export it in `src/tools/mod.rs`. Follow an existing view's `ToolFrontend`, `FrontendMessageHandler`, `Render`, and `Focusable` implementations. Accept the instance `ToolId` and `CommandSender`, use frontend `ToolBase::connect` and `spawn_message_processor` for watched state, and send edits through `ToolBase`. Build controls from the shared GUI widgets and theme.
6. **Register the UI entry points.** In `strawberry-frontend/src/commands.rs`, add the action to the tool `actions!` list and a `register_tool` call in `install`, supplying the label, `BackendToolType`, and view constructor. This registers the sidebar factory, Add Tool choice, and command-palette action together.

Verify processing, archive round trips, multiple instances of the new tool, and enabled/disabled behavior. For a new UI, render it with the GPUI test fixtures and inspect a screenshot for consistency with existing controls (see UI screenshots below). Follow the Unit Tests and Clippy rules below.

## UI screenshots

GPUI's `VisualTestAppContext` renders views offscreen with the real Metal renderer and captures them as images. AppKit must run on the main thread, and libtest runs every test on a spawned thread (the process aborts with "Rust cannot catch foreign exceptions"), so render from a temporary binary instead of a `#[test]`:

1. In `crates/strawberry-frontend/Cargo.toml`, temporarily add `"test-support"` to the features of both `gpui` and `gpui_platform` (the `gpui_platform` feature enables Metal capture; without it capture fails with "render_to_image not implemented for this platform"). Declare the app explicitly as `[[bin]] name = "strawberry-frontend"`, `path = "src/main.rs"`, and add a second `[[bin]]` with `test = false` pointing at a temporary file such as `src/screenshot_tmp.rs`.
2. In that file, include the frontend modules with `#[path = "commands.rs"] mod commands;`, `#[path = "gui/mod.rs"] mod gui;` and `#[path = "tools/mod.rs"] mod tools;`, so `crate::gui` and `crate::tools` paths resolve. Allow dead code and lints for the file.
3. In `main`, create `VisualTestAppContext::with_asset_source(gpui_platform::current_platform(false), Arc::new(assets))`, with an `AssetSource` that reads the requested path from disk (icon paths are absolute). Build a small root view containing the new UI next to an existing control for comparison. Open it with `cx.open_offscreen_window(size, |_, cx| cx.new(...))`, call `cx.run_until_parked()`, and save `cx.capture_screenshot(window.into())` as a PNG in the scratchpad. Captures are at 2x scale.
4. Drive interactions with `simulate_click`, `simulate_mouse_down`, `simulate_mouse_move`, `simulate_mouse_up` and `simulate_keystrokes` (positions are logical window pixels). Read view state back with `window.update(&mut cx, ...)` and capture again to check the result.
5. Run it with `cargo run -p strawberry-frontend --bin <name>` and view the PNG. Afterwards, delete the file and revert `Cargo.toml`; running `cargo check --workspace` then restores `Cargo.lock`.

## Reviewing

- Make sure code is not being excessivley duplicated: if a method is defined to perform a task but is not being used in liue of custom code; flag this. This instruction, however, should not be interpreted to excess: DRY is only true up to a point and should not be taken as a maxim.

- Do not review simply to find bugs; also review to find unnecessary abstraction, locations where abstraction should be used, locations where different data structures would be simpler/more elegant/eloquent/superior--and, generally, for unnecessary code and areas where architecture can be improved.

- Analyze frontend UI layout across components to ensure consistency of UI design idioms. Use the gpui test fixtures to render a new UI to a screenshot when implementing a new ui.

- On Minimalism: consider where code may be significantly simplified by use of an external library, the removal of superflous guard statements, removal of abstraction, addition of abstraction, changes in data types, changes in trait structures, or in any other manner. 

- Decoherence: you must review for decoherence; this is a phenomena in which patterns which make sense in isolation within a code base become nonsensical or overcomplicated when combined. For instance, some code performing thresholding on a f32 between 0 and 1, while another part of the code performs thresholding on a u8 between 0 and 255 is one example of decoherence. 

## Antipatterns

### Excessive BTreeMaps

Do not use a BTreeMap unless determinstic ordering is desired; use a regular HashMap instead for simplicity.

### Returning tuples of like types from functions

Functions should (almost) never return a tuple like (String, String) or (f64, f64, f64), as it is trivial for these to get mixed up, and the contract is not well defined.

### Not utilizing type-driven design

Invalid states should be unrepresentable in code by design. That is, types should be designed and used wherever possible and reasonable without introducing excessive complexity to prevent bugs that may arise from representing impossible states. It is better, for instance, to use a custom type to represent an Email rather than a String, as a custom type can enforce requirements like the validity of the email. Second example: representing validated users with a seperate type, such as ValidatedUser, than unvalidated users, User, such that only functions defined on the ValidatedUser struct can be called. 

### Excessivley long functions

Very long functions should be broken up into smaller components which are easier to reason about. Contracts between functions should be well defined and represented in the type system.

## Imports

Avoid using methods with their full qualifiers like `std::collections::HashSet::new();`. Instead, import such that they can be used like `HashSet::new();`. Furthermore, avoid the use of the `crate::` qualifier where possible. Never import inline inside a function, unless absolutely required.

## Clippy

Check all clippy lints and fix issues before finishing.

## Naming

All functions should be clearly named---in such a way that the intent and purpose of the function is clear from the name---and should contain a verb. Examples:

BAD: `check_gpu_state`
GOOD: `ensure_no_stage_gpu_errors` -- this name makes the purpose of the function much clearer than `check_gpu_state`, as `check_gpu_state` could mean almost anything.

## Comments

All functions should have a one to two line doc comment simply explaining their function. This comment should be clear and simply worded, while not being reductive. Multiple revisions of comments advised.

## Unit Tests

You may only add unit tests transitivley; they must be removed after you are done testing, unless otherwise informed.

## Readme & Markdown

Never edit the README, never create new markdown files/documentation.

## SVG and Icons

Do NOT write custom SVG icons. Use phosphor icons instead: https://phosphoricons.com/. Do not use text as icons, unless explictly instructed to do so.

## UI Text

Do not add superflous text to the UI describing how the internals of Strawberry function, how to interpret results, or anything else not required to make the UI clear and readable.
