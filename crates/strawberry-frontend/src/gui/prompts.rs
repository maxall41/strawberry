#![expect(
    clippy::future_not_send,
    reason = "Native dialogs run on GPUI's main-thread window context"
)]

use gpui::{AsyncWindowContext, PromptLevel};
use strawberry_types::{ExportScope, ImageArrangement, StackPlacement};

/// Show a native choice dialog with a Cancel button; `None` means the user cancelled.
async fn choose<T: Copy>(
    cx: &mut AsyncWindowContext,
    message: &str,
    detail: Option<&str>,
    options: &[(&str, T)],
) -> Option<T> {
    let labels: Vec<&str> = options
        .iter()
        .map(|(label, _)| *label)
        .chain(["Cancel"])
        .collect();
    let answer = cx
        .prompt(PromptLevel::Info, message, detail, &labels)
        .await
        .ok()?;
    options.get(answer).map(|(_, value)| *value)
}

/// Ask whether several images load as layers of one image or as a stack.
pub async fn ask_image_arrangement(
    cx: &mut AsyncWindowContext,
    image_count: usize,
) -> Option<ImageArrangement> {
    choose(
        cx,
        &format!("Load {image_count} images as layers or as a stack?"),
        Some("Layers are composited into one image. A stack keeps each image separate."),
        &[
            ("Layers", ImageArrangement::Layers),
            ("Stack", ImageArrangement::Stacks),
        ],
    )
    .await
}

/// Ask whether annotations spanning several Z indices load onto the selected stack or split by Z.
pub async fn ask_stack_placement(
    cx: &mut AsyncWindowContext,
    annotations: &str,
    z_indices: usize,
) -> Option<StackPlacement> {
    choose(
        cx,
        &format!("These {annotations} span {z_indices} Z indices."),
        None,
        &[
            ("Load onto Selected Stack", StackPlacement::SelectedStack),
            ("Split by Z Index", StackPlacement::SplitByZ),
        ],
    )
    .await
}

/// Ask whether to export the selected stack or every stack.
pub async fn ask_export_scope(cx: &mut AsyncWindowContext) -> Option<ExportScope> {
    choose(
        cx,
        "Export the selected stack or all stacks?",
        Some("All stacks are saved as TIFF images in a ZIP archive."),
        &[
            ("Selected Stack", ExportScope::SelectedStack),
            ("All Stacks", ExportScope::AllStacks),
        ],
    )
    .await
}
