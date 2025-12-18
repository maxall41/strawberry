/// Macro to implement common tool view methods
/// Similar to backend's impl_tool_backend_basics!
#[macro_export]
macro_rules! impl_tool_view_basics {
    ($type:ty, $tool_name:expr) => {
        impl $type {
            fn get_current_tool_index(&self) -> Option<u16> {
                self.base.get_current_tool_index()
            }

            fn render_header(&self) -> impl gpui::IntoElement {
                use gpui::prelude::*;
                gpui::div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .mb_4()
                    .child($tool_name)
            }
        }
    };
}
