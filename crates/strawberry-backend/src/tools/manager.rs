use super::{
    Compositor, Contrast, Counter, Decomposer, DifferenceOfGaussians, GaussianBlur, Grayscale,
    LocalMaxima, Morphology, ObjectSizeFilter, RollingBall, Sobel, Threshold, Watershed,
    automatic_segmentation::AutomaticSegmentation, base::ToolEvents, nervetracer::NerveTracer,
};
use crate::utils::pipeline_io;
use indexmap::IndexMap;
use std::{path::Path, sync::Arc};
use strawberry_processing::GpuImageProcessor;
use strawberry_types::{
    BackendToolType, PendingProcessing, PipelineState, ToolBackend, ToolId, ToolState, ToolSummary,
};
use tokio::sync::watch;

pub struct ToolInstance {
    backend: Box<dyn ToolBackend>,
    tool_state_sender: watch::Sender<ToolState>,
}
impl ToolInstance {
    /// Subscribe to the latest state of this tool instance.
    #[must_use]
    pub fn subscribe_state(&self) -> watch::Receiver<ToolState> {
        self.tool_state_sender.subscribe()
    }
}
impl std::ops::Deref for ToolInstance {
    type Target = dyn ToolBackend;
    /// Borrow the wrapped tool backend.
    fn deref(&self) -> &Self::Target {
        self.backend.as_ref()
    }
}
impl std::ops::DerefMut for ToolInstance {
    /// Borrow the wrapped tool backend mutably.
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.backend.as_mut()
    }
}

pub struct ToolManager {
    pub tools: IndexMap<ToolId, ToolInstance>,
    pub selected_tool: Option<ToolId>,
    /// Stack shown in the viewer and edited by interactive tools.
    pub selected_stack: usize,
    /// Latest ordered tools, enablement, and selection for pipeline subscribers.
    pipeline_state_sender: watch::Sender<PipelineState>,
    gpu: Arc<GpuImageProcessor>,
    next_tool_id: u64,
    processing_order: Vec<ToolId>,
}
impl ToolManager {
    /// Create an empty tool manager sharing the supplied GPU processor.
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        Self {
            tools: IndexMap::new(),
            selected_tool: None,
            selected_stack: 0,
            pipeline_state_sender: watch::channel(PipelineState::default()).0,
            gpu,
            next_tool_id: 1,
            processing_order: vec![],
        }
    }
    /// Subscribe to ordered tool summaries and the selected instance.
    #[must_use]
    pub fn subscribe_pipeline_state(&self) -> watch::Receiver<PipelineState> {
        self.pipeline_state_sender.subscribe()
    }
    /// Allocate a unique tool identity or report exhaustion.
    fn allocate_id(&mut self) -> anyhow::Result<ToolId> {
        let id = ToolId(self.next_tool_id);
        self.next_tool_id = self
            .next_tool_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Tool IDs exhausted"))?;
        Ok(id)
    }
    /// Detached construction has no externally visible side effects.
    async fn create(&self, kind: &BackendToolType) -> anyhow::Result<ToolInstance> {
        let events = ToolEvents::default();
        let gpu = self.gpu.clone();
        let mut backend: Box<dyn ToolBackend> = match kind {
            BackendToolType::AutomaticSegmentation => {
                Box::new(AutomaticSegmentation::new(events, gpu).await?)
            }
            BackendToolType::Compositor => Box::new(Compositor::new(events, gpu).await?),
            BackendToolType::Contrast => Box::new(Contrast::new(events, gpu).await?),
            BackendToolType::Counter => Box::new(Counter::new(events)),
            BackendToolType::Decomposer => Box::new(Decomposer::new(events, gpu).await?),
            BackendToolType::DifferenceOfGaussians => {
                Box::new(DifferenceOfGaussians::new(events, gpu).await?)
            }
            BackendToolType::GaussianBlur => Box::new(GaussianBlur::new(events, gpu).await?),
            BackendToolType::Grayscale => Box::new(Grayscale::new(events, gpu).await?),
            BackendToolType::LocalMaxima => Box::new(LocalMaxima::new(events, gpu).await?),
            BackendToolType::Morphology => Box::new(Morphology::new(events, gpu).await?),
            BackendToolType::NerveTracer => Box::new(NerveTracer::new(events)),
            BackendToolType::ObjectSizeFilter => Box::new(ObjectSizeFilter::new(events)),
            BackendToolType::RollingBall => Box::new(RollingBall::new(events, gpu).await?),
            BackendToolType::Sobel => Box::new(Sobel::new(events, gpu).await?),
            BackendToolType::Threshold => Box::new(Threshold::new(events, gpu).await?),
            BackendToolType::Watershed => Box::new(Watershed::new(events, gpu).await?),
        };
        backend.select_stack(self.selected_stack);
        let tool_state_sender = watch::channel(backend.get_state()).0;
        Ok(ToolInstance {
            backend,
            tool_state_sender,
        })
    }
    /// Create, enable, and select a new tool instance.
    ///
    /// # Errors
    /// Returns an error if GPU initialization fails or the tool ID space is exhausted.
    pub async fn add_tool(&mut self, kind: &BackendToolType) -> anyhow::Result<ToolId> {
        let mut tool = self.create(kind).await?;
        tool.set_enabled(true);
        tool.tool_state_sender.send_replace(tool.get_state());
        let id = self.allocate_id()?;
        self.tools.insert(id, tool);
        self.select(Some(id));
        Ok(id)
    }
    /// Select an existing tool and cancel the previous selection's interaction.
    pub fn select(&mut self, id: Option<ToolId>) {
        if id.is_some_and(|id| !self.tools.contains_key(&id)) {
            return;
        }
        if id != self.selected_tool {
            if let Some(tool) = self.selected_tool.and_then(|id| self.tools.get_mut(&id)) {
                tool.cancel_interaction();
            }
            self.selected_tool = id;
        }
    }
    /// Select the stack every tool edits, cancelling gestures begun on another stack.
    pub fn select_stack(&mut self, stack: usize) {
        if stack == self.selected_stack {
            return;
        }
        self.selected_stack = stack;
        for tool in self.tools.values_mut() {
            tool.cancel_interaction();
            tool.select_stack(stack);
        }
    }
    /// Remove a tool instance and clear its selection when needed.
    pub fn remove_tool(&mut self, id: ToolId) {
        if self.selected_tool == Some(id) {
            self.select(None);
        }
        self.tools.shift_remove(&id);
    }
    /// Move a tool before another instance, or to the end of the stack.
    pub fn move_tool(&mut self, id: ToolId, before: Option<ToolId>) {
        let Some(from) = self.tools.get_index_of(&id) else {
            return;
        };
        if before == Some(id) {
            return;
        }
        let to = match before {
            Some(before) => {
                let Some(index) = self.tools.get_index_of(&before) else {
                    return;
                };
                if from < index {
                    index.saturating_sub(1)
                } else {
                    index
                }
            }
            None => self.tools.len().saturating_sub(1),
        };
        self.tools.move_index(from, to);
    }
    /// Request processing from the first changed enabled tool position.
    pub fn reconcile(&mut self, pending: &mut PendingProcessing) {
        let current: Vec<_> = self
            .tools
            .iter()
            .enumerate()
            .filter(|(_, (_, tool))| tool.is_enabled() && tool.supports_processing())
            .map(|(index, (id, _))| (index, *id))
            .collect();
        let common = self
            .processing_order
            .iter()
            .zip(&current)
            .take_while(|(old, (_, id))| *old == id)
            .count();
        if common < self.processing_order.len() || common < current.len() {
            pending.request_processing(
                current
                    .get(common)
                    .map_or(self.tools.len(), |(index, _)| *index),
            );
        }
        self.processing_order = current.into_iter().map(|(_, id)| id).collect();
    }
    /// Publish changed tool snapshots and pipeline summaries to subscribers.
    pub fn publish(&mut self) -> bool {
        let mut changed = false;
        for tool in self.tools.values_mut() {
            let state = tool.get_state();
            changed |= tool.tool_state_sender.send_if_modified(|previous| {
                if *previous == state {
                    false
                } else {
                    *previous = state;
                    true
                }
            });
        }
        let state = PipelineState {
            selected_tool: self.selected_tool,
            tools: self
                .tools
                .iter()
                .map(|(id, tool)| ToolSummary {
                    id: *id,
                    kind: tool.iam(),
                    is_enabled: tool.is_enabled(),
                })
                .collect(),
        };
        changed |= self.pipeline_state_sender.send_if_modified(|previous| {
            if *previous == state {
                false
            } else {
                *previous = state;
                true
            }
        });
        changed
    }
    /// Save tool settings and assets to a pipeline archive.
    ///
    /// # Errors
    /// Returns an error if tool assets cannot be serialized or the archive cannot be written.
    pub fn save_pipeline(&self, path: &Path) -> anyhow::Result<()> {
        let entries = self
            .tools
            .values()
            .map(|tool| Ok((tool.get_state(), tool.pipeline_assets()?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        pipeline_io::save(path, &entries)
    }
    /// Replace the pipeline with validated tool instances from an archive.
    ///
    /// # Errors
    /// Returns an error if the archive, settings, or assets cannot be loaded or
    /// validated, or if the tool ID space is exhausted.
    pub async fn load_pipeline(&mut self, path: &Path, cli: bool) -> anyhow::Result<()> {
        let mut candidate = IndexMap::new();
        for entry in pipeline_io::load(path)? {
            let kind = entry.kind()?;
            let mut tool = self.create(&kind).await?;
            tool.load_persisted_state(entry.state, &entry.assets, cli)?;
            // Loading replaces runtime-only state, including the stack selection.
            tool.select_stack(self.selected_stack);
            tool.get_state().validate()?;
            tool.cancel_interaction();
            tool.tool_state_sender.send_replace(tool.get_state());
            let id = self.allocate_id()?;
            candidate.insert(id, tool);
        }
        self.select(None);
        self.tools = candidate;
        Ok(())
    }
}
