use std::collections::HashSet;
use std::mem::{discriminant, Discriminant};

use strawberry_types::{
    CompositorMessage, ContrastMessage, CountMessage, DecomposerMessage, GaussianBlurMessage,
    LocalMaximaMessage, ManagerMessage, MorphologyMessage, NerveTracerMessage, PipelineCommand,
    PipelineObjectMessage, RollingBallMessage, SobelMessage, ThresholdMessage, WatershedMessage,
};

/// Debounce key for a command. Keying on the `PipelineCommand` discriminant
/// alone would collapse every `PipelineObject` message into one bucket, so
/// unrelated messages sent in the same batch would swallow each other (e.g. a
/// `DeleteCountType` immediately followed by `SetActiveCountType` dropped the
/// delete). Nested message enums are therefore keyed by their innermost
/// variant.
#[derive(PartialEq, Eq, Hash)]
enum DedupKey {
    Command(Discriminant<PipelineCommand>),
    Object(Discriminant<PipelineObjectMessage>),
    Compositor(Discriminant<CompositorMessage>),
    Contrast(Discriminant<ContrastMessage>),
    Count(Discriminant<CountMessage>),
    Decomposer(Discriminant<DecomposerMessage>),
    GaussianBlur(Discriminant<GaussianBlurMessage>),
    LocalMaxima(Discriminant<LocalMaximaMessage>),
    Manager(Discriminant<ManagerMessage>),
    Morphology(Discriminant<MorphologyMessage>),
    NerveTracer(Discriminant<NerveTracerMessage>),
    RollingBall(Discriminant<RollingBallMessage>),
    Sobel(Discriminant<SobelMessage>),
    Threshold(Discriminant<ThresholdMessage>),
    Watershed(Discriminant<WatershedMessage>),
}

/// `None` means the command must never be deduplicated. Debouncing is only
/// valid for messages that set an absolute value (last write wins); imperative
/// messages like "delete the current count" or "navigate once" are relative,
/// so each occurrence in a batch (e.g. from key-repeat) must be applied.
fn dedup_key(command: &PipelineCommand) -> Option<DedupKey> {
    let PipelineCommand::PipelineObject(message) = command else {
        return Some(DedupKey::Command(discriminant(command)));
    };
    Some(match message {
        PipelineObjectMessage::CompositorMessage(m) => DedupKey::Compositor(discriminant(m)),
        PipelineObjectMessage::ContrastMessage(m) => DedupKey::Contrast(discriminant(m)),
        PipelineObjectMessage::CountMessage(
            CountMessage::DeleteCurrentCount
            | CountMessage::ReviewNavigateNext
            | CountMessage::ReviewNavigatePrevious,
        ) => return None,
        PipelineObjectMessage::CountMessage(m) => DedupKey::Count(discriminant(m)),
        PipelineObjectMessage::DecomposerMessage(m) => DedupKey::Decomposer(discriminant(m)),
        PipelineObjectMessage::GaussianBlurMessage(m) => DedupKey::GaussianBlur(discriminant(m)),
        PipelineObjectMessage::LocalMaximaMessage(m) => DedupKey::LocalMaxima(discriminant(m)),
        PipelineObjectMessage::ManagerMessage(m) => DedupKey::Manager(discriminant(m)),
        PipelineObjectMessage::MorphologyMessage(m) => DedupKey::Morphology(discriminant(m)),
        PipelineObjectMessage::NerveTracerMessage(m) => DedupKey::NerveTracer(discriminant(m)),
        PipelineObjectMessage::RollingBallMessage(m) => DedupKey::RollingBall(discriminant(m)),
        PipelineObjectMessage::SobelMessage(m) => DedupKey::Sobel(discriminant(m)),
        PipelineObjectMessage::ThresholdMessage(m) => DedupKey::Threshold(discriminant(m)),
        PipelineObjectMessage::WatershedMessage(m) => DedupKey::Watershed(discriminant(m)),
        PipelineObjectMessage::ToggleActive(_) => DedupKey::Object(discriminant(message)),
    })
}

/// Collects and deduplicates commands from a *flume* receiver, keeping only
/// the last occurrence of each message variant (see [`DedupKey`]).
pub fn collect_and_deduplicate_commands(
    receiver: &flume::Receiver<PipelineCommand>,
) -> Vec<PipelineCommand> {
    let commands: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();

    if commands.len() > 1 {
        let mut seen_keys = HashSet::new();
        let mut deduplicated = Vec::new();

        for command in commands.into_iter().rev() {
            let keep = match dedup_key(&command) {
                Some(key) => seen_keys.insert(key),
                None => true,
            };
            if keep {
                deduplicated.push(command);
            }
        }

        deduplicated.reverse();
        deduplicated
    } else {
        commands
    }
}
