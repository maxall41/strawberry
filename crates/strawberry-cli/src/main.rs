use anyhow::{anyhow, Context, Result};
use clap::Parser;
use colored::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::sync::{broadcast, watch};
use tokio::time::Duration;

use strawberry_backend::Pipeline;
use strawberry_types::{FeedbackLevel, FrontendMessage, OutputFrame, PipelineCommand};

/// Budget for one group, measured from the moment its load is dispatched.
const GROUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Discard everything currently queued on a broadcast receiver.
fn drain(receiver: &mut broadcast::Receiver<PipelineCommand>) {
    loop {
        match receiver.try_recv() {
            Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
}

/// Strawberry - Advanced Image Processing Pipeline
#[derive(Parser)]
#[command(name = "strawberry")]
#[command(about = "A powerful image processing pipeline for scientific and research applications")]
#[command(version, long_about = None)]
pub struct Args {
    /// Pipeline configuration file to load (.toml format)
    #[arg(short, long, value_name = "FILE")]
    pub pipeline: Option<PathBuf>,

    /// Input image file or directory containing images
    #[arg(short, long, value_name = "PATH")]
    pub input: Option<PathBuf>,

    /// Output file or directory for processed images
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Enable CLI mode (non-interactive processing)
    #[arg(long)]
    pub cli: bool,

    /// Regex pattern to filter image files (e.g., ".*\\.tiff?$" for TIFF files)
    #[arg(
        long,
        value_name = "PATTERN",
        default_value = ".*\\.(tif|tiff|png|jpg|jpeg)$"
    )]
    pub filter: String,

    /// Process images recursively in subdirectories
    #[arg(short, long)]
    pub recursive: bool,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Dry run - show what would be processed without actually processing
    #[arg(long)]
    pub dry_run: bool,

    /// Output format for processed images
    #[arg(long, value_name = "FORMAT", default_value = "preserve")]
    pub format: OutputFormat,

    /// Prefix to add to output filenames
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,

    /// Suffix to add to output filenames (before extension)
    #[arg(long, value_name = "SUFFIX")]
    pub suffix: Option<String>,

    /// Grouping mode for processing multiple images together
    #[arg(long, value_name = "MODE", default_value = "none")]
    pub group: GroupingMode,

    /// Regex pattern to extract group identifier from filename (for regex grouping)
    #[arg(long, value_name = "PATTERN")]
    pub group_regex: Option<String>,
}

#[derive(Clone, Debug)]
pub enum GroupingMode {
    None,
    Subdirectory,
    Regex,
}

impl std::str::FromStr for GroupingMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(GroupingMode::None),
            "subdirectory" | "subdir" => Ok(GroupingMode::Subdirectory),
            "regex" => Ok(GroupingMode::Regex),
            _ => Err(format!("Invalid grouping mode: {s}")),
        }
    }
}

#[derive(Clone, Debug)]
pub enum OutputFormat {
    Preserve,
    Png,
    Tiff,
    Jpeg,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "preserve" => Ok(OutputFormat::Preserve),
            "png" => Ok(OutputFormat::Png),
            "tiff" | "tif" => Ok(OutputFormat::Tiff),
            "jpeg" | "jpg" => Ok(OutputFormat::Jpeg),
            _ => Err(format!("Invalid output format: {s}")),
        }
    }
}

impl OutputFormat {
    pub fn extension(&self) -> &str {
        match self {
            OutputFormat::Preserve => "",
            OutputFormat::Png => "png",
            OutputFormat::Tiff => "tiff",
            OutputFormat::Jpeg => "jpg",
        }
    }
}

pub struct CliProcessor {
    args: Args,
    regex: regex::Regex,
    group_regex: Option<regex::Regex>,
}

impl CliProcessor {
    pub fn new(args: Args) -> Result<Self> {
        let regex = regex::Regex::new(&args.filter)
            .with_context(|| format!("Invalid regex pattern: {}", args.filter))?;

        let group_regex = if let Some(pattern) = &args.group_regex {
            Some(
                regex::Regex::new(pattern)
                    .with_context(|| format!("Invalid group regex pattern: {pattern}"))?,
            )
        } else {
            None
        };

        // Validate grouping configuration
        if matches!(args.group, GroupingMode::Regex) && args.group_regex.is_none() {
            return Err(anyhow!(
                "Group regex pattern is required when using regex grouping mode"
            ));
        }

        if matches!(args.group, GroupingMode::Subdirectory) && !args.recursive {
            println!("{}", "Warning: Subdirectory grouping enabled but recursive is disabled. Consider enabling --recursive for better results.".yellow());
        }

        Ok(Self {
            args,
            regex,
            group_regex,
        })
    }

    pub async fn run(&self) -> Result<()> {
        self.validate_args()?;

        let input_path = self
            .args
            .input
            .as_ref()
            .ok_or_else(|| anyhow!("Input path is required for CLI mode"))?;

        let output_path = self
            .args
            .output
            .as_ref()
            .ok_or_else(|| anyhow!("Output path is required for CLI mode"))?;

        let pipeline_file = self
            .args
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("Pipeline file is required for CLI mode"))?;

        if !pipeline_file.exists() {
            return Err(anyhow!(
                "Pipeline file does not exist: {}",
                pipeline_file.display()
            ));
        }

        let image_files = self.discover_images(input_path)?;

        if image_files.is_empty() {
            println!(
                "{}",
                "No images found matching the filter criteria.".yellow()
            );
            return Ok(());
        }

        let groups = self.group_images(image_files)?;

        if self.args.verbose {
            println!(
                "{}",
                format!("Found {} group(s) to process:", groups.len()).green()
            );
            for (group_name, files) in &groups {
                println!("  {}: {} image(s)", group_name.cyan(), files.len());
                if self.args.verbose {
                    for file in files {
                        println!("    {}", file.display().to_string().dimmed());
                    }
                }
            }
        }

        if self.args.dry_run {
            let total_images: usize = groups.values().map(|v| v.len()).sum();
            println!(
                "{}",
                format!(
                    "Dry run completed. {} group(s) with {} total image(s) would be processed.",
                    groups.len(),
                    total_images
                )
                .green()
            );
            return Ok(());
        }

        self.process_groups(groups, output_path, pipeline_file)
            .await?;

        Ok(())
    }

    fn validate_args(&self) -> Result<()> {
        if let Some(input) = &self.args.input {
            if !input.exists() {
                return Err(anyhow!("Input path does not exist: {}", input.display()));
            }
        }

        if let Some(output) = &self.args.output {
            // If input is a directory, output should be treated as a directory too
            let output_should_be_dir = self
                .args
                .input
                .as_ref()
                .map(|i| i.is_dir())
                .unwrap_or(false);

            if output_should_be_dir
                || output.to_string_lossy().ends_with('/')
                || output.to_string_lossy().ends_with('\\')
            {
                // Treat as directory
                if !output.exists() {
                    if self.args.verbose {
                        println!("Creating output directory: {}", output.display());
                    }
                    fs::create_dir_all(output).with_context(|| {
                        format!("Failed to create output directory: {}", output.display())
                    })?;
                }
            } else {
                // Treat as file, create parent directory if needed
                if let Some(parent) = output.parent() {
                    if !parent.exists() {
                        if self.args.verbose {
                            println!("Creating output directory: {}", parent.display());
                        }
                        fs::create_dir_all(parent).with_context(|| {
                            format!("Failed to create output directory: {}", parent.display())
                        })?;
                    }
                }
            }
        }

        Ok(())
    }

    fn discover_images(&self, input_path: &Path) -> Result<Vec<PathBuf>> {
        let mut image_files = Vec::new();

        if input_path.is_file() {
            if self.is_image_file(input_path) {
                image_files.push(input_path.to_path_buf());
            } else {
                return Err(anyhow!(
                    "Input file does not match filter criteria: {}",
                    input_path.display()
                ));
            }
        } else if input_path.is_dir() {
            self.scan_directory(input_path, &mut image_files)?;
        } else {
            return Err(anyhow!(
                "Input path is neither a file nor a directory: {}",
                input_path.display()
            ));
        }

        image_files.sort();
        Ok(image_files)
    }

    fn scan_directory(&self, dir_path: &Path, image_files: &mut Vec<PathBuf>) -> Result<()> {
        let entries = fs::read_dir(dir_path)
            .with_context(|| format!("Failed to read directory: {}", dir_path.display()))?;

        for entry in entries {
            let entry = entry.with_context(|| {
                format!("Failed to read directory entry in: {}", dir_path.display())
            })?;
            let path = entry.path();

            if path.is_file() {
                if self.is_image_file(&path) {
                    image_files.push(path);
                }
            } else if path.is_dir() && self.args.recursive {
                self.scan_directory(&path, image_files)?;
            }
        }

        Ok(())
    }

    fn is_image_file(&self, path: &Path) -> bool {
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            self.regex.is_match(filename)
        } else {
            false
        }
    }

    fn group_images(&self, image_files: Vec<PathBuf>) -> Result<HashMap<String, Vec<PathBuf>>> {
        let mut groups = HashMap::new();

        match &self.args.group {
            GroupingMode::None => {
                // Each image is its own group
                for (index, file) in image_files.into_iter().enumerate() {
                    let group_name = format!("image_{:04}", index + 1);
                    groups.insert(group_name, vec![file]);
                }
            }
            GroupingMode::Subdirectory => {
                // Group by parent directory
                for file in image_files {
                    let group_name = if let Some(parent) = file.parent() {
                        parent
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("root")
                            .to_string()
                    } else {
                        "root".to_string()
                    };
                    groups.entry(group_name).or_insert_with(Vec::new).push(file);
                }
            }
            GroupingMode::Regex => {
                // Group by regex pattern match
                let group_regex = self
                    .group_regex
                    .as_ref()
                    .expect("group_regex is guaranteed to be Some when in Regex grouping mode");
                for file in image_files {
                    let filename = file
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or_else(|| anyhow!("Invalid filename: {}", file.display()))?;

                    let group_name = if let Some(captures) = group_regex.captures(filename) {
                        if let Some(matched) = captures.get(1) {
                            matched.as_str().to_string()
                        } else if let Some(matched) = captures.get(0) {
                            matched.as_str().to_string()
                        } else {
                            "ungrouped".to_string()
                        }
                    } else {
                        "ungrouped".to_string()
                    };

                    groups.entry(group_name).or_insert_with(Vec::new).push(file);
                }
            }
        }

        Ok(groups)
    }

    async fn process_groups(
        &self,
        groups: HashMap<String, Vec<PathBuf>>,
        output_path: &Path,
        pipeline_file: &Path,
    ) -> Result<()> {
        // view_recv must stay alive: the backend treats a receiverless broadcast
        // channel as a send failure. It is also how backend errors reach us.
        let (mut pipeline, view_sender, mut view_recv, mut frame_rx) = Pipeline::new().await?;

        // Start pipeline processing task
        let pipeline_handle = tokio::spawn(async move {
            loop {
                if let Err(e) = pipeline.process_commands().await {
                    // In CLI mode, pipeline errors are handled internally with user feedback
                    // We still log to stderr for debugging purposes
                    eprintln!(
                        "{}",
                        format!("Error processing pipeline commands: {e}").red()
                    );
                }
                tokio::time::sleep(Duration::from_millis(33)).await;
            }
        });

        // Load pipeline configuration
        if let Err(e) = view_sender.send(PipelineCommand::CLILoadPipeline(
            pipeline_file.to_path_buf(),
        )) {
            pipeline_handle.abort();
            return Err(anyhow!("Failed to load pipeline: {}", e));
        }

        let total_groups = groups.len();
        let total_images: usize = groups.values().map(|v| v.len()).sum();
        let input_is_dir = self
            .args
            .input
            .as_ref()
            .map(|i| i.is_dir())
            .unwrap_or(false);
        let output_is_dir =
            output_path.is_dir() || input_is_dir || total_groups > 1 || total_images > 1;

        if output_is_dir && !output_path.exists() {
            fs::create_dir_all(output_path).with_context(|| {
                format!(
                    "Failed to create output directory: {}",
                    output_path.display()
                )
            })?;
        }

        for (group_index, (group_name, group_files)) in groups.iter().enumerate() {
            if self.args.verbose {
                println!(
                    "{} [{}/{}]: {} ({} image(s))",
                    "Processing group".green(),
                    group_index + 1,
                    total_groups,
                    group_name.cyan(),
                    group_files.len()
                );
            }

            self.process_group(
                group_name,
                group_files,
                output_path,
                &view_sender,
                &mut view_recv,
                &mut frame_rx,
                (group_index + 1) as u64,
            )
            .await
            .with_context(|| format!("Failed to process group: {group_name}"))?;
        }

        pipeline_handle.abort();

        if self.args.verbose {
            println!(
                "{}",
                format!(
                    "Successfully processed {total_groups} group(s) with {total_images} total image(s)."
                )
                .green()
            );
        }

        Ok(())
    }

    async fn process_group(
        &self,
        group_name: &str,
        group_files: &[PathBuf],
        output_path: &Path,
        view_sender: &broadcast::Sender<PipelineCommand>,
        view_recv: &mut broadcast::Receiver<PipelineCommand>,
        frame_rx: &mut watch::Receiver<Option<OutputFrame>>,
        expected_generation: u64,
    ) -> Result<()> {
        // Only feedback generated by this group's run should be able to fail it.
        drain(view_recv);
        view_sender
            .send(PipelineCommand::Load(group_files.to_vec()))
            .map_err(|e| {
                anyhow!(
                    "Failed to send load command for group {}: {}",
                    group_name,
                    e
                )
            })?;

        self.await_generation(view_recv, frame_rx, expected_generation, group_name)
            .await?;

        let input_is_dir = self
            .args
            .input
            .as_ref()
            .map(|i| i.is_dir())
            .unwrap_or(false);
        let output_is_dir = output_path.is_dir() || input_is_dir;

        let output_file = if output_is_dir {
            if group_files.len() == 1 {
                // For single files, preserve the directory structure and filename
                self.generate_output_filename_with_structure(&group_files[0], output_path)?
            } else {
                // For multiple files, use group-based naming
                let output_filename =
                    self.generate_group_output_filename(group_name, &group_files[0])?;
                output_path.join(output_filename)
            }
        } else {
            output_path.to_path_buf()
        };

        // The backend writes the file: the pipeline holds its output in BGRA (the
        // format GPUI renders), and only the backend owns the GPU colorspace
        // converter that turns it back into RGBA on the way to disk.
        drain(view_recv);
        view_sender
            .send(PipelineCommand::SaveOutput(output_file.clone()))
            .map_err(|e| {
                anyhow!(
                    "Failed to send save command for group {}: {}",
                    group_name,
                    e
                )
            })?;

        self.await_save(view_recv, group_name)
            .await
            .with_context(|| format!("Output file: {}", output_file.display()))?;

        if self.args.verbose {
            println!(
                "  {} {}",
                "→".green(),
                output_file.display().to_string().bright_white()
            );
        }

        Ok(())
    }

    /// Wait for the frame produced by `expected_generation`, draining backend
    /// feedback meanwhile so a failed group reports why it failed instead of
    /// expiring the timeout in silence.
    async fn await_generation(
        &self,
        view_recv: &mut broadcast::Receiver<PipelineCommand>,
        frame_rx: &mut watch::Receiver<Option<OutputFrame>>,
        expected_generation: u64,
        group_name: &str,
    ) -> Result<()> {
        // Absolute deadline: frames from other generations must not keep
        // extending the budget.
        let deadline = tokio::time::Instant::now() + GROUP_TIMEOUT;

        loop {
            tokio::select! {
                changed = frame_rx.changed() => {
                    changed.map_err(|e| anyhow!("Pipeline communication error: {e}"))?;
                    let generation = frame_rx
                        .borrow_and_update()
                        .as_ref()
                        .map(|frame| frame.load_generation);
                    if generation == Some(expected_generation) {
                        return Ok(());
                    }
                }
                message = view_recv.recv() => {
                    self.handle_feedback(message)?;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(anyhow!(
                        "Timeout waiting for group {group_name} processing to complete"
                    ));
                }
            }
        }
    }

    /// Wait for the backend's reply to a `SaveOutput` command. Callers drain
    /// stale feedback first, so the next reply is this save's.
    async fn await_save(
        &self,
        view_recv: &mut broadcast::Receiver<PipelineCommand>,
        group_name: &str,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + GROUP_TIMEOUT;

        loop {
            tokio::select! {
                message = view_recv.recv() => {
                    match message {
                        Ok(PipelineCommand::FrontendMessage(FrontendMessage::UserFeedback(
                            text,
                            level,
                        ))) => {
                            return match level {
                                FeedbackLevel::Success => Ok(()),
                                FeedbackLevel::Error => Err(anyhow!("{text}")),
                            };
                        }
                        other => self.handle_feedback(other)?,
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(anyhow!("Timeout waiting for group {group_name} to be written"));
                }
            }
        }
    }

    /// Turn one backend message into a hard failure if it reports an error, so a
    /// bad group aborts with the real reason rather than a bare timeout.
    fn handle_feedback(
        &self,
        message: Result<PipelineCommand, broadcast::error::RecvError>,
    ) -> Result<()> {
        match message {
            Ok(PipelineCommand::FrontendMessage(FrontendMessage::UserFeedback(text, level))) => {
                match level {
                    FeedbackLevel::Error => Err(anyhow!("{text}")),
                    FeedbackLevel::Success => {
                        if self.args.verbose {
                            println!("  {} {}", "•".green(), text);
                        }
                        Ok(())
                    }
                }
            }
            Ok(_) => Ok(()),
            // Overflow is progress chatter we do not act on; the frame channel
            // remains the authority on completion.
            Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
            Err(broadcast::error::RecvError::Closed) => {
                Err(anyhow!("Pipeline shut down unexpectedly"))
            }
        }
    }

    fn generate_group_output_filename(
        &self,
        group_name: &str,
        reference_file: &Path,
    ) -> Result<String> {
        let extension = match &self.args.format {
            OutputFormat::Preserve => reference_file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png"),
            _ => self.args.format.extension(),
        };

        let mut output_filename = String::new();

        if let Some(prefix) = &self.args.prefix {
            output_filename.push_str(prefix);
        }

        output_filename.push_str(group_name);

        if let Some(suffix) = &self.args.suffix {
            output_filename.push_str(suffix);
        }

        output_filename.push('.');
        output_filename.push_str(extension);

        Ok(output_filename)
    }

    fn generate_output_filename_with_structure(
        &self,
        input_file: &Path,
        output_base: &Path,
    ) -> Result<PathBuf> {
        let input_base = self
            .args
            .input
            .as_ref()
            .expect("input path is guaranteed to be Some in CLI mode");

        // Calculate the relative path from input base to the input file
        let relative_path = if input_base.is_dir() {
            input_file.strip_prefix(input_base).with_context(|| {
                format!(
                    "Failed to calculate relative path for {}",
                    input_file.display()
                )
            })?
        } else {
            // If input base is a file, just use the filename
            input_file
                .file_name()
                .ok_or_else(|| anyhow!("Invalid input filename: {}", input_file.display()))?
                .as_ref()
        };

        // Apply format conversion if needed
        let output_path = if !matches!(self.args.format, OutputFormat::Preserve) {
            let mut path = relative_path.to_path_buf();
            path.set_extension(self.args.format.extension());
            path
        } else {
            relative_path.to_path_buf()
        };

        // Apply prefix/suffix if needed
        let final_path = if self.args.prefix.is_some() || self.args.suffix.is_some() {
            let filename = output_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow!("Invalid filename in path: {}", output_path.display()))?;

            let extension = output_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png");

            let mut new_filename = String::new();

            if let Some(prefix) = &self.args.prefix {
                new_filename.push_str(prefix);
            }

            new_filename.push_str(filename);

            if let Some(suffix) = &self.args.suffix {
                new_filename.push_str(suffix);
            }

            new_filename.push('.');
            new_filename.push_str(extension);

            let mut new_path = output_path.clone();
            new_path.set_file_name(new_filename);
            new_path
        } else {
            output_path
        };

        // Combine with output base and ensure parent directories exist
        let full_output_path = output_base.join(final_path);

        if let Some(parent) = full_output_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create output directory: {}", parent.display())
                })?;
            }
        }

        Ok(full_output_path)
    }
}

pub async fn run_cli(args: Args) -> Result<()> {
    let processor = CliProcessor::new(args)?;
    processor.run().await
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    run_cli(args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parsing() {
        assert!(matches!(
            "png"
                .parse::<OutputFormat>()
                .expect("png should parse to OutputFormat::Png"),
            OutputFormat::Png
        ));
        assert!(matches!(
            "tiff"
                .parse::<OutputFormat>()
                .expect("tiff should parse to OutputFormat::Tiff"),
            OutputFormat::Tiff
        ));
        assert!(matches!(
            "preserve"
                .parse::<OutputFormat>()
                .expect("preserve should parse to OutputFormat::Preserve"),
            OutputFormat::Preserve
        ));
        assert!("invalid".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_grouping_mode_parsing() {
        assert!(matches!(
            "none"
                .parse::<GroupingMode>()
                .expect("none should parse to GroupingMode::None"),
            GroupingMode::None
        ));
        assert!(matches!(
            "subdirectory"
                .parse::<GroupingMode>()
                .expect("subdirectory should parse to GroupingMode::Subdirectory"),
            GroupingMode::Subdirectory
        ));
        assert!(matches!(
            "regex"
                .parse::<GroupingMode>()
                .expect("regex should parse to GroupingMode::Regex"),
            GroupingMode::Regex
        ));
        assert!("invalid".parse::<GroupingMode>().is_err());
    }

    #[test]
    fn test_regex_filter() {
        let args = Args {
            filter: r".*\.tiff?$".to_string(),
            ..Default::default()
        };
        let processor =
            CliProcessor::new(args).expect("CliProcessor should be created successfully in test");

        assert!(processor.is_image_file(Path::new("test.tif")));
        assert!(processor.is_image_file(Path::new("test.tiff")));
        assert!(!processor.is_image_file(Path::new("test.png")));
    }
}

// Implement Default for Args to support testing
impl Default for Args {
    fn default() -> Self {
        Self {
            pipeline: None,
            input: None,
            output: None,
            cli: false,
            filter: r".*\.(tif|tiff|png|jpg|jpeg)$".to_string(),
            recursive: false,
            verbose: false,
            dry_run: false,
            format: OutputFormat::Preserve,
            prefix: None,
            suffix: None,
            group: GroupingMode::None,
            group_regex: None,
        }
    }
}
