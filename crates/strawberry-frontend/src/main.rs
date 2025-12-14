use anyhow::Result;
use gpui::{
    actions, div, img, prelude::*, px, rgb, size, App, AppContext, Application, AssetSource,
    Bounds, Context, Entity, ExternalPaths, FocusHandle, Focusable, ImageSource, KeyBinding,
    KeyDownEvent, KeyUpEvent, Menu, MenuItem, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, RenderImage, ScrollWheelEvent, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};
use gui::feedback::FeedbackModal;
use image::{Frame, RgbaImage};
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use std::{fs, panic};
use strawberry_backend::Pipeline;
use strawberry_types::{
    overlay::{CircleStyle, OverlayCollection, OverlayElement},
    FrontendMessage, PipelineCommand, ToolFrontend,
};
use tools::compositor::CompositorView;
use tools::contrast::ContrastView;
use tools::counter::CounterView;
use tools::decomposer::DecomposerView;
use tools::gaussian_blur::GaussianBlurView;
use tools::grayscale::GrayscaleView;
use tools::local_maxima::LocalMaximaView;
use tools::manager::ManagerView;
use tools::morphology::MorphologyView;
use tools::rolling_ball::RollingBallView;
use tools::sobel::SobelView;
use tools::threshold::ThresholdView;
use tools::watershed::WatershedView;

struct Assets {
    base: PathBuf,
}

mod gui;
mod tools;

const MOVEMENT_SPEED: f32 = 25.0;

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
    display_image: ImageSource,
    img_coords: Arc<RwLock<Option<Bounds<Pixels>>>>,
    overlays: OverlayCollection,
    snap_target: Option<(f32, f32)>,
    toasts: Vec<Entity<FeedbackModal>>,
    last_viewport_size: Option<gpui::Size<Pixels>>,
}

impl PrimaryViewer {
    pub fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut receiver_ref = self.command_receiver.resubscribe();

        // Request initial overlays
        if let Err(e) = self.command_sender.send(PipelineCommand::RequestOverlays) {
            eprintln!("Failed to request overlays: {e}");
        }

        cx.spawn_in(window, async move |this, cx| {
            loop {
                if let Ok(command) = receiver_ref.try_recv() {
                    match command {
                        PipelineCommand::Output(output) => {
                            let frame = Frame::new(output);
                            let img_frames = vec![frame];
                            let img_data = Arc::new(RenderImage::new(img_frames));

                            if let Err(e) = this.update(cx, |this, cx| {
                                let new_width = img_data.size(0).width.0 as u32;
                                let new_height = img_data.size(0).height.0 as u32;

                                if new_width != this.original_width
                                    || new_height != this.original_height
                                {
                                    if let Some(viewport) = this.last_viewport_size {
                                        let viewport_width = viewport.width.0;
                                        let viewport_height = viewport.height.0;
                                        let zoom = this.zoom_level;

                                        this.pan_x = (viewport_width / 2.0)
                                            - ((new_width as f32 / 2.0) * zoom);
                                        this.pan_y = (viewport_height / 2.0)
                                            - ((new_height as f32 / 2.0) * zoom);
                                    }
                                }

                                this.original_width = new_width;
                                this.original_height = new_height;
                                this.display_image = ImageSource::Render(img_data);
                                cx.notify();
                            }) {
                                eprintln!("Failed to update display image: {e}");
                            }

                            // Request overlays after image update
                            let _ = this.update(cx, |this, _| {
                                let _ = this.command_sender.send(PipelineCommand::RequestOverlays);
                            });

                            println!("Image updated");
                        }
                        PipelineCommand::FrontendMessage(msg) => match msg {
                            FrontendMessage::UserFeedback(text) => {
                                println!("Toast added {}", text);
                                let raw_modal = FeedbackModal::new(text);
                                let modal = cx.new(|cx| {
                                    raw_modal.enable_auto_close(cx);
                                    raw_modal
                                });
                                if let Err(e) = this.update(cx, |this, cx| {
                                    this.toasts
                                        .retain(|toast| !toast.read(cx).marked_for_deletion);
                                    this.toasts.push(
                                        modal.expect("Modal entity should be created successfully"),
                                    );
                                    cx.notify();
                                }) {
                                    eprintln!("Failed to add toast: {e}");
                                }
                            }
                            FrontendMessage::ReturnOverlays(overlays) => {
                                if let Err(e) = this.update(cx, |this, cx| {
                                    this.overlays = overlays;
                                    cx.notify();
                                }) {
                                    eprintln!("Failed to update overlays: {e}");
                                }
                            }
                            FrontendMessage::SnapToPosition(x, y) => {
                                if let Err(e) = this.update(cx, |this, _cx| {
                                    this.snap_target = Some((x, y));
                                }) {
                                    eprintln!("Failed to set snap target: {e}");
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await; // 30 FPS
            }
        })
        .detach();
    }

    fn handle_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        let sender_p = self.command_sender.clone();

        cx.spawn_in(_window, async move |_, _| {
            let files = FileDialog::new()
                .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
                .set_directory("~/")
                .pick_files();

            if let Some(files) = files {
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
        let center_x = center.x.0;
        let center_y = center.y.0;

        // Find the point in image coordinates that's currently at center
        let image_center_x = (center_x - self.pan_x) / old_zoom;
        let image_center_y = (center_y - self.pan_y) / old_zoom;

        // Adjust pan so this same point remains at center with new zoom
        self.pan_x = center_x - (image_center_x * new_zoom);
        self.pan_y = center_y - (image_center_y * new_zoom);
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta_y = match event.delta {
            gpui::ScrollDelta::Pixels(point) => point.y.0,
            gpui::ScrollDelta::Lines(point) => point.y * 20.0,
        };

        if delta_y == 0.0 {
            return;
        }

        let old_zoom = self.zoom_level;
        let scale = 1.001f32.powf(delta_y);
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

    fn pan_with_current_keys(&mut self, cx: &mut Context<Self>) {
        let mut pan_x = 0.0;
        let mut pan_y = 0.0;

        if self.pressed_keys.contains("a") {
            pan_x += MOVEMENT_SPEED;
        }
        if self.pressed_keys.contains("d") {
            pan_x -= MOVEMENT_SPEED;
        }
        if self.pressed_keys.contains("w") {
            pan_y += MOVEMENT_SPEED;
        }
        if self.pressed_keys.contains("s") {
            pan_y -= MOVEMENT_SPEED;
        }

        if pan_x != 0.0 || pan_y != 0.0 {
            self.pan_x += pan_x;
            self.pan_y += pan_y;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke.key;
        if matches!(key.as_str(), "w" | "a" | "s" | "d") {
            self.pressed_keys.insert(key.clone());
            self.pan_with_current_keys(cx);
        }
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
            let relative_x = (screen_x - coords.origin.x.0) / coords.size.width.0;
            let relative_y = (screen_y - coords.origin.y.0) / coords.size.height.0;

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
            let viewport_center_x = viewport.width.0 / 2.0;
            let viewport_center_y = viewport.height.0 / 2.0;

            // Calculate pan to center the position
            self.pan_x = viewport_center_x - (x * self.zoom_level);
            self.pan_y = viewport_center_y - (y * self.zoom_level);
        }

        // Ensure this element is always focused
        window.focus(&self.focus_handle(cx));

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
                    let mouse_x = event.position.x.0;
                    let mouse_y = event.position.y.0;

                    if let Some((image_x, image_y)) = this.screen_to_image_coords(mouse_x, mouse_y)
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
                let mouse_x = event.position.x.0;
                let mouse_y = event.position.y.0;

                if let Some((image_x, image_y)) = this.screen_to_image_coords(mouse_x, mouse_y) {
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
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_action(cx.listener(Self::handle_zoom_in))
            .on_action(cx.listener(Self::handle_zoom_out))
            .on_action(cx.listener(Self::handle_open_file))
            .on_action(cx.listener(Self::handle_save_image))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
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
            .child(
                div()
                    .absolute()
                    .left(px(self.pan_x))
                    .top(px(self.pan_y))
                    .child(_img),
            )
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
    }
}

actions!([
    Quit,
    ZoomIn,
    ZoomOut,
    OpenFile,
    SaveImage,
    CompositorSettings,
    SobelSettings,
    ContrastSettings,
    CounterSettings,
    GaussianBlurSettings,
    GrayscaleSettings,
    LocalMaximaSettings,
    ManagerSettings,
    MorphologySettings,
    RollingBallSettings,
    ThresholdSettings,
    WatershedSettings,
    DecomposerSettings
]);

// Generic tool window creation function
fn create_tool_window<T, A>(
    cx: &mut App,
    view_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    view_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    constructor: impl Fn(
            tokio::sync::broadcast::Receiver<PipelineCommand>,
            tokio::sync::broadcast::Sender<PipelineCommand>,
        ) -> T
        + 'static,
) where
    T: ToolFrontend + gpui::Render + 'static,
    A: gpui::Action + 'static,
{
    let view_sender_clone = view_sender.clone();
    let view_recv_clone = view_receiver.resubscribe();

    cx.on_action(move |_: &A, cx| {
        let mut tool = constructor(view_recv_clone.resubscribe(), view_sender_clone.clone());

        if let Err(e) = cx.open_window(tool.get_window_options(), |window, cx| {
            cx.new(|cx| {
                tool.initialize(window, cx);
                tool
            })
        }) {
            eprintln!("Failed to open tool window: {e}");
        }
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let (mut pipeline, view_sender, view_recv) = Pipeline::new().await?;

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

    Application::new()
        .with_assets(Assets {
            base: manifest_dir.join("examples"),
        })
        .run(move |cx: &mut App| {
            // Register all tool windows using the generic function
            create_tool_window::<CompositorView, CompositorSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                CompositorView::new,
            );
            create_tool_window::<ContrastView, ContrastSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                ContrastView::new,
            );
            create_tool_window::<CounterView, CounterSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                CounterView::new,
            );
            create_tool_window::<GaussianBlurView, GaussianBlurSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                GaussianBlurView::new,
            );
            create_tool_window::<GrayscaleView, GrayscaleSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                GrayscaleView::new,
            );
            create_tool_window::<ThresholdView, ThresholdSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                ThresholdView::new,
            );
            create_tool_window::<LocalMaximaView, LocalMaximaSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                LocalMaximaView::new,
            );
            create_tool_window::<MorphologyView, MorphologySettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                MorphologyView::new,
            );
            create_tool_window::<RollingBallView, RollingBallSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                RollingBallView::new,
            );
            create_tool_window::<WatershedView, WatershedSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                WatershedView::new,
            );
            create_tool_window::<ManagerView, ManagerSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                ManagerView::new,
            );
            create_tool_window::<DecomposerView, DecomposerSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                DecomposerView::new,
            );
            create_tool_window::<SobelView, SobelSettings>(
                cx,
                view_sender.clone(),
                view_recv.resubscribe(),
                SobelView::new,
            );

            let _window_closed_subscription = cx.on_window_closed(move |_| {});

            cx.on_action(|_: &Quit, cx| cx.quit());

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-l", OpenFile, None),
                KeyBinding::new("cmd-s", SaveImage, None),
                KeyBinding::new("+", ZoomIn, None),
                KeyBinding::new("-", ZoomOut, None),
            ]);

            cx.set_menus(vec![
                Menu {
                    name: "Strawberry".into(),
                    items: vec![MenuItem::action("Quit", Quit)],
                },
                Menu {
                    name: "File".into(),
                    items: vec![
                        MenuItem::action("Load Images...", OpenFile),
                        MenuItem::action("Save Image", SaveImage),
                    ],
                },
                Menu {
                    name: "Tools".into(),
                    items: vec![
                        MenuItem::action("Pipeline", ManagerSettings),
                        MenuItem::action("Compositor", CompositorSettings),
                        MenuItem::action("Contrast", ContrastSettings),
                        MenuItem::action("Counter", CounterSettings),
                        MenuItem::action("Gaussian Blur", GaussianBlurSettings),
                        MenuItem::action("Grayscale", GrayscaleSettings),
                        MenuItem::action("Local Maxima", LocalMaximaSettings),
                        MenuItem::action("Morphology", MorphologySettings),
                        MenuItem::action("Rolling Ball", RollingBallSettings),
                        MenuItem::action("Threshold", ThresholdSettings),
                        MenuItem::action("Watershed", WatershedSettings),
                        MenuItem::action("Decomposer", DecomposerSettings),
                        MenuItem::action("Sobel", SobelSettings),
                    ],
                },
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

            if let Err(e) = cx.open_window(window_options, |window, cx| {
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
                    display_image: ImageSource::Render(img_data),
                    img_coords: Arc::new(RwLock::new(None)),
                    overlays: OverlayCollection::new(),
                    snap_target: None,
                    toasts: vec![],
                    last_viewport_size: None,
                };

                let viewer = cx.new(|cx| {
                    raw_primary_viewer.process_messages(window, cx);
                    raw_primary_viewer
                });

                // Bind keys locally to the viewer
                cx.bind_keys([
                    KeyBinding::new("=", ZoomIn, Some("PrimaryViewer")),
                    KeyBinding::new("-", ZoomOut, Some("PrimaryViewer")),
                ]);

                // Focus the viewer so it can receive key events
                window.focus(&viewer.read(cx).focus_handle(cx));

                viewer
            }) {
                eprintln!("Failed to open main window: {e}");
                return;
            }

            // Activate the app after window creation
            cx.activate(true);
        });

    Ok(())
}
