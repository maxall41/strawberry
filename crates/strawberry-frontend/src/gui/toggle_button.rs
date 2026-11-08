use gpui::{Context, ElementId, IntoElement, MouseDownEvent, SharedString, div, prelude::*, px};

use super::theme;
use super::tooltip::TooltipView;

type ClickHandler<V> =
    Box<dyn Fn(&mut V, &MouseDownEvent, &mut gpui::Window, &mut Context<V>) + 'static>;

pub struct ToggleButton<V: 'static> {
    pub label: String,
    pub is_enabled: bool,
    pub enabled_text: String,
    pub disabled_text: String,
    pub on_click: ClickHandler<V>,
    pub keyboard_shortcut: Option<String>,
    pub disable_margin: bool,
}

impl<V: 'static> ToggleButton<V> {
    pub fn new(
        label: String,
        is_enabled: bool,
        on_click: impl Fn(&mut V, &MouseDownEvent, &mut gpui::Window, &mut Context<V>) + 'static,
    ) -> Self {
        Self {
            label,
            is_enabled,
            enabled_text: "Yes".to_string(),
            disabled_text: "No".to_string(),
            on_click: Box::new(on_click),
            keyboard_shortcut: None,
            disable_margin: false,
        }
    }

    pub fn with_keyboard_shortcut(mut self, shortcut: &str) -> Self {
        self.keyboard_shortcut = Some(shortcut.to_string());
        self
    }

    pub fn with_text(mut self, enabled_text: String, disabled_text: String) -> Self {
        self.enabled_text = enabled_text;
        self.disabled_text = disabled_text;
        self
    }

    pub const fn with_no_margin(mut self) -> Self {
        self.disable_margin = true;
        self
    }

    pub fn render(self, cx: &Context<V>) -> impl IntoElement + use<V> {
        let label = self.label;
        let is_enabled = self.is_enabled;
        let state_text = if is_enabled {
            self.enabled_text
        } else {
            self.disabled_text
        };

        div()
            .id(ElementId::Name(SharedString::new(format!(
                "button-{label}"
            ))))
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .when(!self.disable_margin, gpui::Styled::mb_3)
            .child(label)
            .when_some(self.keyboard_shortcut, |this, shortcut| {
                this.tooltip(move |_window, cx| {
                    let tooltip_view = TooltipView {
                        text: shortcut.clone(),
                    };
                    cx.new(|_cx| tooltip_view).into()
                })
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(state_text),
                    )
                    .child(
                        div()
                            .id("toggle-switch")
                            .group("toggle-switch")
                            .p_1()
                            .m_neg_1()
                            .cursor_pointer()
                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(self.on_click))
                            .child(
                                div()
                                    .id("toggle-track")
                                    .w(px(34.))
                                    .h(px(20.))
                                    .rounded_full()
                                    .p(px(2.))
                                    .bg(if is_enabled {
                                        theme::ACCENT
                                    } else {
                                        theme::CONTROL
                                    })
                                    .group_hover("toggle-switch", |style| {
                                        style.bg(if is_enabled {
                                            theme::ACCENT_HOVER
                                        } else {
                                            theme::CONTROL_HOVER
                                        })
                                    })
                                    .flex()
                                    .items_center()
                                    .when(is_enabled, gpui::Styled::justify_end)
                                    .child(div().size(px(16.)).rounded_full().bg(theme::TEXT)),
                            ),
                    ),
            )
    }
}
