/// Generic cache for tool outputs
/// All fields are public for direct access - no helper methods needed!
pub struct ToolCache<TState> {
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<TState>,
    pub frontend_latest_state: Option<TState>,
}

impl<TState> ToolCache<TState> {
    pub fn new() -> Self {
        Self {
            cached_output: None,
            cached_state: None,
            frontend_latest_state: None,
        }
    }
}

impl<TState> Default for ToolCache<TState> {
    fn default() -> Self {
        Self::new()
    }
}
