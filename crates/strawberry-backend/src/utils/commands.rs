use strawberry_types::PipelineCommand;
use std::collections::HashSet;

/// Collects and deduplicates commands from a *flume* receiver, keeping only the last occurrence of each command type
pub fn collect_and_deduplicate_commands(
    receiver: &flume::Receiver<PipelineCommand>,
) -> Vec<PipelineCommand> {
    let commands: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();

    

    if commands.len() > 1 {
        let mut seen_discriminants = HashSet::new();
        let mut deduplicated = Vec::new();

        for command in commands.into_iter().rev() {
            let discriminant = std::mem::discriminant(&command);
            if seen_discriminants.insert(discriminant) {
                deduplicated.push(command);
            }
        }

        deduplicated.reverse();
        deduplicated
    } else {
        commands
    }
}
