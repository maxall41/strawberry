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
