use std::sync::Mutex;
use strawberry_types::FrontendMessage;

#[derive(Default)]
pub struct ToolEvents(Mutex<Vec<FrontendMessage>>);
impl ToolEvents {
    pub fn emit(&self, event: FrontendMessage) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
    pub fn drain(&self) -> Vec<FrontendMessage> {
        std::mem::take(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

#[derive(Default)]
pub struct ToolBase {
    pub events: ToolEvents,
}
impl ToolBase {
    pub fn new(events: ToolEvents) -> Self {
        Self { events }
    }
}
#[macro_export]
macro_rules! impl_tool_backend_basics {
    ($tool_type:ty) => {
        fn persisted_state_mut(&mut self) -> Option<&mut dyn strawberry_types::PersistedState> {
            Some(&mut self.state)
        }
        fn is_enabled(&self) -> bool {
            self.state.is_enabled
        }
        fn set_enabled(&mut self, enabled: bool) {
            self.state.is_enabled = enabled;
        }
        fn drain_events(&mut self) -> Vec<strawberry_types::FrontendMessage> {
            self.base.events.drain()
        }
        $crate::impl_tool_cache!();
    };
}

/// Shared cache and reconciliation lifecycle, also used by interactive tools.
#[macro_export]
macro_rules! impl_tool_cache {
    () => {
        fn reconcile_state(&mut self) -> bool {
            self.cache.reconcile(&self.state)
        }

        fn acknowledge_processed_state(&mut self) {
            self.cache.sync.acknowledge(&self.state);
        }

        fn valid_cached_output(&self) -> Option<Vec<std::sync::Arc<image::RgbaImage>>> {
            self.cache.valid_output(&self.state)
        }

        fn set_cached_output(&mut self, output: Vec<std::sync::Arc<image::RgbaImage>>) {
            self.cache.cached_output = Some(output);
            self.cache.cached_state = Some(self.state.clone());
        }

        fn invalidate_cache(&mut self) {
            self.cache.invalidate();
        }
    };
}
