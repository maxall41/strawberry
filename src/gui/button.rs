use gpui::{div, prelude::*, rgb, AnyView, Context, IntoElement, MouseDownEvent, RenderOnce};

struct TooltipView {
    text: String,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_1()
            .bg(rgb(0x333333))
            .border_2()
            .border_color(rgb(0x979797))
            .text_color(rgb(0xffffff))
            .rounded_md()
            .text_xs()
            .child(self.text.clone())
    }
}

pub struct ToggleButton<V: 'static> {
    pub label: String,
    pub is_enabled: bool,
    pub enabled_text: String,
    pub disabled_text: String,
    pub on_click:
        Box<dyn Fn(&mut V, &MouseDownEvent, &mut gpui::Window, &mut Context<V>) + 'static>,
    pub keyboard_shortcut: Option<String>,
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

    pub fn render(self, cx: &mut Context<V>) -> impl IntoElement {
        let label = self.label;
        let is_enabled = self.is_enabled;
        let button_text = if is_enabled {
            self.enabled_text
        } else {
            self.disabled_text
        };

        div()
            .id("button")
            .flex()
            .items_center()
            .gap_2()
            .mb_3()
            .child(format!("{label}:"))
            .when(self.keyboard_shortcut.is_some(), |this| {
                this.tooltip(move |_window, cx| {
                    let tooltip_view = TooltipView {
                        text: self
                            .keyboard_shortcut
                            .clone()
                            .expect("Should never fire: Checked is some"),
                    };
                    cx.new(|_cx| tooltip_view).into()
                })
            })
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_enabled {
                        rgb(0x00aa00)
                    } else {
                        rgb(0xaa0000)
                    })
                    .hover(|style| {
                        style.bg(if is_enabled {
                            rgb(0x00bb00) // Slightly brighter green on hover
                        } else {
                            rgb(0xbb0000) // Slightly brighter red on hover
                        })
                    })
                    .child(button_text)
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(self.on_click)),
            )
    }
}
