use anyhow::Result;
use clap::Parser;
use gpui::{
    actions, div, img, prelude::*, px, rgb, size, AnyDrag, App, AppContext, Application,
    AssetSource, Bounds, Context, Entity, ExternalPaths, FileDropEvent, FocusHandle, Focusable,
    ImageSource, KeyBinding, KeyDownEvent, KeyUpEvent, Menu, MenuItem, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use gui::feedback::FeedbackModal;
use image::{Frame, RgbaImage};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tools::decomposer::DecomposerView;
use tools::sobel::SobelView;

use cli::{run_cli, Args};
use pipeline::counter::{CounterMode, CounterState, CounterTool};
use pipeline::{
    CountMessage, FrontendMessage, Pipeline, PipelineCommand, PipelineObjectMessage, ToolState,
};
use rfd::FileDialog;
use tools::compositor::CompositorView;
use tools::contrast::ContrastView;
use tools::counter::CounterView;
use tools::gaussian_blur::GaussianBlurView;
use tools::grayscale::GrayscaleView;
use tools::local_maxima::LocalMaximaView;
use tools::manager::ManagerView;
use tools::morphology::MorphologyView;
use tools::rolling_ball::RollingBallView;
use tools::threshold::ThresholdView;
use tools::watershed::WatershedView;
use tools::ToolFrontend;
use utils::consts::CIRCLE_COLORS;

struct Assets {
    base: PathBuf,
}

mod cli;
mod gui;
mod pipeline;
mod processing;
mod tools;
mod utils;

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

#[derive(Debug, Clone)]
struct LassoState {
    is_dragging: bool,
    points: Vec<(u64, u64)>,
}

impl Default for LassoState {
    fn default() -> Self {
        Self {
            is_dragging: false,
            points: vec![],
        }
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
    count_data: Option<CounterState>,
    review_snap_target: Option<(u64, u64)>,
    lasso_state: LassoState,
    counter_sender: Option<flume::Sender<PipelineCommand>>,
    toasts: Vec<Entity<FeedbackModal>>,
}

impl PrimaryViewer {
    fn load_image_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let sender_p = self.command_sender.clone();

        cx.spawn_in(_window, async move |_, _| {
            let files = FileDialog::new()
                .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
                .set_directory("/")
                .pick_files();

            if let Some(files) = files {
                if let Err(e) = sender_p.send(PipelineCommand::Load(files, false)) {
                    eprintln!("Failed to send loaded images to pipeline: {}", e);
                }
            }
        })
        .detach();
    }

    pub fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut receiver_ref = self.command_receiver.resubscribe();
        let (counter_sender, counter_recv) = flume::unbounded();
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            pipeline::BackendToolType::Counter,
            counter_sender,
        )) {
            eprintln!("Failed to get counter backend in primary viewer {}", e)
        };

        // secondary_receivers: Vec<flume::Receiver<PipelineCommand>>,
        // secondary_senders: Vec<flume::Sender<PipelineCommand>>,

        let secondary_receivers: Vec<flume::Receiver<PipelineCommand>> = vec![counter_recv];

        cx.spawn_in(window, async move |this, cx| {
            loop {
                if let Ok(command) = receiver_ref.try_recv() {
                    match command {
                        PipelineCommand::Output(output) => {
                            let frame = Frame::new(output);
                            let img_frames = vec![frame];
                            let img_data = Arc::new(RenderImage::new(img_frames));

                            if let Err(e) = this.update(cx, |this, cx| {
                                this.original_width = img_data.size(0).width.0 as u32;
                                this.original_height = img_data.size(0).height.0 as u32;
                                this.display_image = ImageSource::Render(img_data);
                                cx.notify();
                            }) {
                                eprintln!("Failed to update display image: {}", e);
                            }
                            println!("Image updated");
                        }
                        PipelineCommand::FrontendMessage(msg) => match msg {
                            FrontendMessage::UserFeedback(text) => {
                                let raw_modal = FeedbackModal::new(text);
                                let modal = cx.new(|cx| {
                                    raw_modal.enable_auto_close(cx);
                                    raw_modal
                                });
                                if let Err(e) = this.update(cx, |this, cx| {
                                    this.toasts.push(
                                        modal.expect("Modal entity should be created successfully"),
                                    );
                                    println!("Toast added");
                                    cx.notify();
                                }) {
                                    eprintln!("Failed to add toast: {}", e);
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                for recv in &secondary_receivers {
                    if let Ok(msg) = recv.try_recv() {
                        match msg {
                            PipelineCommand::FrontendMessage(msg) => match msg {
                                FrontendMessage::ReturnBackendTools(_, return_sender) => {
                                    if let Err(e) = this.update(cx, |this, cx| {
                                        this.counter_sender = Some(return_sender[0].clone());
                                        //TODO: Mmm danger
                                    }) {
                                        eprintln!("Failed to update counter sender: {}", e);
                                    }
                                }
                                FrontendMessage::ReturnState(tool_state) => {
                                    match tool_state {
                                        ToolState::CounterState(counter_state) => {
                                            // Check if we're in review mode and need to snap to a count
                                            let should_snap = counter_state.is_enabled
                                                && counter_state.active_tool == CounterTool::Review
                                                && counter_state.active_count_type.is_some();

                                            let snap_position = if should_snap {
                                                counter_state
                                                    .active_count_type
                                                    .and_then(|count_type| {
                                                        counter_state.manual_counts.get(&count_type)
                                                    })
                                                    .and_then(|counts| {
                                                        counts
                                                            .get(counter_state.review_current_index)
                                                    })
                                                    .map(|count| count.position)
                                            } else {
                                                None
                                            };

                                            // Store count data and review snap target
                                            if let Err(e) = this.update(cx, |this, cx| {
                                                this.count_data = Some(counter_state);
                                                this.review_snap_target = snap_position;
                                                cx.notify();
                                            }) {
                                                eprintln!("Failed to update display image: {}", e);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            },
                            _ => {}
                        }
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
        self.load_image_file(_window, cx);
    }

    fn zoom_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level = self.zoom_level * 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level = self.zoom_level / 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        cx.notify();
    }

    fn adjust_pan_for_center_zoom(&mut self, window: &mut Window, old_zoom: f32, new_zoom: f32) {
        let viewport = window.viewport_size();
        let viewport_center_x = viewport.width.0 / 2.0;
        let viewport_center_y = viewport.height.0 / 2.0;

        // Find the point in image coordinates that's currently at viewport center
        let image_center_x = (viewport_center_x - self.pan_x) / old_zoom;
        let image_center_y = (viewport_center_y - self.pan_y) / old_zoom;

        // Adjust pan so this same point remains at viewport center with new zoom
        self.pan_x = viewport_center_x - (image_center_x * new_zoom);
        self.pan_y = viewport_center_y - (image_center_y * new_zoom);
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
        if let Some(coords) = self
            .img_coords
            .read()
            .expect("Image coordinates lock should not be poisoned")
            .clone()
        {
            let relative_x = (screen_x - coords.origin.x.0) / coords.size.width.0;
            let relative_y = (screen_y - coords.origin.y.0) / coords.size.height.0;

            if relative_x >= 0.0 && relative_x <= 1.0 && relative_y >= 0.0 && relative_y <= 1.0 {
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

    // Start lasso operation
    fn start_lasso(&mut self, image_x: u64, image_y: u64) {
        self.lasso_state.is_dragging = true;
        self.lasso_state.points.clear();
        self.lasso_state.points.push((image_x, image_y));
    }

    // Add point to lasso
    fn add_lasso_point(&mut self, image_x: u64, image_y: u64) {
        if self.lasso_state.is_dragging {
            self.lasso_state.points.push((image_x, image_y));
        }
    }

    // End lasso operation and send points to backend
    fn end_lasso(&mut self) {
        if self.lasso_state.is_dragging && !self.lasso_state.points.is_empty() {
            self.lasso_state.is_dragging = false;

            // Send the lasso points to the backend
            let points = self.lasso_state.points.clone();
            if let Some(counter_sender) = &self.counter_sender {
                if let Err(e) = counter_sender.send(PipelineCommand::PipelineObject(
                    PipelineObjectMessage::CountMessage(CountMessage::DeletionLasso(points)),
                )) {
                    eprintln!("Failed to send lasso deletion command: {}", e);
                }
            }
            println!("Lasso points sent to backend");
            self.lasso_state.points.clear();
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
        // Handle review snap if needed
        if let Some((x, y)) = self.review_snap_target.take() {
            let viewport = window.viewport_size();
            let viewport_center_x = viewport.width.0 / 2.0;
            let viewport_center_y = viewport.height.0 / 2.0;

            // Calculate pan to center the count position
            self.pan_x = viewport_center_x - (x as f32 * self.zoom_level);
            self.pan_y = viewport_center_y - (y as f32 * self.zoom_level);
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
                        let is_lasso_delete_active =
                            if let Some(ref counter_state) = this.count_data {
                                counter_state.is_enabled
                                    && counter_state.active_tool == CounterTool::LassoDelete
                            } else {
                                false
                            };
                        if is_lasso_delete_active {
                            this.start_lasso(image_x, image_y);
                        } else {
                            if let Err(e) = mouse_down_sender
                                .send(PipelineCommand::HandleClick(image_x, image_y))
                            {
                                eprintln!("Error sending click event: {}", e);
                            }
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
                  cx: &mut Context<PrimaryViewer>| {
                if this.lasso_state.is_dragging {
                    let mouse_x = event.position.x.0;
                    let mouse_y = event.position.y.0;

                    if let Some((image_x, image_y)) = this.screen_to_image_coords(mouse_x, mouse_y)
                    {
                        this.add_lasso_point(image_x, image_y);
                        cx.notify();
                    }
                }
            },
        ));

        let mut _img = _img.on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(
                move |this: &mut PrimaryViewer,
                      _event: &MouseUpEvent,
                      _window: &mut Window,
                      cx: &mut Context<PrimaryViewer>| {
                    if this.lasso_state.is_dragging {
                        this.end_lasso();
                        cx.notify();
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
            .on_action(cx.listener(Self::handle_zoom_in))
            .on_action(cx.listener(Self::handle_zoom_out))
            .on_action(cx.listener(Self::handle_open_file))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_drop(cx.listener(
                |this: &mut PrimaryViewer,
                 paths: &ExternalPaths,
                 _window: &mut Window,
                 _cx: &mut Context<PrimaryViewer>| {
                    if let Err(e) = this
                        .command_sender
                        .send(PipelineCommand::Load(paths.paths().to_vec(), false))
                    {
                        eprintln!("Failed to send loaded images to pipeline: {}", e);
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
            .children(self.toasts.clone())
            .children({
                // Draw circles for count data
                let mut circles = Vec::new();

                if let Some(ref counter_state) = self.count_data {
                    if counter_state.is_enabled {
                        // Helper function to convert image coords to screen coords
                        let img_to_screen = |img_x: u64, img_y: u64| -> (f32, f32) {
                            (
                                img_x as f32 * self.zoom_level + self.pan_x,
                                img_y as f32 * self.zoom_level + self.pan_y,
                            )
                        };

                        // Draw manual counts
                        for (count_type_id, counts) in &counter_state.manual_counts {
                            let color_index = (*count_type_id as usize) % CIRCLE_COLORS.len();
                            let color = CIRCLE_COLORS[color_index];
                            let gpui_color = rgb((color[0] as u32) << 16
                                | (color[1] as u32) << 8
                                | color[2] as u32);

                            for (index, count) in counts.iter().enumerate() {
                                let (screen_x, screen_y) =
                                    img_to_screen(count.position.0, count.position.1);

                                // Use hot pink for selected count in review mode
                                let circle_color = if counter_state.active_tool
                                    == CounterTool::Review
                                    && counter_state.active_count_type == Some(*count_type_id)
                                    && index == counter_state.review_current_index
                                {
                                    rgb(0xffd500) // Hot pink
                                } else {
                                    gpui_color
                                };

                                circles.push(
                                    div()
                                        .absolute()
                                        .left(px(screen_x - 10.0)) // Center the 20px circle
                                        .top(px(screen_y - 10.0))
                                        .w(px(20.0))
                                        .h(px(20.0))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(circle_color),
                                );
                            }
                        }

                        // Draw auto counts
                        if counter_state.mode == CounterMode::Auto {
                            let color = CIRCLE_COLORS[0]; // Use first color for auto counts
                            let gpui_color = rgb((color[0] as u32) << 16
                                | (color[1] as u32) << 8
                                | color[2] as u32);

                            for count in &counter_state.auto_counts {
                                let (screen_x, screen_y) =
                                    img_to_screen(count.position.0, count.position.1);
                                circles.push(
                                    div()
                                        .absolute()
                                        .left(px(screen_x - 10.0))
                                        .top(px(screen_y - 10.0))
                                        .w(px(20.0))
                                        .h(px(20.0))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(gpui_color),
                                );
                            }
                        }
                    }
                }

                // Draw lasso lines if dragging
                if self.lasso_state.is_dragging && self.lasso_state.points.len() > 1 {
                    let img_to_screen = |img_x: u64, img_y: u64| -> (f32, f32) {
                        (
                            img_x as f32 * self.zoom_level + self.pan_x,
                            img_y as f32 * self.zoom_level + self.pan_y,
                        )
                    };

                    // Draw small dots for each lasso point
                    for point in &self.lasso_state.points {
                        let (screen_x, screen_y) = img_to_screen(point.0, point.1);
                        circles.push(
                            div()
                                .absolute()
                                .left(px(screen_x - 2.0))
                                .top(px(screen_y - 2.0))
                                .w(px(4.0))
                                .h(px(4.0))
                                .rounded_full()
                                .bg(rgb(0xff0000)), // Red color for lasso
                        );
                    }
                }

                circles
            })
    }
}

actions!([
    Quit,
    ZoomIn,
    ZoomOut,
    OpenFile,
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
            let tool_view = cx.new(|cx| {
                tool.initialize(window, cx);
                tool
            });
            tool_view
        }) {
            eprintln!("Failed to open tool window: {}", e);
        }
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    if args.cli {
        return run_cli(args).await;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let (mut pipeline, view_sender, view_recv) = Pipeline::new().await?;

    tokio::spawn(async move {
        loop {
            if let Err(e) = pipeline.process_commands().await {
                eprintln!("Error processing pipeline commands: {}", e);
            }
            tokio::time::sleep(Duration::from_millis(33)).await; // Lock at 30FPS
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
                KeyBinding::new("cmd-o", OpenFile, None),
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
                    items: vec![MenuItem::action("Open Images...", OpenFile)],
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
                    counter_sender: None,
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
                    count_data: None,
                    review_snap_target: None,
                    lasso_state: LassoState::default(),
                    toasts: vec![],
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
                eprintln!("Failed to open main window: {}", e);
                return;
            }

            // Activate the app after window creation
            cx.activate(true);
        });

    Ok(())
}
