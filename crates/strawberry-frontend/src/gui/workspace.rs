use gpui::{Pixels, Point, Size, point, px, size};

pub const TOOLBAR_HEIGHT: f32 = 44.0;
pub const STATUS_BAR_ROW_HEIGHT: f32 = 36.0;
pub const STATUS_BAR_PADDING: f32 = 4.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 300.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 560.0;
pub const SIDEBAR_GRIP_WIDTH: f32 = 5.0;

/// Return the height of a status bar with `rows` rows; it is hidden when there are none.
pub fn status_bar_height(rows: usize) -> f32 {
    if rows == 0 {
        return 0.0;
    }
    let rows = u16::try_from(rows).map_or(f32::MAX, f32::from);
    rows.mul_add(STATUS_BAR_ROW_HEIGHT, 2.0 * STATUS_BAR_PADDING)
}

pub struct WorkspaceLayout {
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub resizing_sidebar: bool,
    /// Rows of the status bar below the canvas, one per dimension holding several images.
    pub status_bar_rows: usize,
}

impl Default for WorkspaceLayout {
    /// Create the default configuration for this component.
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            sidebar_width: 360.0,
            resizing_sidebar: false,
            status_bar_rows: 0,
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
        size(
            px((f32::from(viewport.width) - sidebar).max(1.0)),
            px((f32::from(viewport.height)
                - TOOLBAR_HEIGHT
                - status_bar_height(self.status_bar_rows))
            .max(1.0)),
        )
    }

    /// Return the canvas origin within the workspace.
    pub fn canvas_position(position: Point<Pixels>) -> Point<Pixels> {
        point(position.x, px(f32::from(position.y) - TOOLBAR_HEIGHT))
    }
}
