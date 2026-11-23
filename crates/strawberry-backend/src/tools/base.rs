use std::{
    mem,
    sync::{Mutex, PoisonError},
};
use strawberry_types::FrontendMessage;

#[derive(Default)]
pub struct ToolEvents(Mutex<Vec<FrontendMessage>>);
impl ToolEvents {
    /// Append a frontend event to the shared queue.
    pub fn emit(&self, event: FrontendMessage) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
    /// Return and clear all queued frontend events.
    pub fn drain(&self) -> Vec<FrontendMessage> {
        mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[derive(Default)]
pub struct ToolBase {
    pub events: ToolEvents,
}
impl ToolBase {
    /// Create shared tool state using the supplied event queue.
    pub const fn new(events: ToolEvents) -> Self {
        Self { events }
    }
}
#[macro_export]
macro_rules! impl_tool_backend_basics {
    ($tool_type:ty $(, $replace_input:ident)?) => {
        /// Expose mutable settings for pipeline persistence.
        fn persisted_state_mut(&mut self) -> Option<&mut dyn strawberry_types::PersistedState> {
            Some(&mut self.state)
        }
        /// Return whether this tool is enabled.
        fn is_enabled(&self) -> bool {
            self.state.is_enabled
        }
        /// Update whether this tool participates in the pipeline.
        fn set_enabled(&mut self, enabled: bool) {
            self.state.is_enabled = enabled;
        }
        /// Return and clear queued frontend events.
        fn drain_events(&mut self) -> Vec<strawberry_types::FrontendMessage> {
            self.base.events.drain()
        }
        $crate::impl_tool_cache!($($replace_input)?);
    };
}

/// Share cache management, optionally replacing derived state when a bypassed input changes.
#[macro_export]
macro_rules! impl_tool_cache {
    ($($replace_input:ident)?) => {
        /// Reconcile settings and report whether processing is needed.
        fn reconcile_state(&mut self) -> bool {
            self.cache.reconcile(&self.state)
        }

        /// Record the settings acknowledged by processing.
        fn acknowledge_processed_state(&mut self) {
            self.cache.reconcile(&self.state);
        }

        /// Return cached output when its input and processing settings still match.
        fn valid_cached_output(
            &self,
            input: &strawberry_types::ImageCollection,
        ) -> Option<strawberry_types::ImageCollection> {
            self.cache.valid_output(&self.state, input)
        }

        /// Cache processed images together with their input and settings.
        fn set_cached_output(
            &mut self,
            input: &strawberry_types::ImageCollection,
            output: strawberry_types::ImageCollection,
        ) {
            self.cache.store_output(&self.state, input, output);
        }

        /// Discard retained data when the bypassed input changes.
        fn reconcile_bypassed_input(&mut self, input: &strawberry_types::ImageCollection) {
            if self.cache.input_differs(input) {
                self.invalidate_cache();
                $(self.$replace_input(input.get_shape());)?
            }
        }

        /// Discard cached output and the remembered input.
        fn invalidate_cache(&mut self) {
            self.cache.invalidate_all();
        }
    };
}
