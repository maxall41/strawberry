use gpui::{Pixels, Point, Size, point, px, size};

pub const TOOLBAR_HEIGHT: f32 = 44.0;
pub const STATUS_BAR_HEIGHT: f32 = 44.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 300.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 560.0;
pub const SIDEBAR_GRIP_WIDTH: f32 = 5.0;

pub struct WorkspaceLayout {
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub resizing_sidebar: bool,
    /// Shown below the canvas while several stacks are loaded.
    pub status_bar_visible: bool,
}

impl Default for WorkspaceLayout {
    /// Create the default configuration for this component.
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            sidebar_width: 360.0,
            resizing_sidebar: false,
            status_bar_visible: false,
        }
    }
}

impl WorkspaceLayout {
    /// Return the available image canvas dimensions.
    pub fn canvas_size(&self, viewport: Size<Pixels>) -> Size<Pixels> {
        let sidebar = if self.sidebar_visible {
            self.sidebar_width + SIDEBAR_GRIP_WIDTH
        } else {
            0.0
        };
        let status_bar = if self.status_bar_visible {
            STATUS_BAR_HEIGHT
        } else {
            0.0
        };
        size(
            px((f32::from(viewport.width) - sidebar).max(1.0)),
            px((f32::from(viewport.height) - TOOLBAR_HEIGHT - status_bar).max(1.0)),
        )
    }

    /// Return the canvas origin within the workspace.
    pub fn canvas_position(position: Point<Pixels>) -> Point<Pixels> {
        point(position.x, px(f32::from(position.y) - TOOLBAR_HEIGHT))
    }
}
