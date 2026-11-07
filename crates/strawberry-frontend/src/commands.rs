use crate::gui::actions::actions;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{
    Action, AnyWindowHandle, App, Context, Entity, Focusable, Global, Render, Window, div,
    prelude::*,
};
use strawberry_types::{BackendToolType, PipelineCommand, ToolFrontend};
use tokio::sync::broadcast;

use crate::gui::busy::BusyOverlay;
use crate::tools::FrontendMessageHandler;
use crate::tools::compositor::CompositorView;
use crate::tools::contrast::ContrastView;
use crate::tools::counter::CounterView;
use crate::tools::decomposer::DecomposerView;
use crate::tools::gaussian_blur::GaussianBlurView;
use crate::tools::grayscale::GrayscaleView;
use crate::tools::local_maxima::LocalMaximaView;
use crate::tools::manager::ManagerView;
use crate::tools::morphology::MorphologyView;
use crate::tools::nervetracer::NerveTracerView;
use crate::tools::rolling_ball::RollingBallView;
use crate::tools::sobel::SobelView;
use crate::tools::threshold::ThresholdView;
use crate::tools::watershed::WatershedView;

actions!([
    Quit,
    ZoomIn,
    ZoomOut,
    OpenFile,
    SaveImage,
    ToggleCommandPalette,
    PalettePaste,
]);

actions!([
    CompositorSettings,
    ContrastSettings,
    CounterSettings,
    GaussianBlurSettings,
    GrayscaleSettings,
    ThresholdSettings,
    LocalMaximaSettings,
    MorphologySettings,
    RollingBallSettings,
    WatershedSettings,
    ManagerSettings,
    DecomposerSettings,
    SobelSettings,
    NerveTracerSettings,
]);

/// The right-aligned text on a palette row: e.g., "Tool" or keyboard shortcut
pub enum CommandHint {
    /// A key equivalent, e.g. "⌘S".
    Shortcut(&'static str),
    /// A grouping label, e.g. "Tool".
    Category(&'static str),
}

impl CommandHint {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Shortcut(text) | Self::Category(text) => text,
        }
    }
}

pub struct PalleteCommand {
    pub label: &'static str,
    pub hint: CommandHint,
    pub action: Box<dyn Action>,
}

#[derive(Default)]
struct OpenToolWindows(HashMap<BackendToolType, AnyWindowHandle>);
impl Global for OpenToolWindows {}

/// Hosts a tool view plus the blocking busy overlay that locks the window's controls while a slow pipeline run is in flight.
struct ToolWindowRoot<T> {
    tool: Entity<T>,
    busy_overlay: Entity<BusyOverlay>,
}

impl<T: Render> Render for ToolWindowRoot<T> {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.tool.clone())
            .child(self.busy_overlay.clone())
    }
}

fn open_tool_window<T>(mut tool: T, cx: &mut App)
where
    T: ToolFrontend + FrontendMessageHandler + Render + Focusable + 'static,
{
    let backend_type = tool.base().backend_type.clone();

    if let Some(existing) = cx
        .try_global::<OpenToolWindows>()
        .and_then(|open| open.0.get(&backend_type))
        .copied()
    {
        if cx
            .windows()
            .iter()
            .any(|window| window.window_id() == existing.window_id())
        {
            cx.defer(move |cx| {
                if let Err(e) = existing.update(cx, |_, window, _| window.activate_window()) {
                    eprintln!("Failed to focus existing tool window: {e}");
                }
            });
            return;
        }
        cx.default_global::<OpenToolWindows>()
            .0
            .remove(&backend_type);
    }

    match cx.open_window(tool.get_window_options(), |window, cx| {
        let tool = cx.new(|cx| {
            tool.initialize(window, cx);
            tool
        });
        window.focus(&tool.read(cx).focus_handle(cx), cx);
        cx.new(|cx| ToolWindowRoot {
            tool,
            busy_overlay: cx.new(|cx| BusyOverlay::new(true, cx)),
        })
    }) {
        Ok(handle) => {
            cx.default_global::<OpenToolWindows>()
                .0
                .insert(backend_type, handle.into());
        }
        Err(e) => eprintln!("Failed to open tool window: {e}"),
    }
}

fn track_window_closes(cx: &App) {
    cx.on_window_closed(|cx, window_id| {
        let tracked = cx
            .try_global::<OpenToolWindows>()
            .is_some_and(|open| open.0.values().any(|w| w.window_id() == window_id));
        if tracked {
            cx.default_global::<OpenToolWindows>()
                .0
                .retain(|_, w| w.window_id() != window_id);
        }
    })
    .detach();
}

/// Installs the action handler that opens `T`'s window and returns its palette entry.
fn register_tool<T, A>(
    cx: &mut App,
    action: A,
    label: &'static str,
    view_sender: broadcast::Sender<PipelineCommand>,
    view_receiver: &broadcast::Receiver<PipelineCommand>,
    constructor: impl Fn(
        broadcast::Receiver<PipelineCommand>,
        broadcast::Sender<PipelineCommand>,
        &App,
    ) -> T
    + 'static,
) -> PalleteCommand
where
    T: ToolFrontend + FrontendMessageHandler + Render + Focusable + 'static,
    A: Action + 'static,
{
    let sender = view_sender;
    let receiver = view_receiver.resubscribe();

    cx.on_action(move |_: &A, cx| {
        open_tool_window(constructor(receiver.resubscribe(), sender.clone(), cx), cx);
    });

    PalleteCommand {
        label,
        hint: CommandHint::Category("Tool"),
        action: Box::new(action),
    }
}

/// Installs every action handler and returns the palette's command list.
#[expect(
    clippy::too_many_lines,
    reason = "Keep declarative GPUI layout and its event bindings together"
)]
pub fn install(
    cx: &mut App,
    tx: broadcast::Sender<PipelineCommand>,
    rx: &broadcast::Receiver<PipelineCommand>,
    image_directory: Arc<RwLock<Option<PathBuf>>>,
) -> Rc<[PalleteCommand]> {
    track_window_closes(cx);

    let mut commands = vec![
        register_tool(
            cx,
            CompositorSettings,
            "Compositor",
            tx.clone(),
            rx,
            CompositorView::new,
        ),
        register_tool(
            cx,
            ContrastSettings,
            "Contrast",
            tx.clone(),
            rx,
            ContrastView::new,
        ),
        register_tool(
            cx,
            CounterSettings,
            "Counter",
            tx.clone(),
            rx,
            move |receiver, sender, cx| {
                CounterView::new(receiver, sender, image_directory.clone(), cx)
            },
        ),
        register_tool(
            cx,
            GaussianBlurSettings,
            "Gaussian Blur",
            tx.clone(),
            rx,
            GaussianBlurView::new,
        ),
        register_tool(
            cx,
            GrayscaleSettings,
            "Grayscale",
            tx.clone(),
            rx,
            GrayscaleView::new,
        ),
        register_tool(
            cx,
            ThresholdSettings,
            "Threshold",
            tx.clone(),
            rx,
            ThresholdView::new,
        ),
        register_tool(
            cx,
            LocalMaximaSettings,
            "Local Maxima",
            tx.clone(),
            rx,
            LocalMaximaView::new,
        ),
        register_tool(
            cx,
            MorphologySettings,
            "Morphology",
            tx.clone(),
            rx,
            MorphologyView::new,
        ),
        register_tool(
            cx,
            RollingBallSettings,
            "Rolling Ball",
            tx.clone(),
            rx,
            RollingBallView::new,
        ),
        register_tool(
            cx,
            WatershedSettings,
            "Watershed",
            tx.clone(),
            rx,
            WatershedView::new,
        ),
        register_tool(
            cx,
            ManagerSettings,
            "Pipeline",
            tx.clone(),
            rx,
            ManagerView::new,
        ),
        register_tool(
            cx,
            DecomposerSettings,
            "Decomposer",
            tx.clone(),
            rx,
            DecomposerView::new,
        ),
        register_tool(cx, SobelSettings, "Sobel", tx.clone(), rx, SobelView::new),
        register_tool(
            cx,
            NerveTracerSettings,
            "Nerve Tracer",
            tx,
            rx,
            NerveTracerView::new,
        ),
    ];

    commands.extend([
        PalleteCommand {
            label: "Load Images…",
            hint: CommandHint::Shortcut("⌘L"),
            action: Box::new(OpenFile),
        },
        PalleteCommand {
            label: "Save Image",
            hint: CommandHint::Shortcut("⌘S"),
            action: Box::new(SaveImage),
        },
        PalleteCommand {
            label: "Zoom In",
            hint: CommandHint::Shortcut("+"),
            action: Box::new(ZoomIn),
        },
        PalleteCommand {
            label: "Zoom Out",
            hint: CommandHint::Shortcut("−"),
            action: Box::new(ZoomOut),
        },
        PalleteCommand {
            label: "Quit",
            hint: CommandHint::Shortcut("⌘Q"),
            action: Box::new(Quit),
        },
    ]);

    commands.into()
}
