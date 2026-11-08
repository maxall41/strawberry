use std::sync::{Arc, RwLock};

use gpui::{Context, Window};

pub mod base;

pub use base::ToolBase;
use strawberry_types::{FrontendMessage, PipelineCommand, ToolState};

pub mod compositor;
pub mod contrast;
pub mod counter;
pub mod decomposer;
pub mod gaussian_blur;
pub mod grayscale;
pub mod local_maxima;
pub mod morphology;
pub mod nervetracer;
pub mod rolling_ball;
pub mod sobel;
pub mod threshold;
pub mod watershed;

/// Disconnects an initialized tool view when it is dropped.
pub struct CleanupGuard {
    command_sender: tokio::sync::broadcast::Sender<strawberry_types::PipelineCommand>,
    backend_tool_type: strawberry_types::BackendToolType,
    connected: bool,
}

impl CleanupGuard {
    pub const fn new(
        command_sender: tokio::sync::broadcast::Sender<strawberry_types::PipelineCommand>,
        backend_tool_type: strawberry_types::BackendToolType,
    ) -> Self {
        Self {
            command_sender,
            backend_tool_type,
            connected: false,
        }
    }

    const fn mark_connected(&mut self) {
        self.connected = true;
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        // Views that never connected must not disconnect another view's
        // frontend subscription.
        if !self.connected {
            return;
        }

        println!(
            "CleanupGuard dropping for {:?} - tool view released",
            self.backend_tool_type
        );
        if let Err(e) =
            self.command_sender
                .send(strawberry_types::PipelineCommand::DisconnectFrontend(
                    self.backend_tool_type.clone(),
                ))
        {
            eprintln!(
                "Failed to send disconnect command for {:?}: {}",
                self.backend_tool_type, e
            );
        }
    }
}

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

pub fn spawn_message_processor<V: FrontendMessageHandler>(
    view: &mut V,
    window: &Window,
    cx: &Context<V>,
) {
    // The receive channel is flume (MPMC): a second consumer task would steal
    // messages and race state application out of order, so the view could end
    // up displaying stale backend state forever. Spawn exactly once per view.
    if view.base().processor_spawned {
        return;
    }
    view.base_mut().processor_spawned = true;

    let state = view.state_arc();
    let Some(receiver) = view.base().receive_channel.clone() else {
        view.base_mut().processor_spawned = false;
        return;
    };

    cx.spawn_in(window, async move |this, cx| {
        loop {
            let Ok(command) = receiver.recv_async().await else {
                break;
            };
            let PipelineCommand::FrontendMessage(message) = command else {
                continue;
            };

            match message {
                FrontendMessage::ReturnState(tool_state) => {
                    let Some(new_state) = V::extract_state(tool_state) else {
                        continue;
                    };
                    let Ok(mut locked) = state.write() else {
                        eprintln!("Failed to acquire write lock on state");
                        continue;
                    };
                    if *locked != new_state {
                        *locked = new_state.clone();
                        drop(locked);
                        let _ = this.update(cx, |view, cx| {
                            view.on_state_changed(&new_state, cx);
                            cx.notify();
                        });
                    }
                }
                FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                    let _ = this.update(cx, |view, cx| {
                        let base = view.base_mut();
                        if let Ok(mut idx) = base.current_tool_index.write() {
                            *idx = backend_ids.first().copied();
                        }
                        base.backend_tool_indicies = backend_ids;
                        base.send_channels = senders;

                        if let Some(sender) = base.send_channels.first() {
                            let _ = sender.send(PipelineCommand::RequestState());
                        }
                        cx.notify();
                    });
                }
                _ => {}
            }
        }
    })
    .detach();
}
