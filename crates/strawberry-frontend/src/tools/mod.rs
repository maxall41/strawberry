use std::sync::{Arc, RwLock};

use gpui::{Context, Window};

pub mod base;

pub use base::ToolBase;
use strawberry_types::ToolState;

pub mod compositor;
pub mod contrast;
pub mod counter;
pub mod decomposer;
pub mod difference_of_gaussians;
pub mod gaussian_blur;
pub mod grayscale;
pub mod local_maxima;
pub mod morphology;
pub mod nervetracer;
pub mod object_size_filter;
pub mod rolling_ball;
pub mod sobel;
pub mod threshold;
pub mod watershed;

pub trait FrontendMessageHandler: Sized + 'static {
    type State: Clone + PartialEq + Send + Sync + 'static;

    /// Get the state Arc
    fn state_arc(&self) -> Arc<RwLock<Self::State>>;

    /// Get the base (immutable)
    fn base(&self) -> &ToolBase;

    /// Get the base (mutable)
    fn base_mut(&mut self) -> &mut ToolBase;

    /// Extract tool-specific state from `ToolState` enum
    fn extract_state(tool_state: ToolState) -> Option<Self::State>;

    /// Called when state changes (before cx.notify)
    /// Default: does nothing extra
    fn on_state_changed(&mut self, _new_state: &Self::State, _cx: &mut Context<Self>) {}
}

/// Watch backend tool state and refresh the view when settings change.
pub fn spawn_message_processor<V: FrontendMessageHandler>(
    view: &mut V,
    window: &Window,
    cx: &Context<V>,
) {
    if view.base().processor_spawned {
        return;
    }
    let Some(subscription) = view.base_mut().subscription.take() else {
        return;
    };
    view.base_mut().processor_spawned = true;
    let state = view.state_arc();
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Some(mut receiver)) = subscription.await else {
            return;
        };
        loop {
            let tool_state = receiver.borrow_and_update().clone();
            if let Some(new_state) = V::extract_state(tool_state)
                && this
                    .update(cx, |view, cx| {
                        let mut locked = state
                            .write()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if *locked != new_state {
                            *locked = new_state.clone();
                            drop(locked);
                            view.on_state_changed(&new_state, cx);
                            cx.notify();
                        }
                    })
                    .is_err()
            {
                break;
            }
            if receiver.changed().await.is_err() {
                break;
            }
        }
    })
    .detach();
}
pub mod automatic_segmentation;
