/// Macro to implement common tool view methods
/// Similar to backend's `impl_tool_backend_basics`!
#[macro_export]
macro_rules! impl_tool_view_basics {
    ($type:ty, $tool_name:expr_2021) => {
        impl $type {
            fn render_header(&self) -> impl gpui::IntoElement + use<> {
                use gpui::prelude::*;
                gpui::div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .pb_2()
                    .mb_1()
                    .border_b_1()
                    .border_color($crate::gui::theme::BORDER_MUTED)
                    .child($tool_name)
            }
        }
    };
}
