#![expect(
    clippy::future_not_send,
    reason = "Native dialogs run on GPUI's main-thread window context"
)]

use gpui::{AsyncWindowContext, PromptLevel};
use strawberry_types::{AnnotationPlacement, ExportScope};

/// How the user chose to load several images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportChoice {
    /// Place the images as the channels of one image.
    Layers,
    /// Place the images along Z.
    Stack,
    /// Place the images by parsing their file names.
    Hyperstack,
}

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

/// Ask whether several images load as layers of one image, a stack, or a hyperstack.
pub async fn ask_image_arrangement(
    cx: &mut AsyncWindowContext,
    image_count: usize,
) -> Option<ImportChoice> {
    choose(
        cx,
        &format!("Load {image_count} images as layers, a stack, or a hyperstack?"),
        Some(
            "Layers become the channels of one image. Stacks have only one dimension. Hyperstacks have multiple dimensions.",
        ),
        &[
            ("Layers", ImportChoice::Layers),
            ("Stack", ImportChoice::Stack),
            ("Hyperstack", ImportChoice::Hyperstack),
        ],
    )
    .await
}

/// Ask whether annotations spanning several images load onto the current image or split
/// across images; annotations on a single image always load onto the current one.
pub async fn ask_annotation_placement(
    cx: &mut AsyncWindowContext,
    annotations: &str,
    images: usize,
) -> Option<AnnotationPlacement> {
    if images <= 1 {
        return Some(AnnotationPlacement::CurrentPosition);
    }
    choose(
        cx,
        &format!("These {annotations} span {images} images."),
        None,
        &[
            (
                "Load onto Current Image",
                AnnotationPlacement::CurrentPosition,
            ),
            ("Split Across Images", AnnotationPlacement::SplitByPosition),
        ],
    )
    .await
}

/// Ask whether to export the displayed image or every image.
pub async fn ask_export_scope(cx: &mut AsyncWindowContext) -> Option<ExportScope> {
    choose(
        cx,
        "Export the current image or all images?",
        Some("All images are saved as TIFF images in a ZIP archive."),
        &[
            ("Current Image", ExportScope::CurrentView),
            ("All Images", ExportScope::AllViews),
        ],
    )
    .await
}
