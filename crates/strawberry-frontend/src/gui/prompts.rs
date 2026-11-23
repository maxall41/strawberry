#![expect(
    clippy::future_not_send,
    reason = "Native dialogs run on GPUI's main-thread window context"
)]

use std::collections::HashSet;

use gpui::{AsyncWindowContext, PromptLevel};
use strawberry_types::{AnnotationPlacement, ExportScope, Position, Shape};

/// How the user chose to load several images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportChoice {
    /// Place the images as the channels of one image.
    Channels,
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

/// Ask whether several images load as the channels of one image, a stack, or a hyperstack.
pub async fn ask_image_arrangement(
    cx: &mut AsyncWindowContext,
    image_count: usize,
) -> Option<ImportChoice> {
    choose(
        cx,
        &format!("Load {image_count} images as channels, a stack, or a hyperstack?"),
        Some("Stacks have only one dimension. Hyperstacks have multiple dimensions."),
        &[
            ("Channels", ImportChoice::Channels),
            ("Stack", ImportChoice::Stack),
            ("Hyperstack", ImportChoice::Hyperstack),
        ],
    )
    .await
}

/// Ask where annotations read from a file load, given the positions of `shape` they were
/// `saved` on. Annotations saved on several positions load onto the current position or split
/// across positions, and those saved on another loaded position load onto the current or saved
/// one. Annotations saved on positions `shape` lacks cannot be split, so the user is warned and
/// may only load them onto the current position. Otherwise they load onto the `selected`
/// position without asking.
pub async fn ask_annotation_placement(
    cx: &mut AsyncWindowContext,
    annotations: &str,
    saved: &HashSet<Position>,
    selected: Position,
    shape: Shape,
) -> Option<AnnotationPlacement> {
    if shape.position_count() <= 1 {
        return Some(AnnotationPlacement::CurrentPosition);
    }
    let missing = saved
        .iter()
        .filter(|&&position| !shape.contains_position(position))
        .count();
    if missing > 0 {
        return choose(
            cx,
            &format!("These {annotations} were saved on {missing} image(s) that are not loaded."),
            None,
            &[(
                "Load onto Current Image",
                AnnotationPlacement::CurrentPosition,
            )],
        )
        .await;
    }
    let (message, split_label) = match saved.len() {
        0 => return Some(AnnotationPlacement::CurrentPosition),
        1 => {
            if saved.contains(&selected) {
                return Some(AnnotationPlacement::CurrentPosition);
            }
            (
                format!("These {annotations} were saved on another image."),
                "Load onto Saved Image",
            )
        }
        count => (
            format!("These {annotations} span {count} images."),
            "Split Across Images",
        ),
    };
    choose(
        cx,
        &message,
        None,
        &[
            (
                "Load onto Current Image",
                AnnotationPlacement::CurrentPosition,
            ),
            (split_label, AnnotationPlacement::SplitByPosition),
        ],
    )
    .await
}

/// Ask whether to export the displayed image, every image, or, when `shape` has several
/// channels, the channels at the displayed position composited into one image.
pub async fn ask_export_scope(cx: &mut AsyncWindowContext, shape: Shape) -> Option<ExportScope> {
    let current = ("Current Image", ExportScope::CurrentView);
    let all = ("All Images", ExportScope::AllViews);
    let (message, options) = if shape.c > 1 {
        (
            "Export the current image, the composite of its channels, or all images?",
            vec![
                current,
                ("Composite Image", ExportScope::CompositeView),
                all,
            ],
        )
    } else {
        (
            "Export the current image or all images?",
            vec![current, all],
        )
    };
    choose(
        cx,
        message,
        Some("All images are saved as TIFF images in a ZIP archive."),
        &options,
    )
    .await
}
