use gpui::WindowOptions;

pub mod compositor;
pub mod contrast;

pub trait ToolFrontend {
    fn get_window_options(&self) -> WindowOptions;
}
