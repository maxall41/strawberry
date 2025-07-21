use gpui::WindowOptions;

pub mod compositor;

pub trait ToolFrontend {
    fn get_window_options(&self) -> WindowOptions;
}
