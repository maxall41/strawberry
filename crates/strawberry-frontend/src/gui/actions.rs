use gpui::{Action, KeyBinding};

// Don't throw derive_partial_eq_without_eq on actions macro.
macro_rules! actions {
    ($( $namespace:path, )? [$( $(#[$attr:meta])* $name:ident),* $(,)?]) => {
        gpui::actions!($( $namespace, )? [
            $(
                #[allow(
                    clippy::derive_partial_eq_without_eq,
                    reason = "Equality derives on action structs are supplied by GPUI"
                )]
                $(#[$attr])*
                $name
            ),*
        ]);
    };
}

pub(crate) use actions;

/// Toggle the channel at `index` in the focused tool's channel list.
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(no_json)]
pub struct ToggleChannel {
    pub index: usize,
}

/// Create ⌘1 to ⌘9 bindings toggling the first `channel_count` channels of the views with
/// key context `context`.
pub fn create_channel_shortcuts(channel_count: usize, context: &str) -> Vec<KeyBinding> {
    (0..channel_count.min(9))
        .map(|index| {
            KeyBinding::new(
                &format!("cmd-{}", index.saturating_add(1)),
                ToggleChannel { index },
                Some(context),
            )
        })
        .collect()
}
