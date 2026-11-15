use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FocusHandle, Interactivity, IntoElement,
    RenderOnce, Rgba, SharedString, Stateful, StyleRefinement, Window, div, prelude::*, px,
};

use super::{icon::Icon, theme, tooltip::TooltipView};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, Default)]
pub enum ButtonVariant {
    #[default]
    Secondary,
    Primary,
    Success,
    Danger,
    /// Compact red cross for removal actions; provide an accessible label.
    Delete,
    Ghost,
}

impl ButtonVariant {
    fn colors(self) -> (Rgba, Rgba, Rgba) {
        match self {
            Self::Secondary => (theme::CONTROL, theme::CONTROL_HOVER, theme::CONTROL_ACTIVE),
            Self::Primary => (theme::ACCENT, theme::ACCENT_HOVER, theme::ACCENT_ACTIVE),
            Self::Success => (theme::SUCCESS, theme::SUCCESS_HOVER, theme::SUCCESS_ACTIVE),
            Self::Danger | Self::Delete => {
                (theme::DANGER, theme::DANGER_HOVER, theme::DANGER_ACTIVE)
            }
            Self::Ghost => (
                gpui::transparent_black().into(),
                theme::CONTROL_HOVER,
                theme::CONTROL_ACTIVE,
            ),
        }
    }
}

/// Shared action control. IDs must be stable and unique among sibling elements.
/// Activation uses GPUI clicks (release inside, Enter, or Space), never mouse-down.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    element: Stateful<Div>,
    variant: ButtonVariant,
    selected: Option<bool>,
    disabled: bool,
    tooltip_text: Option<SharedString>,
    on_click: Option<ClickHandler>,
    focus_handle: Option<FocusHandle>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            id: id.clone(),
            element: div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .min_h(px(30.))
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme::BORDER_MUTED)
                .text_sm()
                .text_color(theme::TEXT)
                .font_weight(gpui::FontWeight::MEDIUM),
            variant: ButtonVariant::default(),
            selected: None,
            disabled: false,
            tooltip_text: None,
            on_click: None,
            focus_handle: None,
        }
    }

    pub fn focus_handle(mut self, focus: &FocusHandle) -> Self {
        self.focus_handle = Some(focus.clone());
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        self.element = self.element.aria_label(label.clone()).child(label);
        self
    }

    pub const fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub const fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn compact(mut self) -> Self {
        self.element = self.element.min_h(px(26.)).px_2().text_xs();
        self
    }

    pub fn large(mut self) -> Self {
        self.element = self.element.min_h(px(36.)).px_4().py_2();
        self
    }

    /// Square icon control with an accessible name.
    pub fn icon(self, icon: impl IntoElement, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        self.aria_label(label)
            .flex_none()
            .p_0()
            .min_w(px(28.))
            .min_h(px(28.))
            .child(icon)
    }

    pub fn tooltip_text(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip_text = Some(text.into());
        self
    }

    pub fn on_click(
        mut self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(listener));
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements);
    }
}

impl InteractiveElement for Button {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl StatefulInteractiveElement for Button {}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = if let Some(focus) = self.focus_handle {
            focus
        } else {
            let focus = window.use_keyed_state(self.id, cx, |_, cx| cx.focus_handle());
            focus.read(cx).clone()
        };
        let enabled = !self.disabled;
        let mouse_focus = focus.clone();
        let variant = if self.selected == Some(true) {
            ButtonVariant::Primary
        } else {
            self.variant
        };
        let (background, hover, active) = variant.colors();
        let colored = matches!(
            variant,
            ButtonVariant::Primary
                | ButtonVariant::Success
                | ButtonVariant::Danger
                | ButtonVariant::Delete
        );
        self.element
            .role(gpui::Role::Button)
            .when(matches!(variant, ButtonVariant::Delete), |el| {
                el.flex_none()
                    .p_0()
                    .size(px(20.))
                    .min_w(px(20.))
                    .min_h(px(20.))
                    .rounded_sm()
                    .child(Icon::x())
            })
            .when_some(self.tooltip_text, |el, text| {
                el.tooltip(move |_, cx| {
                    cx.new(|_| TooltipView {
                        text: text.to_string(),
                    })
                    .into()
                })
            })
            .when_some(self.selected, |el, selected| {
                el.aria_toggled(if selected {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
            })
            // Keep containing tool panels and the image canvas from taking focus
            // or starting gestures when a button is pressed.
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                if enabled {
                    mouse_focus.focus(window, cx);
                }
                cx.stop_propagation();
            })
            .bg(if self.disabled {
                theme::CONTROL_ACTIVE
            } else {
                background
            })
            .when(colored || matches!(variant, ButtonVariant::Ghost), |el| {
                el.border_color(gpui::transparent_black())
            })
            .when(self.disabled, |el| {
                el.text_color(theme::TEXT_MUTED)
                    .opacity(0.6)
                    .cursor(gpui::CursorStyle::Arrow)
            })
            .when(!self.disabled, |el| {
                el.cursor_pointer()
                    .track_focus(&focus)
                    .hover(move |style| {
                        let style = style.bg(hover);
                        if colored {
                            style
                        } else {
                            style.border_color(theme::BORDER)
                        }
                    })
                    .active(move |style| style.bg(active))
                    .focus_visible(|style| style.border_color(theme::TEXT))
                    .when_some(self.on_click, |el, listener| {
                        el.on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            listener(event, window, cx);
                        })
                    })
            })
    }
}
