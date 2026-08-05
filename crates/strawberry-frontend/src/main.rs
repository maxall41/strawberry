use anyhow::Result;
use commands::{
    OpenFile, PalettePaste, PalleteCommand, Quit, SaveImage, ToggleCommandPalette, ZoomIn, ZoomOut,
};
use gpui::{
    div, img, prelude::*, px, rgb, size, Animation, AnimationExt, App, AppContext, AssetSource,
    Bounds, Context, Entity, ExternalPaths, FocusHandle, Focusable, ImageSource, KeyBinding,
    KeyDownEvent, KeyUpEvent, Menu, MenuItem, ModifiersChangedEvent, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage, ScrollWheelEvent, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use gui::busy::{set_pipeline_busy, BusyOverlay};
use gui::feedback::FeedbackModal;
use gui::palette::{CommandPalette, PaletteDismissed};
use image::{Frame, RgbaImage};
use rfd::FileDialog;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use std::{fs, panic};
use strawberry_backend::Pipeline;
use strawberry_types::{
    overlay::{CircleStyle, OverlayCollection, OverlayElement},
    FeedbackLevel, FrontendMessage, PipelineCommand,
};
use tokio::sync::broadcast;

struct Assets {
    base: PathBuf,
}

mod commands;
mod gui;
mod tools;

const MOVEMENT_SPEED: f32 = 800.0;

const ACTIVITY_SHOW_DELAY: Duration = Duration::from_millis(300);
const ACTIVITY_LINGER: Duration = Duration::from_millis(250);

/// Busy-indicator state for one pipeline run.
struct PipelineActivity {
    label: String,
    stage: usize,
    total: usize,
    /// Flips once the run outlasts ACTIVITY_SHOW_DELAY.
    shown: bool,
    /// The run ended but the indicator is lingering; a new run's progress
    /// message clears this and takes the indicator over seamlessly.
    finishing: bool,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(std::borrow::Cow::Owned(data)))
            .map_err(|e| e.into())
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(|e| e.into())
    }
}

struct PrimaryViewer {
    focus_handle: FocusHandle,
    zoom_level: f32,
    original_width: u32,
    original_height: u32,
    pan_x: f32,
    pan_y: f32,
    pressed_keys: std::collections::HashSet<String>,
    command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    output_frame_receiver: tokio::sync::watch::Receiver<Option<strawberry_types::OutputFrame>>,
    display_image: ImageSource,
    img_coords: Arc<RwLock<Option<Bounds<Pixels>>>>,
    overlays: OverlayCollection,
    snap_target: Option<(f32, f32)>,
    toasts: Vec<Entity<FeedbackModal>>,
    last_viewport_size: Option<gpui::Size<Pixels>>,
    image_directory: Arc<RwLock<Option<PathBuf>>>,
    image_loaded: bool,
    pan_loop_running: bool,
    activity: Option<PipelineActivity>,
    activity_epoch: u64,
    busy_overlay: Entity<BusyOverlay>,
    palette: Option<Entity<CommandPalette>>,
    commands: Rc<[PalleteCommand]>,
}

impl PrimaryViewer {
    pub fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut receiver = self.command_receiver.resubscribe();
        let mut output_frames = self.output_frame_receiver.clone();

        // Request initial overlays
        if let Err(e) = self.command_sender.send(PipelineCommand::RequestOverlays) {
            eprintln!("Failed to request overlays: {e}");
        }

        cx.spawn_in(window, async move |this, cx| {
            let mut closed = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;

                let mut latest_output = None;
                let mut latest_overlays = None;
                let mut latest_snap = None;
                let mut toasts = Vec::new();
                let mut activity_events = Vec::new();

                loop {
                    match receiver.try_recv() {
                        Ok(PipelineCommand::FrontendMessage(msg)) => match msg {
                            FrontendMessage::UserFeedback(text, level) => {
                                toasts.push((text, level))
                            }
                            FrontendMessage::ReturnOverlays(overlays) => {
                                latest_overlays = Some(overlays)
                            }
                            FrontendMessage::SnapToPosition(x, y) => latest_snap = Some((x, y)),
                            FrontendMessage::PipelineProgress(progress) => {
                                activity_events.push(Some(progress))
                            }
                            FrontendMessage::PipelineFinished => activity_events.push(None),
                            _ => {}
                        },
                        Ok(_) => {}
                        Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                        Err(broadcast::error::TryRecvError::Empty) => break,
                        Err(broadcast::error::TryRecvError::Closed) => {
                            closed = true;
                            break;
                        }
                    }
                }

                if output_frames.has_changed().unwrap_or(false) {
                    latest_output = output_frames
                        .borrow_and_update()
                        .as_ref()
                        .map(|frame| frame.image.clone());
                }

                if latest_output.is_none()
                    && latest_overlays.is_none()
                    && latest_snap.is_none()
                    && toasts.is_empty()
                    && activity_events.is_empty()
                {
                    if closed {
                        break;
                    }
                    continue;
                }

                let result = this.update_in(cx, |this, window, cx| {
                    for (text, level) in toasts {
                        this.add_toast(text, level, cx);
                    }
                    for event in activity_events {
                        match event {
                            Some(progress) => this.pipeline_progress(progress, cx),
                            None => this.pipeline_finished(cx),
                        }
                    }
                    if let Some(image) = latest_output {
                        this.show_output(image, window, cx);
                    }
                    if let Some(overlays) = latest_overlays {
                        this.overlays = overlays;
                    }
                    if let Some((x, y)) = latest_snap {
                        this.snap_target = Some((x, y));
                    }
                    cx.notify();
                });

                // The only update failure is the viewer being gone (window
                // closed).
                if result.is_err() || closed {
                    break;
                }
            }
        })
        .detach();
    }

    fn show_output(&mut self, image: Arc<RgbaImage>, window: &mut Window, cx: &mut Context<Self>) {
        let img_data = Arc::new(RenderImage::new(vec![Frame::new(Arc::unwrap_or_clone(
            image,
        ))]));
        let new_width = img_data.size(0).width.0 as u32;
        let new_height = img_data.size(0).height.0 as u32;

        // Center the viewport on the new image unless its dimensions match the
        // current one (then it's just a reprocessed frame; keep the camera put).
        if new_width != self.original_width || new_height != self.original_height {
            if let Some(viewport) = self.last_viewport_size {
                self.pan_x = (viewport.width.to_f64() as f32 / 2.0)
                    - ((new_width as f32 / 2.0) * self.zoom_level);
                self.pan_y = (viewport.height.to_f64() as f32 / 2.0)
                    - ((new_height as f32 / 2.0) * self.zoom_level);
            }
        }

        self.original_width = new_width;
        self.original_height = new_height;
        self.image_loaded = true;
        let previous = std::mem::replace(&mut self.display_image, ImageSource::Render(img_data));

        if let ImageSource::Render(previous) = previous {
            if let Err(e) = window.drop_image(previous) {
                eprintln!("Failed to release previous frame from the sprite atlas: {e}");
            }
        }

        cx.notify();

        if let Err(e) = self.command_sender.send(PipelineCommand::RequestOverlays) {
            eprintln!("Failed to request overlays after image update: {e}");
        }
    }

    fn pipeline_progress(
        &mut self,
        progress: strawberry_types::PipelineProgress,
        cx: &mut Context<Self>,
    ) {
        let label = humanize_tool_name(&progress.name);
        if let Some(activity) = &mut self.activity {
            activity.label = label;
            activity.stage = progress.stage;
            activity.total = progress.total;
            activity.finishing = false;
            return;
        }

        self.activity = Some(PipelineActivity {
            label,
            stage: progress.stage,
            total: progress.total,
            shown: false,
            finishing: false,
        });
        self.activity_epoch += 1;
        let epoch = self.activity_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ACTIVITY_SHOW_DELAY).await;
            this.update(cx, |this, cx| {
                if this.activity_epoch != epoch {
                    return;
                }
                if let Some(activity) = &mut this.activity {
                    activity.shown = true;
                    set_pipeline_busy(cx, true);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn pipeline_finished(&mut self, cx: &mut Context<Self>) {
        let Some(activity) = &mut self.activity else {
            return;
        };
        if !activity.shown {
            self.activity = None;
            self.activity_epoch += 1;
            return;
        }

        activity.finishing = true;
        let epoch = self.activity_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ACTIVITY_LINGER).await;
            this.update(cx, |this, cx| {
                let still_finishing = this.activity.as_ref().is_some_and(|a| a.finishing);
                if this.activity_epoch == epoch && still_finishing {
                    this.activity = None;
                    this.activity_epoch += 1;
                    set_pipeline_busy(cx, false);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn add_toast(&mut self, text: String, level: FeedbackLevel, cx: &mut Context<Self>) {
        println!("Toast added {}", text);
        let modal = cx.new(|cx| {
            let modal = FeedbackModal::new(text, level);
            modal.enable_auto_close(cx);
            modal
        });
        self.toasts
            .retain(|toast| !toast.read(cx).marked_for_deletion);
        self.toasts.push(modal);
        cx.notify();
    }

    fn handle_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        let sender_p = self.command_sender.clone();
        let image_dir = self.image_directory.clone();

        cx.spawn_in(_window, async move |_, _| {
            let files = FileDialog::new()
                .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
                .set_directory("~/")
                .pick_files();

            if let Some(files) = files {
                if let Some(first_file) = files.first() {
                    if let Some(parent) = first_file.parent() {
                        if let Ok(mut dir) = image_dir.write() {
                            *dir = Some(parent.to_path_buf());
                        }
                    }
                }

                if let Err(e) = sender_p.send(PipelineCommand::Load(files)) {
                    eprintln!("Failed to send loaded images to pipeline: {e}");
                }
            }
        })
        .detach();
    }

    fn handle_save_image(&mut self, _: &SaveImage, _window: &mut Window, cx: &mut Context<Self>) {
        let sender_p = self.command_sender.clone();

        cx.spawn_in(_window, async move |_, _| {
            let output_file = FileDialog::new()
                .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
                .set_directory("~/")
                .save_file();

            if let Some(file) = output_file {
                if let Err(e) = sender_p.send(PipelineCommand::SaveOutput(file)) {
                    eprintln!("Failed to send save output command to pipeline: {e}");
                }
            }
        })
        .detach();
    }

    fn zoom_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level *= 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level /= 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn adjust_pan_for_center_zoom(&mut self, window: &mut Window, old_zoom: f32, new_zoom: f32) {
        let viewport = window.viewport_size();
        let center = Point::new(viewport.width / 2.0, viewport.height / 2.0);
        self.adjust_pan_around_point(center, old_zoom, new_zoom);
    }

    fn adjust_pan_around_point(&mut self, center: Point<Pixels>, old_zoom: f32, new_zoom: f32) {
        let center_x = center.x;
        let center_y = center.y;

        // Find the point in image coordinates that's currently at center
        let image_center_x = (center_x.to_f64() as f32 - self.pan_x) / old_zoom;
        let image_center_y = (center_y.to_f64() as f32 - self.pan_y) / old_zoom;

        // Adjust pan so this same point remains at center with new zoom
        self.pan_x = center_x.to_f64() as f32 - (image_center_x * new_zoom);
        self.pan_y = center_y.to_f64() as f32 - (image_center_y * new_zoom);
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta_y = match event.delta {
            gpui::ScrollDelta::Pixels(point) => point.y.to_f64() as f32,
            gpui::ScrollDelta::Lines(point) => point.y * 40.0,
        };

        if delta_y == 0.0 {
            return;
        }

        let old_zoom = self.zoom_level;
        let scale = 1.0035f32.powf(delta_y);
        self.zoom_level *= scale;

        // Clamp zoom level to reasonable bounds
        self.zoom_level = self.zoom_level.clamp(0.1, 50.0);

        self.adjust_pan_around_point(event.position, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn handle_zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_in(window, cx);
    }

    fn handle_zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_out(window, cx);
    }

    fn apply_held_pan(&mut self, dt: f32, cx: &mut Context<Self>) {
        let mut dx: f32 = 0.0;
        let mut dy: f32 = 0.0;

        if self.pressed_keys.contains("a") {
            dx += 1.0;
        }
        if self.pressed_keys.contains("d") {
            dx -= 1.0;
        }
        if self.pressed_keys.contains("w") {
            dy += 1.0;
        }
        if self.pressed_keys.contains("s") {
            dy -= 1.0;
        }

        let length = (dx * dx + dy * dy).sqrt();
        if length > 0.0 {
            // Normalize such that diagonal movement is not faster
            self.pan_x += dx / length * MOVEMENT_SPEED * dt;
            self.pan_y += dy / length * MOVEMENT_SPEED * dt;
            cx.notify();
        }
    }

    fn start_pan_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pan_loop_running {
            return;
        }
        self.pan_loop_running = true;

        cx.spawn_in(window, async move |this, cx| {
            let mut last_tick = std::time::Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;

                let now = std::time::Instant::now();
                let dt = now.duration_since(last_tick).as_secs_f32();
                last_tick = now;

                let done = this.update(cx, |this, cx| {
                    if this.pressed_keys.is_empty() {
                        this.pan_loop_running = false;
                        return true;
                    }
                    this.apply_held_pan(dt, cx);
                    false
                });

                match done {
                    Ok(false) => {}
                    _ => break,
                }
            }
        })
        .detach();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Don't handle keys as panning when pallete is open.
        if self.palette.is_some() {
            return;
        }
        if event.keystroke.modifiers.modified() {
            return;
        }
        let key = &event.keystroke.key;
        if matches!(key.as_str(), "w" | "a" | "s" | "d") {
            self.pressed_keys.insert(key.clone());
            self.start_pan_loop(window, cx);
        }
    }

    fn on_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if event.modifiers.modified() {
            self.pressed_keys.clear();
        }
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            cx.notify();
            return;
        }
        self.pressed_keys.clear();

        let commands = self.commands.clone();
        let palette = cx.new(|cx| CommandPalette::new(commands, cx));
        cx.subscribe(&palette, |viewer, _palette, _: &PaletteDismissed, cx| {
            viewer.palette = None;
            cx.notify();
        })
        .detach();

        window.focus(&palette.read(cx).focus_handle(cx), cx);
        self.palette = Some(palette);
        cx.notify();
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        let key = &event.keystroke.key;
        if matches!(key.as_str(), "w" | "a" | "s" | "d") {
            self.pressed_keys.remove(key);
        }
    }

    // Helper method to convert screen coordinates to image coordinates
    fn screen_to_image_coords(&self, screen_x: f32, screen_y: f32) -> Option<(u64, u64)> {
        if let Some(coords) = *self
            .img_coords
            .read()
            .expect("Image coordinates lock should not be poisoned")
        {
            let relative_x =
                (screen_x - coords.origin.x.to_f64() as f32) / coords.size.width.to_f64() as f32;
            let relative_y =
                (screen_y - coords.origin.y.to_f64() as f32) / coords.size.height.to_f64() as f32;

            if (0.0..=1.0).contains(&relative_x) && (0.0..=1.0).contains(&relative_y) {
                let image_pixel_x = (relative_x * self.original_width as f32) as u64;
                let image_pixel_y = (relative_y * self.original_height as f32) as u64;
                Some((image_pixel_x, image_pixel_y))
            } else {
                None
            }
        } else {
            None
        }
    }

    // Helper to convert image coords to screen coords
    fn image_to_screen_coords(&self, img_x: f32, img_y: f32) -> (f32, f32) {
        (
            img_x * self.zoom_level + self.pan_x,
            img_y * self.zoom_level + self.pan_y,
        )
    }

    // Render a single overlay element
    fn render_overlay_element(&self, element: &OverlayElement) -> impl IntoElement {
        match element {
            OverlayElement::Circle {
                x,
                y,
                radius,
                color,
                style,
            } => {
                let (screen_x, screen_y) = self.image_to_screen_coords(*x, *y);
                let screen_radius = radius * self.zoom_level;
                let gpui_color =
                    rgb((color[0] as u32) << 16 | (color[1] as u32) << 8 | color[2] as u32);

                match style {
                    CircleStyle::Border { width } => {
                        let screen_width = width * self.zoom_level;
                        div()
                            .absolute()
                            .left(px(screen_x - screen_radius))
                            .top(px(screen_y - screen_radius))
                            .w(px(screen_radius * 2.0))
                            .h(px(screen_radius * 2.0))
                            .rounded_full()
                            .border(px(screen_width))
                            .border_color(gpui_color)
                    }
                    CircleStyle::Filled => div()
                        .absolute()
                        .left(px(screen_x - screen_radius))
                        .top(px(screen_y - screen_radius))
                        .w(px(screen_radius * 2.0))
                        .h(px(screen_radius * 2.0))
                        .rounded_full()
                        .bg(gpui_color),
                    CircleStyle::Selected { width } => {
                        let screen_width = width * self.zoom_level;
                        div()
                            .absolute()
                            .left(px(screen_x - screen_radius))
                            .top(px(screen_y - screen_radius))
                            .w(px(screen_radius * 2.0))
                            .h(px(screen_radius * 2.0))
                            .rounded_full()
                            .border(px(screen_width))
                            .border_color(gpui_color)
                    }
                }
            }
            OverlayElement::Point {
                x,
                y,
                radius,
                color,
            } => {
                let (screen_x, screen_y) = self.image_to_screen_coords(*x, *y);
                let screen_radius = radius * self.zoom_level;
                let gpui_color =
                    rgb((color[0] as u32) << 16 | (color[1] as u32) << 8 | color[2] as u32);

                div()
                    .absolute()
                    .left(px(screen_x - screen_radius))
                    .top(px(screen_y - screen_radius))
                    .w(px(screen_radius * 2.0))
                    .h(px(screen_radius * 2.0))
                    .rounded_full()
                    .bg(gpui_color)
            }
            OverlayElement::Polyline {
                points,
                color,
                stroke_width,
            } => {
                if points.is_empty() {
                    return div();
                }

                let screen_stroke = stroke_width * self.zoom_level;

                // Convert image coordinates to screen coordinates
                let screen_points: Vec<(f32, f32)> = points
                    .iter()
                    .map(|(x, y)| self.image_to_screen_coords(*x, *y))
                    .collect();

                // Convert RGBA array to gpui::Rgba
                let gpui_color =
                    rgb((color[0] as u32) << 16 | (color[1] as u32) << 8 | color[2] as u32);

                gui::polyline::Polyline::new(screen_points, gpui_color, px(screen_stroke))
                    .into_element()
            }
            _ => div(), // Placeholder for other element types not yet implemented
        }
    }
}

impl Focusable for PrimaryViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PrimaryViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_viewport_size = Some(window.viewport_size());

        // Handle snap if needed
        if let Some((x, y)) = self.snap_target.take() {
            let viewport = window.viewport_size();
            let viewport_center_x = viewport.width / 2.0;
            let viewport_center_y = viewport.height / 2.0;

            // Calculate pan to center the position
            self.pan_x = viewport_center_x.to_f64() as f32 - (x * self.zoom_level);
            self.pan_y = viewport_center_y.to_f64() as f32 - (y * self.zoom_level);
        }

        // Keep the canvas focused for pan/zoom keys as long as command palette is not open.
        if self.palette.is_none() {
            window.focus(&self.focus_handle(cx), cx);
        }

        let activity_status = self.activity.as_ref().filter(|a| a.shown).map(|a| {
            if a.total > 1 {
                format!("{} · {}/{}", a.label, a.stage + 1, a.total)
            } else {
                format!("{}…", a.label)
            }
        });

        let mut _img = img(self.display_image.clone())
            .use_nearest_filter(true)
            .w(px(self.original_width as f32 * self.zoom_level))
            .h(px(self.original_height as f32 * self.zoom_level));

        let coords = _img.coords.clone();
        self.img_coords = coords.clone();

        let mouse_down_sender = self.command_sender.clone();

        let mut _img = _img.on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener({
                move |this: &mut PrimaryViewer,
                      event: &MouseDownEvent,
                      _window: &mut Window,
                      _: &mut Context<PrimaryViewer>| {
                    let mouse_x = event.position.x;
                    let mouse_y = event.position.y;

                    if let Some((image_x, image_y)) = this
                        .screen_to_image_coords(mouse_x.to_f64() as f32, mouse_y.to_f64() as f32)
                    {
                        // Send both interaction start (for lasso) and click (for place/delete)
                        if let Err(e) = mouse_down_sender
                            .send(PipelineCommand::InteractionStart(image_x, image_y))
                        {
                            eprintln!("Error sending interaction start: {e}");
                        }
                        if let Err(e) =
                            mouse_down_sender.send(PipelineCommand::HandleClick(image_x, image_y))
                        {
                            eprintln!("Error sending click event: {e}");
                        }
                    } else {
                        eprintln!("Could not convert screen coordinates to image coordinates");
                    }
                }
            }),
        );

        _img = _img.on_mouse_move(cx.listener(
            move |this: &mut PrimaryViewer,
                  event: &MouseMoveEvent,
                  _window: &mut Window,
                  _cx: &mut Context<PrimaryViewer>| {
                let mouse_x = event.position.x;
                let mouse_y = event.position.y;

                if let Some((image_x, image_y)) =
                    this.screen_to_image_coords(mouse_x.to_f64() as f32, mouse_y.to_f64() as f32)
                {
                    let _ = this
                        .command_sender
                        .send(PipelineCommand::InteractionMove(image_x, image_y));
                }
            },
        ));

        let mut _img = _img.on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(
                move |this: &mut PrimaryViewer,
                      _event: &MouseUpEvent,
                      _window: &mut Window,
                      _cx: &mut Context<PrimaryViewer>| {
                    if let Err(e) = this.command_sender.send(PipelineCommand::InteractionEnd) {
                        eprintln!("Error sending interaction end: {e}");
                    }
                },
            ),
        );
        div()
            .id("main")
            .key_context("PrimaryViewer")
            .track_focus(&self.focus_handle(cx))
            .focusable()
            .size_full()
            .bg(gui::theme::CANVAS)
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_action(cx.listener(Self::handle_zoom_in))
            .on_action(cx.listener(Self::handle_zoom_out))
            .on_action(cx.listener(Self::handle_open_file))
            .on_action(cx.listener(Self::handle_save_image))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_drop(cx.listener(
                |this: &mut PrimaryViewer,
                 paths: &ExternalPaths,
                 _window: &mut Window,
                 _cx: &mut Context<PrimaryViewer>| {
                    if let Err(e) = this
                        .command_sender
                        .send(PipelineCommand::Load(paths.paths().to_vec()))
                    {
                        eprintln!("Failed to send loaded images to pipeline: {e}");
                    }
                },
            ))
            .when(self.image_loaded, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(self.pan_x))
                        .top(px(self.pan_y))
                        .child(_img),
                )
            })
            .when(!self.image_loaded, |el| {
                el.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_color(gui::theme::TEXT_MUTED)
                        .child(div().text_lg().child("Drop images here"))
                        .child(div().text_sm().child("or press ⌘L to open files")),
                )
            })
            .children(
                self.toasts
                    .iter()
                    .enumerate()
                    .map(|(index, toast)| {
                        div()
                            .absolute()
                            .bottom(px(20.0 + (index as f32 * 90.0)))
                            .right(px(20.0))
                            .child(toast.clone())
                    })
                    .collect::<Vec<_>>(),
            )
            .children(
                self.overlays
                    .all_elements()
                    .map(|element| self.render_overlay_element(element))
                    .collect::<Vec<_>>(),
            )
            .child(self.busy_overlay.clone())
            .when_some(activity_status, |el, status| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(3.))
                        .bg(gpui::Rgba {
                            a: 0.5,
                            ..gui::theme::SURFACE_DEEP
                        })
                        .child(
                            div().size_full().bg(gui::theme::ACCENT).with_animation(
                                "pipeline-busy-pulse",
                                Animation::new(Duration::from_millis(1500))
                                    .repeat()
                                    .with_easing(gpui::pulsating_between(0.3, 1.0)),
                                |el, delta| el.opacity(delta),
                            ),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(9.))
                        .right(px(10.))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(gpui::Rgba {
                            a: 0.85,
                            ..gui::theme::CANVAS
                        })
                        .text_xs()
                        .text_color(gui::theme::TEXT_MUTED)
                        .child(status),
                )
            })
            // Pallete added as last child to ensure that it paints above other elements.
            .when_some(self.palette.clone(), |el, palette| el.child(palette))
    }
}

/// "RollingBall" (a `BackendToolType` debug name) -> "Rolling Ball".
fn humanize_tool_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            result.push(' ');
        }
        result.push(c);
    }
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let (mut pipeline, view_sender, view_recv, output_frames) = Pipeline::new().await?;

    let pipeline_handle = tokio::spawn(async move {
        loop {
            if let Err(e) = pipeline.process_commands().await {
                eprintln!("Error processing pipeline commands: {e}");
                panic!("Pipeline processing failed: {e}");
            }
            tokio::time::sleep(Duration::from_millis(33)).await; // Lock at 30FPS
        }
    });

    // Spawn a task to monitor the pipeline and propagate panics
    tokio::spawn(async move {
        if let Err(e) = pipeline_handle.await {
            if e.is_panic() {
                eprintln!("Pipeline task panicked, shutting down application");
                panic::resume_unwind(e.into_panic());
            }
        }
    });

    let image_directory: Arc<RwLock<Option<PathBuf>>> = Arc::new(RwLock::new(None));

    gpui_platform::application()
        .with_assets(Assets {
            base: manifest_dir.join("examples"),
        })
        .run(move |cx: &mut App| {
            let commands = commands::install(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                image_directory.clone(),
            );

            cx.on_action(|_: &Quit, cx| cx.quit());

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-l", OpenFile, None),
                KeyBinding::new("cmd-s", SaveImage, None),
                KeyBinding::new("secondary-p", ToggleCommandPalette, None),
                KeyBinding::new("secondary-v", PalettePaste, Some("CommandPalette")),
                KeyBinding::new("+", ZoomIn, Some("PrimaryViewer && !CommandPalette")),
                KeyBinding::new("-", ZoomOut, Some("PrimaryViewer && !CommandPalette")),
            ]);

            cx.set_menus(vec![
                Menu::new("Strawberry").items([
                    MenuItem::action("Command Palette…", ToggleCommandPalette),
                    MenuItem::separator(),
                    MenuItem::action("Quit", Quit),
                ]),
                Menu::new("File").items([
                    MenuItem::action("Load Images...", OpenFile),
                    MenuItem::action("Save Image", SaveImage),
                ]),
            ]);

            let window_options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Strawberry")),
                    appears_transparent: false,
                    ..Default::default()
                }),

                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    size: size(px(1100.), px(600.)),
                    origin: Point::new(px(200.), px(200.)),
                })),

                ..Default::default()
            };

            // Create a default 800x600 black image
            let width = 800u32;
            let height = 600u32;
            let mut data = RgbaImage::new(width, height);

            // Fill with black pixels (BGRA format for GPUI)
            for pixel in data.chunks_exact_mut(4) {
                pixel[0] = 0; // B
                pixel[1] = 0; // G
                pixel[2] = 0; // R
                pixel[3] = 255; // A
            }

            let frame = Frame::new(data);
            let img_frames = vec![frame];
            let img_data = Arc::new(RenderImage::new(img_frames));

            let primary_viewer_image_dir = image_directory.clone();
            let main_window = cx.open_window(window_options, |window, cx| {
                let mut raw_primary_viewer = PrimaryViewer {
                    focus_handle: cx.focus_handle(),
                    zoom_level: 1.0,
                    original_width: 800,
                    original_height: 600,
                    pan_x: 0.0,
                    pan_y: 0.0,
                    pressed_keys: std::collections::HashSet::new(),
                    command_receiver: view_recv.resubscribe(),
                    command_sender: view_sender.clone(),
                    output_frame_receiver: output_frames.clone(),
                    display_image: ImageSource::Render(img_data),
                    img_coords: Arc::new(RwLock::new(None)),
                    overlays: OverlayCollection::new(),
                    snap_target: None,
                    toasts: vec![],
                    last_viewport_size: None,
                    image_directory: primary_viewer_image_dir,
                    image_loaded: false,
                    pan_loop_running: false,
                    activity: None,
                    activity_epoch: 0,
                    busy_overlay: cx.new(|cx| BusyOverlay::new(false, cx)),
                    palette: None,
                    commands,
                };

                let viewer = cx.new(|cx| {
                    raw_primary_viewer.process_messages(window, cx);
                    raw_primary_viewer
                });

                // Bind keys locally to the viewer
                cx.bind_keys([
                    KeyBinding::new("=", ZoomIn, Some("PrimaryViewer && !CommandPalette")),
                    KeyBinding::new("-", ZoomOut, Some("PrimaryViewer && !CommandPalette")),
                ]);

                // Focus the viewer so it can receive key events
                window.focus(&viewer.read(cx).focus_handle(cx), cx);

                viewer
            });

            let main_window = match main_window {
                Ok(window) => window,
                Err(e) => {
                    eprintln!("Failed to open main window: {e}");
                    return;
                }
            };

            cx.on_action(move |_: &ToggleCommandPalette, cx| {
                cx.defer(move |cx| {
                    if let Err(e) = main_window.update(cx, |viewer, window, cx| {
                        window.activate_window();
                        viewer.toggle_palette(window, cx);
                    }) {
                        eprintln!("Failed to toggle command palette: {e}");
                    }
                });
            });

            let main_window_id = main_window.window_id();
            cx.on_window_closed(move |cx, window_id| {
                if window_id == main_window_id {
                    cx.quit();
                }
            })
            .detach();

            // Activate the app after window creation
            cx.activate(true);
        });

    Ok(())
}
