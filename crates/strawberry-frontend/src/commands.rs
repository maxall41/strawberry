use crate::gui::actions::actions;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{Action, App, AppContext, Entity, Focusable, Render};
use strawberry_types::{BackendToolType, CommandSender, ToolFrontend, ToolId};

use crate::gui::sidebar::{ToolPanel, ToolRegistration, ToolSidebar};
use crate::tools::FrontendMessageHandler;
use crate::tools::automatic_segmentation::AutomaticSegmentationView;
use crate::tools::compositor::CompositorView;
use crate::tools::contrast::ContrastView;
use crate::tools::counter::CounterView;
use crate::tools::decomposer::DecomposerView;
use crate::tools::difference_of_gaussians::DifferenceOfGaussiansView;
use crate::tools::gaussian_blur::GaussianBlurView;
use crate::tools::grayscale::GrayscaleView;
use crate::tools::local_maxima::LocalMaximaView;
use crate::tools::morphology::MorphologyView;
use crate::tools::nervetracer::NerveTracerView;
use crate::tools::object_size_filter::ObjectSizeFilterView;
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
    DifferenceOfGaussiansSettings,
    GaussianBlurSettings,
    GrayscaleSettings,
    ThresholdSettings,
    LocalMaximaSettings,
    MorphologySettings,
    ObjectSizeFilterSettings,
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
fn register_tool<T, A>(
    cx: &mut App,
    sidebar: &Entity<ToolSidebar>,
    action: A,
    label: &'static str,
    kind: BackendToolType,
    view_sender: CommandSender,
    constructor: impl Fn(ToolId, CommandSender, &App) -> T + 'static,
) -> PalleteCommand
where
    T: ToolFrontend + FrontendMessageHandler + Render + Focusable + 'static,
    A: Action + 'static,
{
    sidebar.update(cx, |sidebar, _| {
        sidebar.register(ToolRegistration::new(
            kind.clone(),
            label,
            move |id, window, cx| {
                let mut tool = constructor(id, view_sender.clone(), cx);
                let tool = cx.new(|cx| {
                    cx.on_focus_in(&tool.base().focus_handle, window, |tool: &mut T, _, _| {
                        tool.base().select();
                    })
                    .detach();
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
    tx: CommandSender,
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
            CompositorView::new,
        ),
        register_tool(
            cx,
            sidebar,
            ContrastSettings,
            "Contrast",
            BackendToolType::Contrast,
            tx.clone(),
            ContrastView::new,
        ),
        register_tool(
            cx,
            sidebar,
            CounterSettings,
            "Counter",
            BackendToolType::Counter,
            tx.clone(),
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
            GaussianBlurView::new,
        ),
        register_tool(
            cx,
            sidebar,
            DifferenceOfGaussiansSettings,
            "Difference of Gaussians",
            BackendToolType::DifferenceOfGaussians,
            tx.clone(),
            DifferenceOfGaussiansView::new,
        ),
        register_tool(
            cx,
            sidebar,
            GrayscaleSettings,
            "Grayscale",
            BackendToolType::Grayscale,
            tx.clone(),
            GrayscaleView::new,
        ),
        register_tool(
            cx,
            sidebar,
            ThresholdSettings,
            "Threshold",
            BackendToolType::Threshold,
            tx.clone(),
            ThresholdView::new,
        ),
        register_tool(
            cx,
            sidebar,
            LocalMaximaSettings,
            "Local Maxima",
            BackendToolType::LocalMaxima,
            tx.clone(),
            LocalMaximaView::new,
        ),
        register_tool(
            cx,
            sidebar,
            MorphologySettings,
            "Morphology",
            BackendToolType::Morphology,
            tx.clone(),
            MorphologyView::new,
        ),
        register_tool(
            cx,
            sidebar,
            RollingBallSettings,
            "Rolling Ball",
            BackendToolType::RollingBall,
            tx.clone(),
            RollingBallView::new,
        ),
        register_tool(
            cx,
            sidebar,
            ObjectSizeFilterSettings,
            "Object Size Filter",
            BackendToolType::ObjectSizeFilter,
            tx.clone(),
            ObjectSizeFilterView::new,
        ),
        register_tool(
            cx,
            sidebar,
            AutomaticSegmentationSettings,
            "Automatic Segmentation",
            BackendToolType::AutomaticSegmentation,
            tx.clone(),
            AutomaticSegmentationView::new,
        ),
        register_tool(
            cx,
            sidebar,
            WatershedSettings,
            "Watershed",
            BackendToolType::Watershed,
            tx.clone(),
            WatershedView::new,
        ),
        register_tool(
            cx,
            sidebar,
            DecomposerSettings,
            "Decomposer",
            BackendToolType::Decomposer,
            tx.clone(),
            DecomposerView::new,
        ),
        register_tool(
            cx,
            sidebar,
            SobelSettings,
            "Sobel",
            BackendToolType::Sobel,
            tx.clone(),
            SobelView::new,
        ),
        register_tool(
            cx,
            sidebar,
            NerveTracerSettings,
            "Nerve Tracer",
            BackendToolType::NerveTracer,
            tx,
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
