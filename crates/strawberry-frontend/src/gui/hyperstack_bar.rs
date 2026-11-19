use gpui::{Context, Entity, EventEmitter, Window, div, prelude::*, px};
use num_traits::ToPrimitive;
use strawberry_types::{Coordinates, Dimension, Shape};

use super::{
    slider::{Slider, SliderEvent},
    theme,
    value_control::{ValueControl, ValueControlConfig, ValueControlEvent},
    workspace::{STATUS_BAR_PADDING, STATUS_BAR_ROW_HEIGHT, status_bar_height},
};

/// Set a fixed extent width to ensure that all slider bars line up. This value should fit up to 9999 images per dimension.
const EXTENT_WIDTH: f32 = 40.0;

/// Emitted when the user picks an index along one dimension.
pub struct IndexSelected(pub usize);

impl EventEmitter<IndexSelected> for DimensionControl {}

/// A slider and one-based number for moving along one hyperstack dimension.
pub struct DimensionControl {
    dimension: Dimension,
    slider: Entity<Slider>,
    /// One-based index, matching how images are numbered for the user.
    number: Entity<ValueControl>,
    extent: usize,
}

impl DimensionControl {
    /// Create the control and keep its slider and number in step.
    pub fn new(dimension: Dimension, cx: &mut Context<Self>) -> Self {
        let id = dimension.label().to_lowercase();
        let slider = cx.new(|_| Slider::new(format!("{id}-slider"), 0, 0));
        let number = cx.new(|cx| {
            ValueControl::new_integer(
                1,
                ValueControlConfig {
                    min: 1.0,
                    max: 1.0,
                    label: String::new(),
                    step: 1.0,
                    id: format!("{id}-number"),
                },
                cx,
            )
        });
        cx.subscribe(&slider, |this, _, event: &SliderEvent, cx| {
            this.select_index(usize::try_from(event.0).unwrap_or_default(), cx);
        })
        .detach();
        cx.subscribe(&number, |this, _, event: &ValueControlEvent, cx| {
            if let Some(number) = event.integer_value.and_then(|number| number.checked_sub(1)) {
                this.select_index(usize::try_from(number).unwrap_or_default(), cx);
            }
        })
        .detach();
        Self {
            dimension,
            slider,
            number,
            extent: 0,
        }
    }

    /// Show the dimension's extent and selected index without reporting a selection.
    pub fn set_extent(&mut self, index: usize, extent: usize, cx: &mut Context<Self>) {
        self.extent = extent;
        let last = u32::try_from(extent.saturating_sub(1)).unwrap_or(u32::MAX);
        let index = u32::try_from(index).unwrap_or(last).min(last);
        self.slider.update(cx, |slider, cx| {
            slider.set_range(index, last);
            cx.notify();
        });
        self.number.update(cx, |number, cx| {
            number.config.max = last.saturating_add(1).to_f32().unwrap_or(f32::MAX);
            number.set_integer_value(index.saturating_add(1));
            cx.notify();
        });
        cx.notify();
    }

    /// Mirror an index chosen in one control in the other and report the selection.
    fn select_index(&self, index: usize, cx: &mut Context<Self>) {
        let value = u32::try_from(index).unwrap_or(u32::MAX);
        self.slider.update(cx, |slider, cx| {
            slider.set_value(value);
            cx.notify();
        });
        self.number.update(cx, |number, cx| {
            number.set_integer_value(value.saturating_add(1));
            cx.notify();
        });
        cx.emit(IndexSelected(index));
    }
}

impl Render for DimensionControl {
    /// Build the dimension label, slider, number, and extent.
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(12.))
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(self.dimension.label()),
            )
            .child(div().flex_1().min_w_0().child(self.slider.clone()))
            .child(self.number.clone())
            .child(
                div()
                    .flex_none()
                    .w(px(EXTENT_WIDTH))
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("/ {}", self.extent)),
            )
    }
}

/// Emitted when the user picks another image.
pub struct ViewSelected(pub Coordinates);

impl EventEmitter<ViewSelected> for HyperstackBar {}

/// Status bar with a row for each dimension holding several images.
pub struct HyperstackBar {
    controls: Vec<(Dimension, Entity<DimensionControl>)>,
    shape: Shape,
    coordinates: Coordinates,
}

impl HyperstackBar {
    /// Create a control for every dimension; only those holding several images are shown.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let controls = Dimension::ALL
            .into_iter()
            .map(|dimension| {
                let control = cx.new(|cx| DimensionControl::new(dimension, cx));
                cx.subscribe(&control, move |this, _, event: &IndexSelected, cx| {
                    this.coordinates.set(dimension, event.0);
                    cx.emit(ViewSelected(this.coordinates));
                    cx.notify();
                })
                .detach();
                (dimension, control)
            })
            .collect();
        Self {
            controls,
            shape: Shape::default(),
            coordinates: Coordinates::default(),
        }
    }

    /// Show the displayed image's coordinates within `shape` without reporting a selection.
    pub fn set_view(&mut self, coordinates: Coordinates, shape: Shape, cx: &mut Context<Self>) {
        self.coordinates = coordinates;
        self.shape = shape;
        for (dimension, control) in &self.controls {
            control.update(cx, |control, cx| {
                control.set_extent(coordinates.get(*dimension), shape.extent(*dimension), cx);
            });
        }
        cx.notify();
    }

    /// Return the number of rows shown, one per dimension holding several images.
    pub fn row_count(&self) -> usize {
        Dimension::ALL
            .into_iter()
            .filter(|&dimension| self.shape.extent(dimension) > 1)
            .count()
    }
}

impl Render for HyperstackBar {
    /// Build one row per dimension holding several images.
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .controls
            .iter()
            .filter(|(dimension, _)| self.shape.extent(*dimension) > 1)
            .map(|(_, control)| {
                div()
                    .flex()
                    .items_center()
                    .h(px(STATUS_BAR_ROW_HEIGHT))
                    .child(control.clone())
            });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(status_bar_height(self.row_count())))
            .px_3()
            .py(px(STATUS_BAR_PADDING))
            .border_t_1()
            .border_color(theme::BORDER_MUTED)
            .bg(theme::WINDOW_BG)
            .children(rows)
    }
}
