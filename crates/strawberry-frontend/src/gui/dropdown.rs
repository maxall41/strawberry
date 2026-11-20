use std::rc::Rc;

use gpui::{
    Anchored, App, Div, ElementId, Entity, IntoElement, MouseDownEvent, Pixels, Point, RenderOnce,
    Role, SharedString, Stateful, StyleRefinement, Window, anchored, canvas, deferred, div,
    prelude::*, px,
};

use super::{
    Button,
    generic_picker::{build_picker_row, picker_surface},
    icon::Icon,
    theme,
    tool_panel::{ListDirection, next_list_index},
};

/// Matches the height of compact buttons, the size of the trigger.
const OPTION_HEIGHT: f32 = 26.0;

type ToggleHandler = dyn Fn(&bool, &mut Window, &mut App);
type SelectHandler = dyn Fn(&usize, &mut Window, &mut App);

/// Interaction state that only the dropdown needs, kept across frames under its ID.
#[derive(Default)]
struct DropdownState {
    /// The option arrow keys moved to, kept apart from the selection so Escape can still back
    /// out; `None` rests on the selected option.
    highlight: Option<usize>,
    /// The trigger's width in the last frame: the narrowest the menu may be.
    trigger_width: Pixels,
}

/// Move the keyboard highlight and redraw; `None` returns it to the selected option.
fn set_highlight(state: &Entity<DropdownState>, highlight: Option<usize>, cx: &mut App) {
    state.update(cx, |state, cx| {
        state.highlight = highlight;
        cx.notify();
    });
}

/// Compact select control: a button showing the selected option that opens a menu of options
/// below it. The parent owns the open state and the selection, and updates both from the
/// handlers. Style methods size and place the control in its container.
#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    element: Stateful<Div>,
    options: Vec<SharedString>,
    selected: usize,
    open: bool,
    aria_label: Option<SharedString>,
    on_toggle: Rc<ToggleHandler>,
    on_select: Rc<SelectHandler>,
}

impl Dropdown {
    /// Create a closed dropdown over `options` that shows the option at index `selected`.
    pub fn new(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: usize,
    ) -> Self {
        let id = id.into();
        Self {
            element: div().id(id.clone()).relative().flex().flex_col(),
            id,
            options: options.into_iter().map(Into::into).collect(),
            selected,
            open: false,
            aria_label: None,
            on_toggle: Rc::new(|_, _, _| {}),
            on_select: Rc::new(|_, _, _| {}),
        }
    }

    /// Set whether the option menu is shown.
    pub const fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the accessible name of the trigger, such as what the chosen option applies to.
    pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Attach the handler that receives the requested open state whenever the user opens the
    /// menu or dismisses it, including after choosing an option.
    pub fn on_toggle(mut self, listener: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Rc::new(listener);
        self
    }

    /// Attach the handler that receives the index of an option the user chooses in place of the
    /// selected one.
    pub fn on_select(mut self, listener: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Rc::new(listener);
        self
    }

    /// Build the compact button that shows the selected option's label beside a caret.
    fn render_trigger(label: SharedString, aria_label: Option<SharedString>, open: bool) -> Button {
        Button::new("trigger")
            .compact()
            .justify_between()
            .gap_1()
            .aria_expanded(open)
            .aria_value(label.clone())
            .when_some(aria_label, StatefulInteractiveElement::aria_label)
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .child(Icon::disclosure(true).color(theme::TEXT_MUTED)),
            )
    }

    /// Build an invisible layer over the trigger that records the trigger's width each frame.
    /// The anchored menu is laid out apart from the trigger, so it cannot size itself against
    /// the trigger and reads this width instead.
    fn render_trigger_width_probe(state: Entity<DropdownState>) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                state.update(cx, |state, _| state.trigger_width = bounds.size.width);
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// Build the menu listing the options, marking the selected one and the keyboard highlight.
    fn render_menu(
        options: Vec<SharedString>,
        selected: usize,
        highlighted: usize,
        min_width: Pixels,
        choose: &Rc<impl Fn(usize, &mut Window, &mut App) + 'static>,
    ) -> Stateful<Div> {
        picker_surface()
            .id("menu")
            .role(Role::ListBox)
            .min_w(min_width)
            .py_1()
            .children(options.into_iter().enumerate().map(|(index, label)| {
                let choose = choose.clone();
                build_picker_row(("option", index), index == highlighted)
                    .role(Role::ListBoxOption)
                    .aria_selected(index == selected)
                    .h(px(OPTION_HEIGHT))
                    // Lines option labels up with the trigger's label.
                    .px_2()
                    .text_xs()
                    .whitespace_nowrap()
                    .when(index == selected, |row| row.text_color(theme::ACCENT))
                    .child(label)
                    .on_click(move |_, window, cx| choose(index, window, cx))
            }))
    }

    /// Build a layer covering the window beneath the menu, so an outside click only dismisses
    /// the menu and never reaches the trigger, which would reopen it.
    fn render_backdrop(
        window: &Window,
        dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Anchored {
        let viewport = window.viewport_size();
        anchored().position(Point::default()).child(
            div()
                .occlude()
                .w(viewport.width)
                .h(viewport.height)
                .on_any_mouse_down(dismiss),
        )
    }
}

impl Styled for Dropdown {
    /// Return the style of the container that holds the trigger and anchors the menu.
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl RenderOnce for Dropdown {
    /// Build the trigger and, while open, the option menu over a backdrop that dismisses it.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |_, _| DropdownState::default());
        let (highlighted, trigger_width) = {
            let state = state.read(cx);
            (
                state.highlight.unwrap_or(self.selected),
                state.trigger_width,
            )
        };
        let selected = self.selected;
        let open = self.open;
        let option_count = self.options.len();
        let selected_label = self.options.get(selected).cloned().unwrap_or_default();

        let request_open = {
            let state = state.clone();
            let on_toggle = self.on_toggle;
            Rc::new(move |open: bool, window: &mut Window, cx: &mut App| {
                set_highlight(&state, None, cx);
                on_toggle(&open, window, cx);
            })
        };
        let choose = {
            let request_open = request_open.clone();
            let on_select = self.on_select;
            Rc::new(move |index: usize, window: &mut Window, cx: &mut App| {
                if index != selected {
                    on_select(&index, window, cx);
                }
                request_open(false, window, cx);
            })
        };

        let trigger = Self::render_trigger(selected_label, self.aria_label, open).on_click({
            let request_open = request_open.clone();
            move |_, window, cx| request_open(!open, window, cx)
        });

        self.element
            .child(trigger)
            .child(Self::render_trigger_width_probe(state.clone()))
            .when(open, |element| {
                let backdrop = Self::render_backdrop(window, {
                    let request_open = request_open.clone();
                    move |_, window, cx| request_open(false, window, cx)
                });
                let menu = anchored()
                    .snap_to_window_with_margin(px(8.))
                    .child(Self::render_menu(
                        self.options,
                        selected,
                        highlighted,
                        trigger_width,
                        &choose,
                    ));
                element
                    // Capture keys before the trigger turns Enter into a click and before
                    // containers, such as dialogs that close on Escape, see them.
                    .capture_key_down(move |event, window, cx| {
                        let current = state.read(cx).highlight.unwrap_or(selected);
                        match event.keystroke.key.as_str() {
                            "escape" => request_open(false, window, cx),
                            "enter" => choose(current, window, cx),
                            _ => {
                                let Some(direction) = ListDirection::from_key(event) else {
                                    return;
                                };
                                let next = next_list_index(Some(current), option_count, direction);
                                set_highlight(&state, next, cx);
                            }
                        }
                        cx.stop_propagation();
                    })
                    // Deferred drawing paints the backdrop and menu above everything and outside
                    // any clipping ancestor; the slot places the menu just below the trigger.
                    .child(deferred(backdrop).with_priority(1))
                    .child(
                        div()
                            .absolute()
                            .top_full()
                            .left_0()
                            .mt_1()
                            .child(deferred(menu).with_priority(2)),
                    )
            })
    }
}
