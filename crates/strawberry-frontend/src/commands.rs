use crate::gui::actions::actions;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{Action, App, AppContext, Entity, Focusable, Render};
use strawberry_types::{BackendToolType, PipelineCommand, ToolFrontend};
use tokio::sync::broadcast;

use crate::gui::sidebar::{ToolPanel, ToolRegistration, ToolSidebar};
use crate::tools::FrontendMessageHandler;
use crate::tools::automatic_segmentation::AutomaticSegmentationView;
use crate::tools::compositor::CompositorView;
use crate::tools::contrast::ContrastView;
use crate::tools::counter::CounterView;
use crate::tools::decomposer::DecomposerView;
use crate::tools::gaussian_blur::GaussianBlurView;
use crate::tools::grayscale::GrayscaleView;
use crate::tools::local_maxima::LocalMaximaView;
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
    ZoomFit,
    ToggleSidebar,
    OpenFile,
    SaveImage,
    ToggleCommandPalette,
]);

actions!([
    AutomaticSegmentationSettings,
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

/// Registers a lazily initialized sidebar view and its command-palette action.
#[expect(
    clippy::too_many_arguments,
    reason = "Keep tool metadata, channels and factory in one registration"
)]
fn register_tool<T, A>(
    cx: &mut App,
    sidebar: &Entity<ToolSidebar>,
    action: A,
    label: &'static str,
    kind: BackendToolType,
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
    let receiver = view_receiver.resubscribe();
    sidebar.update(cx, |sidebar, _| {
        sidebar.register(ToolRegistration::new(
            kind.clone(),
            label,
            move |window, cx| {
                let mut tool = constructor(receiver.resubscribe(), view_sender.clone(), cx);
                let tool = cx.new(|cx| {
                    tool.initialize(window, cx);
                    tool
                });
                ToolPanel {
                    focus: tool.read(cx).focus_handle(cx),
                    view: tool.into(),
                }
            },
        ));
    });
    let sidebar = sidebar.downgrade();
    cx.on_action(move |_: &A, cx| {
        let _ = sidebar.update(cx, |sidebar, cx| sidebar.add_tool(&kind, cx));
    });

    PalleteCommand {
        label,
        hint: CommandHint::Category("Add tool"),
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
    sidebar: &Entity<ToolSidebar>,
    tx: broadcast::Sender<PipelineCommand>,
    rx: &broadcast::Receiver<PipelineCommand>,
    image_directory: Arc<RwLock<Option<PathBuf>>>,
) -> Rc<[PalleteCommand]> {
    let mut commands = vec![
        register_tool(
            cx,
            sidebar,
            CompositorSettings,
            "Compositor",
            BackendToolType::Compositor,
            tx.clone(),
            rx,
            CompositorView::new,
        ),
        register_tool(
            cx,
            sidebar,
            ContrastSettings,
            "Contrast",
            BackendToolType::Contrast,
            tx.clone(),
            rx,
            ContrastView::new,
        ),
        register_tool(
            cx,
            sidebar,
            CounterSettings,
            "Counter",
            BackendToolType::Counter,
            tx.clone(),
            rx,
            move |receiver, sender, cx| {
                CounterView::new(receiver, sender, image_directory.clone(), cx)
            },
        ),
        register_tool(
            cx,
            sidebar,
            GaussianBlurSettings,
            "Gaussian Blur",
            BackendToolType::GaussianBlur,
            tx.clone(),
            rx,
            GaussianBlurView::new,
        ),
        register_tool(
            cx,
            sidebar,
            GrayscaleSettings,
            "Grayscale",
            BackendToolType::Grayscale,
            tx.clone(),
            rx,
            GrayscaleView::new,
        ),
        register_tool(
            cx,
            sidebar,
            ThresholdSettings,
            "Threshold",
            BackendToolType::Threshold,
            tx.clone(),
            rx,
            ThresholdView::new,
        ),
        register_tool(
            cx,
            sidebar,
            LocalMaximaSettings,
            "Local Maxima",
            BackendToolType::LocalMaxima,
            tx.clone(),
            rx,
            LocalMaximaView::new,
        ),
        register_tool(
            cx,
            sidebar,
            MorphologySettings,
            "Morphology",
            BackendToolType::Morphology,
            tx.clone(),
            rx,
            MorphologyView::new,
        ),
        register_tool(
            cx,
            sidebar,
            RollingBallSettings,
            "Rolling Ball",
            BackendToolType::RollingBall,
            tx.clone(),
            rx,
            RollingBallView::new,
        ),
        register_tool(
            cx,
            sidebar,
            AutomaticSegmentationSettings,
            "Automatic Segmentation",
            BackendToolType::AutomaticSegmentation,
            tx.clone(),
            rx,
            AutomaticSegmentationView::new,
        ),
        register_tool(
            cx,
            sidebar,
            WatershedSettings,
            "Watershed",
            BackendToolType::Watershed,
            tx.clone(),
            rx,
            WatershedView::new,
        ),
        register_tool(
            cx,
            sidebar,
            DecomposerSettings,
            "Decomposer",
            BackendToolType::Decomposer,
            tx.clone(),
            rx,
            DecomposerView::new,
        ),
        register_tool(
            cx,
            sidebar,
            SobelSettings,
            "Sobel",
            BackendToolType::Sobel,
            tx.clone(),
            rx,
            SobelView::new,
        ),
        register_tool(
            cx,
            sidebar,
            NerveTracerSettings,
            "Nerve Tracer",
            BackendToolType::NerveTracer,
            tx,
            rx,
            NerveTracerView::new,
        ),
    ];

    let sidebar = sidebar.downgrade();
    cx.on_action(move |_: &ManagerSettings, cx| {
        let _ = sidebar.update(cx, ToolSidebar::reveal);
    });

    commands.extend([
        PalleteCommand {
            label: "Pipeline",
            hint: CommandHint::Category("Workspace"),
            action: Box::new(ManagerSettings),
        },
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
            label: "Fit Image to View",
            hint: CommandHint::Shortcut("F"),
            action: Box::new(ZoomFit),
        },
        PalleteCommand {
            label: "Toggle Properties Sidebar",
            hint: CommandHint::Shortcut("⌘⌥B"),
            action: Box::new(ToggleSidebar),
        },
        PalleteCommand {
            label: "Quit",
            hint: CommandHint::Shortcut("⌘Q"),
            action: Box::new(Quit),
        },
    ]);

    commands.into()
}
