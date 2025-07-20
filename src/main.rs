use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use composite::GpuCompositor;
use gpui::{
    actions, div, img, prelude::*, px, size, App, AppContext, Application, AssetSource, AsyncApp,
    Bounds, Context, FocusHandle, Focusable, ImageSource, KeyBinding, KeyDownEvent, KeyUpEvent,
    Menu, MenuItem, Point, RenderImage, SharedString, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions,
};
use image::{Frame, ImageReader, RgbaImage};
use rfd::FileDialog;

struct Assets {
    base: PathBuf,
}

mod composite;

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
    image: ImageSource,
    zoom_level: f32,
    original_width: u32,
    original_height: u32,
    pan_x: f32,
    pan_y: f32,
    pressed_keys: std::collections::HashSet<String>,
}

impl PrimaryViewer {
    fn create_default_image() -> ImageSource {
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
        ImageSource::Render(img_data)
    }

    fn load_image_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(files) = FileDialog::new()
            .add_filter("image", &["tif", "tiff", "png", "jpg", "jpeg", "bmp"])
            .set_directory("/")
            .pick_files()
        {
            cx.spawn_in(window, async move |this, cx| {
                let start = std::time::Instant::now();
                let compositer = GpuCompositor::new().await;
                let init_duration = start.elapsed();
                println!("GpuCompositor initialization took: {:?}", init_duration);
                let mut loaded_images = Vec::new();

                let load_start = std::time::Instant::now();
                for file in files {
                    if let Ok(reader) = ImageReader::open(&file) {
                        if let Ok(decoded) = reader.decode() {
                            let data = decoded.to_rgba8();
                            loaded_images.push(data);
                        }
                    }
                }
                let load_duration = load_start.elapsed();
                println!(
                    "Loading {} images took: {:?}",
                    loaded_images.len(),
                    load_duration
                );

                if loaded_images.is_empty() {
                    return;
                }

                let composite_start = std::time::Instant::now();
                if let Some(mut composited) = compositer.composite_images(&loaded_images).await {
                    let composite_duration = composite_start.elapsed();
                    println!("Compositing took: {:?}", composite_duration);

                    let w = composited.width();
                    let h = composited.height();

                    let frame = Frame::new(composited);
                    let img_frames = vec![frame];
                    let img_data = Arc::new(RenderImage::new(img_frames));

                    this.update(cx, |this, cx| {
                        this.image = ImageSource::Render(img_data);
                        this.zoom_level = 1.0;
                        this.pan_x = 0.0;
                        this.pan_y = 0.0;

                        this.original_width = w;
                        this.original_height = h;

                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    fn handle_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.load_image_file(_window, cx);
    }

    fn zoom_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level = self.zoom_level * 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        self.update_image(cx);
    }

    fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level = self.zoom_level / 1.25;
        self.adjust_pan_for_center_zoom(window, old_zoom, self.zoom_level);
        self.update_image(cx);
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

    fn update_image(&mut self, cx: &mut Context<Self>) {
        // Just notify for re-render - let GPUI handle the scaling
        cx.notify();
    }

    fn handle_zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        println!("Zooming in");
        self.zoom_in(window, cx);
    }

    fn handle_zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        println!("Zooming out");
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
}

impl Focusable for PrimaryViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PrimaryViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Ensure this element is always focused
        window.focus(&self.focus_handle(cx));

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
            .child(
                div()
                    .absolute()
                    .left(px(self.pan_x))
                    .top(px(self.pan_y))
                    .child(
                        img(self.image.clone())
                            .use_nearest_filter(true)
                            .w(px(self.original_width as f32 * self.zoom_level))
                            .h(px(self.original_height as f32 * self.zoom_level)),
                    ),
            )
    }
}

actions!(image, [Quit, ZoomIn, ZoomOut, OpenFile]);

#[tokio::main]
async fn main() {
    env_logger::init();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    Application::new()
        .with_assets(Assets {
            base: manifest_dir.join("examples"),
        })
        .run(move |cx: &mut App| {
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
            ]);

            let window_options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Image Example")),
                    appears_transparent: false,
                    ..Default::default()
                }),

                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    size: size(px(1100.), px(600.)),
                    origin: Point::new(px(200.), px(200.)),
                })),

                ..Default::default()
            };

            cx.open_window(window_options, |window, cx| {
                let viewer = cx.new(|cx| PrimaryViewer {
                    focus_handle: cx.focus_handle(),
                    image: PrimaryViewer::create_default_image(),
                    zoom_level: 1.0,
                    original_width: 800,
                    original_height: 600,
                    pan_x: 0.0,
                    pan_y: 0.0,
                    pressed_keys: std::collections::HashSet::new(),
                });

                // Bind keys locally to the viewer
                cx.bind_keys([
                    KeyBinding::new("=", ZoomIn, Some("PrimaryViewer")),
                    KeyBinding::new("-", ZoomOut, Some("PrimaryViewer")),
                ]);

                // Focus the viewer so it can receive key events
                window.focus(&viewer.read(cx).focus_handle(cx));

                viewer
            })
            .unwrap();

            // Activate the app after window creation
            cx.activate(true);
        });
}
