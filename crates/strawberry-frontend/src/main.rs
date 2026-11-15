// wgpu auto-trait checks exceed the default compiler recursion depth.
#![recursion_limit = "256"]

use crate::gui::{Button, ButtonVariant};

use num_traits::ToPrimitive;

use anyhow::Result;
use commands::{
    OpenFile, PalleteCommand, Quit, SaveImage, ToggleCommandPalette, ToggleSidebar, ZoomFit,
    ZoomIn, ZoomOut,
};
use gpui::{
    Animation, AnimationExt, App, AppContext, AssetSource, Bounds, Context, Entity, ExternalPaths,
    FocusHandle, Focusable, ImageSource, KeyBinding, KeyDownEvent, KeyUpEvent, Menu, MenuItem,
    ModifiersChangedEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    RenderImage, ScrollWheelEvent, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, img, prelude::*, px, size,
};
use gui::busy::{BusyOverlay, set_pipeline_busy};
use gui::feedback::FeedbackModal;
use gui::generic_picker::PickerPaste;
use gui::icon::Icon;
use gui::palette::{CommandPalette, PaletteDismissed};
use gui::sidebar::{ToolSelected, ToolSidebar};
use gui::workspace::{
    SIDEBAR_GRIP_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, TOOLBAR_HEIGHT, WorkspaceLayout,
};
use image::{Frame, RgbaImage};
use rfd::FileDialog;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use std::{fs, panic};
use strawberry_backend::Pipeline;
use strawberry_types::{
    FeedbackLevel, FrontendMessage, PipelineCommand,
    overlay::{CircleStyle, OverlayCollection, OverlayElement},
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
    /// Flips once the run outlasts `ACTIVITY_SHOW_DELAY`.
    shown: bool,
    /// The run ended but the indicator is lingering; a new run's progress
    /// message clears this and takes the indicator over seamlessly.
    finishing: bool,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(std::borrow::Cow::Owned(data)))
            .map_err(std::convert::Into::into)
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
            .map_err(std::convert::Into::into)
    }
}

struct PrimaryViewer {
    focus_handle: FocusHandle,
    zoom_level: f32,
    original_width: u32,
    original_height: u32,
    pan_x: f32,
    pan_y: f32,
    pressed_keys: HashSet<String>,
    command_receiver: tokio::sync::broadcast::Receiver<FrontendMessage>,
    command_sender: strawberry_types::CommandSender,
    output_frame_receiver: tokio::sync::watch::Receiver<Option<strawberry_types::OutputFrame>>,
    display_image: ImageSource,
    img_coords: Arc<RwLock<Option<Bounds<Pixels>>>>,
    overlays: OverlayCollection,
    annotation_images: HashMap<usize, (Arc<RgbaImage>, Arc<RenderImage>)>,
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
    sidebar: Entity<ToolSidebar>,
    layout: WorkspaceLayout,
}

impl PrimaryViewer {
    fn canvas_size(&self, window: &Window) -> gpui::Size<Pixels> {
        self.layout.canvas_size(window.viewport_size())
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.sidebar_visible = !self.layout.sidebar_visible;
        self.pressed_keys.clear();
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn fit_image(&mut self, _: &ZoomFit, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.canvas_size(window);
        let width = f32::from(Pixels::from(self.original_width));
        let height = f32::from(Pixels::from(self.original_height));
        self.zoom_level = ((f32::from(viewport.width) - 48.0) / width.max(1.0))
            .min((f32::from(viewport.height) - 48.0) / height.max(1.0))
            .clamp(0.01, 50.0);
        self.pan_x = width.mul_add(-self.zoom_level, f32::from(viewport.width)) / 2.0;
        self.pan_y = height.mul_add(-self.zoom_level, f32::from(viewport.height)) / 2.0;
        cx.notify();
    }

    fn render_toolbar(&self) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap_3()
            .h(px(TOOLBAR_HEIGHT))
            .px_3()
            .border_b_1()
            .border_color(gui::theme::BORDER_MUTED)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(workspace_button("open-images", "Open", OpenFile))
                    .child(workspace_button("save-image", "Export", SaveImage)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(workspace_icon_button(
                        "zoom-out",
                        Icon::minus(),
                        "Zoom out",
                        ZoomOut,
                    ))
                    .child(
                        div()
                            .w(px(48.))
                            .text_center()
                            .text_xs()
                            .text_color(gui::theme::TEXT_MUTED)
                            .child(format!("{:.0}%", self.zoom_level * 100.0)),
                    )
                    .child(workspace_icon_button(
                        "zoom-in",
                        Icon::plus(),
                        "Zoom in",
                        ZoomIn,
                    ))
                    .child(workspace_button("zoom-fit", "Fit", ZoomFit))
                    .child(div().mx_2().h(px(16.)).w(px(1.)).bg(gui::theme::BORDER))
                    .child(workspace_button(
                        "commands",
                        "Commands  ⌘P",
                        ToggleCommandPalette,
                    ))
                    .child(workspace_button(
                        "toggle-properties",
                        if self.layout.sidebar_visible {
                            "Hide inspector"
                        } else {
                            "Show inspector"
                        },
                        ToggleSidebar,
                    )),
            )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep viewer message polling and dispatch in one task"
    )]
    pub fn process_messages(&self, window: &Window, cx: &Context<Self>) {
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
                        Ok(msg) => match msg {
                            FrontendMessage::UserFeedback(text, level) => {
                                toasts.push((text, level));
                            }
                            FrontendMessage::ReturnOverlays(overlays) => {
                                latest_overlays = Some(overlays);
                            }
                            FrontendMessage::SnapToPosition(x, y) => latest_snap = Some((x, y)),
                            FrontendMessage::PipelineProgress(progress) => {
                                activity_events.push(Some(progress));
                            }
                            FrontendMessage::PipelineFinished => activity_events.push(None),
                            FrontendMessage::ToolEvent { event, .. } => match *event {
                                FrontendMessage::UserFeedback(text, level) => {
                                    toasts.push((text, level))
                                }
                                FrontendMessage::SnapToPosition(x, y) => latest_snap = Some((x, y)),
                                _ => {}
                            },
                        },
                        Err(broadcast::error::TryRecvError::Lagged(_)) => {}
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
                            Some(progress) => this.pipeline_progress(&progress, cx),
                            None => this.pipeline_finished(cx),
                        }
                    }
                    if let Some(image) = latest_output {
                        this.show_output(image, window, cx);
                    }
                    if let Some(overlays) = latest_overlays {
                        let masks: HashMap<_, _> = overlays
                            .all_elements()
                            .enumerate()
                            .filter_map(|(index, element)| match element {
                                OverlayElement::ImageMask {
                                    image: Some(image), ..
                                } => Some((index, image.clone())),
                                _ => None,
                            })
                            .collect();
                        this.annotation_images.retain(|key, (_, rendered)| {
                            if masks.contains_key(key) {
                                true
                            } else {
                                let _ = window.drop_image(rendered.clone());
                                false
                            }
                        });
                        for (key, mask) in masks {
                            if this
                                .annotation_images
                                .get(&key)
                                .is_some_and(|(old, _)| Arc::ptr_eq(old, &mask))
                            {
                                continue;
                            }
                            if let Some((_, old)) = this.annotation_images.remove(&key) {
                                let _ = window.drop_image(old);
                            }
                            let rendered =
                                Arc::new(RenderImage::new(vec![Frame::new((*mask).clone())]));
                            this.annotation_images.insert(key, (mask, rendered));
                        }
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
        let new_width = u32::try_from(img_data.size(0).width.0).unwrap_or_default();
        let new_height = u32::try_from(img_data.size(0).height.0).unwrap_or_default();

        // Center the viewport on the new image unless its dimensions match the
        // current one (then it's just a reprocessed frame; keep the camera put).
        if (new_width != self.original_width || new_height != self.original_height)
            && let Some(viewport) = self.last_viewport_size
        {
            self.pan_x = (f32::from(gpui::Pixels::from(new_width)) / 2.0)
                .mul_add(-self.zoom_level, f32::from(viewport.width) / 2.0);
            self.pan_y = (f32::from(gpui::Pixels::from(new_height)) / 2.0)
                .mul_add(-self.zoom_level, f32::from(viewport.height) / 2.0);
        }

        self.original_width = new_width;
        self.original_height = new_height;
        self.image_loaded = true;
        let previous = std::mem::replace(&mut self.display_image, ImageSource::Render(img_data));

        if let ImageSource::Render(previous) = previous
            && let Err(e) = window.drop_image(previous)
        {
            eprintln!("Failed to release previous frame from the sprite atlas: {e}");
        }

        cx.notify();

        if let Err(e) = self.command_sender.send(PipelineCommand::RequestOverlays) {
            eprintln!("Failed to request overlays after image update: {e}");
        }
    }

    fn pipeline_progress(
        &mut self,
        progress: &strawberry_types::PipelineProgress,
        cx: &Context<Self>,
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
        self.activity_epoch = self.activity_epoch.wrapping_add(1);
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

    fn pipeline_finished(&mut self, cx: &Context<Self>) {
        let Some(activity) = &mut self.activity else {
            return;
        };
        if !activity.shown {
            self.activity = None;
            self.activity_epoch = self.activity_epoch.wrapping_add(1);
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
                    this.activity_epoch = this.activity_epoch.wrapping_add(1);
                    set_pipeline_busy(cx, false);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn add_toast(&mut self, text: String, level: FeedbackLevel, cx: &mut Context<Self>) {
        println!("Toast added {text}");
        let modal = cx.new(|cx| {
            let modal = FeedbackModal::new(text, level);
            FeedbackModal::enable_auto_close(cx);
            modal
        });
        self.toasts
            .retain(|toast| !toast.read(cx).marked_for_deletion);
        self.toasts.push(modal);
        cx.notify();
    }

    fn handle_open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let sender_p = self.command_sender.clone();
        let image_dir = self.image_directory.clone();

        cx.spawn_in(window, async move |_, _| {
            let files = FileDialog::new()
                .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
                .set_directory("~/")
                .pick_files();

            if let Some(files) = files {
                if let Some(first_file) = files.first()
                    && let Some(parent) = first_file.parent()
                    && let Ok(mut dir) = image_dir.write()
                {
                    *dir = Some(parent.to_path_buf());
                }

                if let Err(e) = sender_p.send(PipelineCommand::Load {
                    files,
                    reply: tokio::sync::oneshot::channel().0,
                }) {
                    eprintln!("Failed to send loaded images to pipeline: {e}");
                }
            }
        })
        .detach();
    }

    fn handle_save_image(&mut self, _: &SaveImage, window: &mut Window, cx: &mut Context<Self>) {
        let sender_p = self.command_sender.clone();

        cx.spawn_in(window, async move |this, cx| {
            let output_file = FileDialog::new()
                .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
                .set_directory("~/")
                .save_file();

            if let Some(path) = output_file {
                let (reply, result) = tokio::sync::oneshot::channel();
                if sender_p
                    .send(PipelineCommand::SaveOutput { path, reply })
                    .is_ok()
                {
                    let result = result.await;
                    let _ = this.update(cx, |this, cx| {
                        let (text, level) = match result {
                            Ok(Ok(())) => (
                                "Image saved successfully".to_string(),
                                FeedbackLevel::Success,
                            ),
                            Ok(Err(error)) => (error, FeedbackLevel::Error),
                            Err(error) => (error.to_string(), FeedbackLevel::Error),
                        };
                        this.add_toast(text, level, cx);
                    });
                }
            }
        })
        .detach();
    }

    fn zoom_in(&mut self, window: &Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level *= 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn zoom_out(&mut self, window: &Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level /= 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn adjust_pan_for_center_zoom(&mut self, window: &Window, old_zoom: f32, new_zoom: f32) {
        let viewport = self.canvas_size(window);
        let center = Point::new(
            px(f32::from(viewport.width) / 2.0),
            px(f32::from(viewport.height) / 2.0),
        );
        self.adjust_pan_around_point(center, old_zoom, new_zoom);
    }

    fn adjust_pan_around_point(&mut self, center: Point<Pixels>, old_zoom: f32, new_zoom: f32) {
        let center_x = center.x;
        let center_y = center.y;

        // Find the point in image coordinates that's currently at center
        let image_center_x = (f32::from(center_x) - self.pan_x) / old_zoom;
        let image_center_y = (f32::from(center_y) - self.pan_y) / old_zoom;

        // Adjust pan so this same point remains at center with new zoom
        self.pan_x = image_center_x.mul_add(-new_zoom, f32::from(center_x));
        self.pan_y = image_center_y.mul_add(-new_zoom, f32::from(center_y));
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta_y = match event.delta {
            gpui::ScrollDelta::Pixels(point) => f32::from(point.y),
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

        let canvas_position = WorkspaceLayout::canvas_position(event.position);
        self.adjust_pan_around_point(canvas_position, old_zoom, self.zoom_level);
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

        let length = dx.hypot(dy);
        if length > 0.0 {
            // Normalize such that diagonal movement is not faster
            self.pan_x = (dx / length * MOVEMENT_SPEED).mul_add(dt, self.pan_x);
            self.pan_y = (dy / length * MOVEMENT_SPEED).mul_add(dt, self.pan_y);
            cx.notify();
        }
    }

    fn start_pan_loop(&mut self, window: &Window, cx: &Context<Self>) {
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
        if self.palette.is_some() || !self.focus_handle.is_focused(window) {
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
            self.focus_handle.focus(window, cx);
            cx.notify();
            return;
        }
        self.pressed_keys.clear();

        let commands = self.commands.clone();
        let palette = cx.new(|cx| CommandPalette::new(commands, cx));
        cx.subscribe_in(
            &palette,
            window,
            |viewer, _palette, _: &PaletteDismissed, window, cx| {
                viewer.palette = None;
                viewer.focus_handle.focus(window, cx);
                cx.notify();
            },
        )
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
        let coords = (*self
            .img_coords
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner))?;
        // Preserve integer image dimensions exactly in f64 during hit testing.
        let relative_x =
            (f64::from(screen_x) - f64::from(coords.origin.x)) / f64::from(coords.size.width);
        let relative_y =
            (f64::from(screen_y) - f64::from(coords.origin.y)) / f64::from(coords.size.height);
        if self.original_width == 0
            || self.original_height == 0
            || !(0.0..1.0).contains(&relative_x)
            || !(0.0..1.0).contains(&relative_y)
        {
            return None;
        }
        let x = (relative_x * f64::from(self.original_width))
            .floor()
            .to_u64()?;
        let y = (relative_y * f64::from(self.original_height))
            .floor()
            .to_u64()?;
        (x < u64::from(self.original_width) && y < u64::from(self.original_height))
            .then_some((x, y))
    }

    // Render a single overlay element
    fn render_overlay_element(
        &self,
        (index, element): (usize, &OverlayElement),
    ) -> impl IntoElement + use<> {
        let image_width = num_traits::cast::<u32, f32>(self.original_width.max(1)).unwrap_or(1.);
        let image_height = num_traits::cast::<u32, f32>(self.original_height.max(1)).unwrap_or(1.);
        match element {
            OverlayElement::ImageMask { x, y, opacity, .. } => {
                let Some((mask, rendered)) = self.annotation_images.get(&index) else {
                    return div();
                };
                div()
                    .absolute()
                    .opacity(*opacity)
                    .left(gpui::relative(
                        num_traits::cast::<u32, f32>(*x).unwrap_or(0.) / image_width,
                    ))
                    .top(gpui::relative(
                        num_traits::cast::<u32, f32>(*y).unwrap_or(0.) / image_height,
                    ))
                    .w(gpui::relative(
                        num_traits::cast::<u32, f32>(mask.width()).unwrap_or(0.) / image_width,
                    ))
                    .h(gpui::relative(
                        num_traits::cast::<u32, f32>(mask.height()).unwrap_or(0.) / image_height,
                    ))
                    .child(
                        img(rendered.clone())
                            .use_nearest_filter(true)
                            .object_fit(gpui::ObjectFit::Fill)
                            .size_full(),
                    )
            }
            OverlayElement::Circle {
                x,
                y,
                radius,
                color,
                style,
            } => {
                let gpui_color: gpui::Rgba = (*color).into();

                match style {
                    CircleStyle::Border { width } | CircleStyle::Selected { width } => {
                        let screen_width = width * self.zoom_level;
                        div()
                            .absolute()
                            .left(gpui::relative((x - radius) / image_width))
                            .top(gpui::relative((y - radius) / image_height))
                            .w(gpui::relative(radius * 2.0 / image_width))
                            .h(gpui::relative(radius * 2.0 / image_height))
                            .rounded_full()
                            .border(px(screen_width))
                            .border_color(gpui_color)
                    }
                    CircleStyle::Filled => div()
                        .absolute()
                        .left(gpui::relative((x - radius) / image_width))
                        .top(gpui::relative((y - radius) / image_height))
                        .w(gpui::relative(radius * 2.0 / image_width))
                        .h(gpui::relative(radius * 2.0 / image_height))
                        .rounded_full()
                        .bg(gpui_color),
                }
            }
            OverlayElement::Point {
                x,
                y,
                radius,
                color,
            } => {
                let gpui_color: gpui::Rgba = (*color).into();

                div()
                    .absolute()
                    .left(gpui::relative((x - radius) / image_width))
                    .top(gpui::relative((y - radius) / image_height))
                    .w(gpui::relative(radius * 2.0 / image_width))
                    .h(gpui::relative(radius * 2.0 / image_height))
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

                // Convert the explicit RGBA overlay color to GPUI.
                let gpui_color: gpui::Rgba = (*color).into();

                gui::polyline::Polyline::new(points.clone(), gpui_color, px(screen_stroke))
                    .with_image_dimensions(self.original_width, self.original_height)
                    .into_element()
            }
            _ => div(), // Placeholder for other element types not yet implemented
        }
    }
}

impl Focusable for PrimaryViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PrimaryViewer {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep declarative GPUI layout and its event bindings together"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.canvas_size(window);
        if let Some(previous) = self.last_viewport_size {
            // Preserve the image point at the center when resizing or hiding properties.
            self.pan_x += (f32::from(viewport.width) - f32::from(previous.width)) / 2.0;
            self.pan_y += (f32::from(viewport.height) - f32::from(previous.height)) / 2.0;
        }
        self.last_viewport_size = Some(viewport);

        if let Some((x, y)) = self.snap_target.take() {
            self.pan_x = x.mul_add(-self.zoom_level, f32::from(viewport.width) / 2.0);
            self.pan_y = y.mul_add(-self.zoom_level, f32::from(viewport.height) / 2.0);
        }
        if !self.focus_handle.is_focused(window) {
            self.pressed_keys.clear();
        }

        let activity_status = self.activity.as_ref().filter(|a| a.shown).map(|a| {
            if a.total > 1 {
                format!("{} · {}/{}", a.label, a.stage.saturating_add(1), a.total)
            } else {
                format!("{}…", a.label)
            }
        });

        let img = img(self.display_image.clone())
            .use_nearest_filter(true)
            .object_fit(gpui::ObjectFit::Fill)
            .size_full();

        let coords = img.coords.clone();
        self.img_coords = coords;

        let mouse_down_sender = self.command_sender.clone();

        let mut img = img.on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener({
                move |this: &mut Self,
                      event: &MouseDownEvent,
                      _window: &mut Window,
                      _: &mut Context<Self>| {
                    let mouse_x = event.position.x;
                    let mouse_y = event.position.y;

                    if let Some((image_x, image_y)) =
                        this.screen_to_image_coords(f32::from(mouse_x), f32::from(mouse_y))
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

        img = img.on_mouse_move(cx.listener(
            move |this: &mut Self,
                  event: &MouseMoveEvent,
                  _window: &mut Window,
                  _cx: &mut Context<Self>| {
                let mouse_x = event.position.x;
                let mouse_y = event.position.y;

                if let Some((image_x, image_y)) =
                    this.screen_to_image_coords(f32::from(mouse_x), f32::from(mouse_y))
                {
                    let _ = this
                        .command_sender
                        .send(PipelineCommand::InteractionMove(image_x, image_y));
                }
            },
        ));

        let img = img.on_mouse_up_out(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, _| {
                let _ = this.command_sender.send(PipelineCommand::InteractionEnd);
            }),
        );
        let img = img.on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(
                move |this: &mut Self,
                      _event: &MouseUpEvent,
                      _window: &mut Window,
                      _cx: &mut Context<Self>| {
                    if let Err(e) = this.command_sender.send(PipelineCommand::InteractionEnd) {
                        eprintln!("Error sending interaction end: {e}");
                    }
                },
            ),
        );
        let canvas = div()
            .id("image-canvas")
            .relative()
            .key_context("PrimaryViewer")
            .track_focus(&self.focus_handle(cx))
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(gui::theme::CANVAS)
            .overflow_hidden()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus_handle.focus(window, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_drop(cx.listener(
                |this: &mut Self,
                 paths: &ExternalPaths,
                 _window: &mut Window,
                 _cx: &mut Context<Self>| {
                    if let Err(e) = this.command_sender.send(PipelineCommand::Load {
                        files: paths.paths().to_vec(),
                        reply: tokio::sync::oneshot::channel().0,
                    }) {
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
                        .w(px(num_traits::cast::<u32, f32>(self.original_width)
                            .unwrap_or(0.)
                            * self.zoom_level))
                        .h(px(num_traits::cast::<u32, f32>(self.original_height)
                            .unwrap_or(0.)
                            * self.zoom_level))
                        // One layout establishes the image and every annotation's pixel grid.
                        .child(img)
                        .children(
                            self.overlays
                                .all_elements()
                                .enumerate()
                                .map(|element| self.render_overlay_element(element)),
                        ),
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
                        .gap_3()
                        .text_color(gui::theme::TEXT_MUTED)
                        .child(div().text_sm().child("Drop images here to get started"))
                        .child(workspace_button("empty-open", "Open  ⌘L", OpenFile)),
                )
            })
            .children(self.toasts.iter().enumerate().map(|(index, toast)| {
                div()
                    .absolute()
                    .bottom(px(f32::from(gpui::Pixels::from(index)).mul_add(90.0, 20.0)))
                    .right(px(20.0))
                    .child(toast.clone())
            }))
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
                                gpui::Styled::opacity,
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
            });

        div()
            .id("workspace")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(gui::theme::WINDOW_BG)
            .text_color(gui::theme::TEXT)
            .on_action(cx.listener(Self::handle_open_file))
            .on_action(cx.listener(Self::handle_save_image))
            .on_action(cx.listener(Self::handle_zoom_in))
            .on_action(cx.listener(Self::handle_zoom_out))
            .on_action(cx.listener(Self::fit_image))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.layout.resizing_sidebar {
                    if event.dragging() {
                        let maximum = (f32::from(window.viewport_size().width) - 240.0)
                            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                        this.layout.sidebar_width = (f32::from(window.viewport_size().width)
                            - f32::from(event.position.x))
                        .clamp(SIDEBAR_MIN_WIDTH, maximum);
                    } else {
                        this.layout.resizing_sidebar = false;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.layout.resizing_sidebar {
                        this.layout.resizing_sidebar = false;
                        cx.notify();
                    }
                }),
            )
            .child(self.render_toolbar())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(canvas),
                    )
                    .when(self.layout.sidebar_visible, |el| {
                        el.child(
                            div()
                                .id("sidebar-resize")
                                .w(px(SIDEBAR_GRIP_WIDTH))
                                .h_full()
                                .flex_none()
                                .bg(if self.layout.resizing_sidebar {
                                    gui::theme::ACCENT
                                } else {
                                    gui::theme::WINDOW_BG
                                })
                                .cursor_col_resize()
                                .hover(|style| style.bg(gui::theme::ACCENT))
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.layout.resizing_sidebar = true;
                                        this.pressed_keys.clear();
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .w(px(self.layout.sidebar_width))
                                .h_full()
                                .flex_none()
                                .child(self.sidebar.clone()),
                        )
                    }),
            )
            // Paint the command palette over the complete workspace.
            .when_some(self.palette.clone(), gpui::ParentElement::child)
    }
}

fn workspace_button(
    id: &'static str,
    label: &'static str,
    action: impl gpui::Action,
) -> impl IntoElement {
    Button::new(id)
        .variant(ButtonVariant::Ghost)
        .compact()
        .label(label)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn workspace_icon_button(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    action: impl gpui::Action,
) -> impl IntoElement {
    Button::new(id)
        .variant(ButtonVariant::Ghost)
        .compact()
        .icon(icon, label)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

/// "`RollingBall`" (a `BackendToolType` debug name) -> "Rolling Ball".
fn humanize_tool_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len().saturating_add(4));
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            result.push(' ');
        }
        result.push(c);
    }
    result
}

#[tokio::main]
#[expect(
    clippy::too_many_lines,
    reason = "Keep declarative GPUI layout and its event bindings together"
)]
async fn main() -> Result<()> {
    env_logger::init();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let (mut pipeline, view_sender, view_recv, output_frames) = Pipeline::new().await?;

    let pipeline_handle = tokio::spawn(async move {
        loop {
            if let Err(e) = pipeline.process_commands().await {
                eprintln!("Error processing pipeline commands: {e}");
                eprintln!("Pipeline processing failed: {e}");
            }
            tokio::time::sleep(Duration::from_millis(33)).await; // Lock at 30FPS
        }
    });

    // Spawn a task to monitor the pipeline and propagate panics
    tokio::spawn(async move {
        let Err(e) = pipeline_handle.await;
        if e.is_panic() {
            eprintln!("Pipeline task panicked, shutting down application");
            panic::resume_unwind(e.into_panic());
        }
    });

    let image_directory: Arc<RwLock<Option<PathBuf>>> = Arc::new(RwLock::new(None));

    gpui_platform::application()
        .with_assets(Assets {
            base: manifest_dir.join("examples"),
        })
        .run(move |cx: &mut App| {
            cx.on_action(|_: &Quit, cx| cx.quit());

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-l", OpenFile, None),
                KeyBinding::new("cmd-s", SaveImage, None),
                KeyBinding::new("cmd-alt-b", ToggleSidebar, None),
                KeyBinding::new("f", ZoomFit, Some("PrimaryViewer")),
                KeyBinding::new("secondary-p", ToggleCommandPalette, None),
                KeyBinding::new("secondary-v", PickerPaste, Some("SearchPicker")),
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
                window_min_size: Some(size(px(960.), px(540.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Strawberry")),
                    appears_transparent: false,
                    ..Default::default()
                }),

                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    size: size(px(1280.), px(800.)),
                    origin: Point::new(px(200.), px(200.)),
                })),

                ..Default::default()
            };

            // Create a default 800x600 black image
            let width = 800u32;
            let height = 600u32;
            let mut data = RgbaImage::new(width, height);

            // Fill with black pixels (BGRA format for GPUI)
            for pixel in data.as_chunks_mut::<4>().0 {
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
                let sidebar = cx.new(|cx| ToolSidebar::new(view_sender.clone(), window, cx));
                let commands =
                    commands::install(cx, &sidebar, view_sender.clone(), image_directory.clone());
                let raw_primary_viewer = PrimaryViewer {
                    focus_handle: cx.focus_handle(),
                    zoom_level: 1.0,
                    original_width: 800,
                    original_height: 600,
                    pan_x: 0.0,
                    pan_y: 0.0,
                    pressed_keys: HashSet::new(),
                    command_receiver: view_recv.resubscribe(),
                    command_sender: view_sender.clone(),
                    output_frame_receiver: output_frames.clone(),
                    display_image: ImageSource::Render(img_data),
                    img_coords: Arc::new(RwLock::new(None)),
                    overlays: OverlayCollection::new(),
                    annotation_images: HashMap::new(),
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
                    sidebar: sidebar.clone(),
                    layout: WorkspaceLayout::default(),
                };

                let viewer = cx.new(|cx| {
                    cx.subscribe(
                        &sidebar,
                        |viewer: &mut PrimaryViewer, _, _: &ToolSelected, cx| {
                            viewer.layout.sidebar_visible = true;
                            viewer.pressed_keys.clear();
                            cx.notify();
                        },
                    )
                    .detach();
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
