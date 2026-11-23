// wgpu auto-trait checks exceed the default compiler recursion depth.
#![recursion_limit = "256"]

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use colored::Colorize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Display;
use std::fs;
use std::future::{Future, pending};
use std::path::{Path, PathBuf};
use tokio::sync::{broadcast, watch};
use tokio::time::{Duration, Instant, sleep_until, timeout_at};

use strawberry_backend::{
    Pipeline,
    hyperstack::{HyperstackLayout, collect_file_names},
};
use strawberry_types::{
    ExportScope, FeedbackLevel, FrontendMessage, ImageArrangement, OutputFrame, PipelineCommand,
    Shape,
};

/// Most group names a warning lists for one channel count before eliding the rest.
const MAX_LISTED_GROUP_NAMES: usize = 3;

/// Warn when groups load different numbers of channels, since channel-specific tool settings
/// then pick different images, or none, in each group.
fn warn_about_channel_mismatch(groups: &HashMap<String, ImageGroup>) {
    // Ordered so the warning lists channel counts and group names predictably.
    let mut groups_by_channels: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    for (name, group) in groups {
        groups_by_channels
            .entry(group.channels)
            .or_default()
            .insert(name);
    }
    if groups_by_channels.len() <= 1 {
        return;
    }
    let counts: Vec<String> = groups_by_channels
        .iter()
        .map(|(channels, names)| {
            let mut listed: Vec<&str> =
                names.iter().take(MAX_LISTED_GROUP_NAMES).copied().collect();
            if names.len() > MAX_LISTED_GROUP_NAMES {
                listed.push("…");
            }
            format!(
                "{channels} in {} group(s) ({})",
                names.len(),
                listed.join(", ")
            )
        })
        .collect();
    print_warning(format!(
        "Groups have different numbers of channels: {}.",
        counts.join("; ")
    ));
}

/// Print a warning in yellow, prefixed so it stands out from progress output.
fn print_warning(message: impl Display) {
    println!("{}", format!("Warning: {message}").yellow());
}

/// Print a success message in green.
fn print_success(message: impl Display) {
    println!("{}", message.to_string().green());
}

/// Discard everything currently queued on a broadcast receiver.
fn drain(receiver: &mut broadcast::Receiver<FrontendMessage>) {
    while let Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) = receiver.try_recv() {}
}

/// Sleep until `deadline`, or forever without one.
async fn sleep_until_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

/// Await `future`, failing with a timeout naming the `operation` if `deadline` passes first.
async fn await_before<T>(
    deadline: Option<Instant>,
    future: impl Future<Output = T>,
    operation: &str,
) -> Result<T> {
    match deadline {
        Some(deadline) => timeout_at(deadline, future)
            .await
            .with_context(|| format!("Timeout {operation}")),
        None => Ok(future.await),
    }
}

/// Strawberry - Advanced Image Processing Pipeline
#[derive(Parser)]
#[command(name = "strawberry")]
#[command(about = "A powerful image processing pipeline for scientific and research applications")]
#[command(version, long_about = None)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "These are independent command-line switches"
)]
pub struct Args {
    /// Strawberry pipeline archive to load (.sbp format)
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

    /// Rhai script whose `parse_file_names` function places each group's files in a hyperstack; without it, files load as channels
    #[arg(long, value_name = "SCRIPT")]
    pub hyperstack: Option<PathBuf>,

    /// Set a time limit on how long it takes to load/process/save (one group of) the pipeline; actions over this time limit fail the  pipeline.
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum GroupingMode {
    None,
    Subdirectory,
    Regex,
}

impl std::str::FromStr for GroupingMode {
    type Err = String;

    /// Parse the requested image grouping mode and its accepted aliases.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "subdirectory" | "subdir" => Ok(Self::Subdirectory),
            "regex" => Ok(Self::Regex),
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

    /// Parse the requested output format and its accepted aliases.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "preserve" => Ok(Self::Preserve),
            "png" => Ok(Self::Png),
            "tiff" | "tif" => Ok(Self::Tiff),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            _ => Err(format!("Invalid output format: {s}")),
        }
    }
}

impl OutputFormat {
    /// Return the filename extension, or an empty string for preserved formats.
    #[must_use]
    pub const fn extension(&self) -> &str {
        match self {
            Self::Preserve => "",
            Self::Png => "png",
            Self::Tiff => "tiff",
            Self::Jpeg => "jpg",
        }
    }
}

/// One group's files and how they load.
struct ImageGroup {
    /// Every file in the group; these name its output.
    files: Vec<PathBuf>,
    /// The files that load, which a hyperstack script may narrow.
    loaded_files: Vec<PathBuf>,
    arrangement: ImageArrangement,
    channels: usize,
}

impl ImageGroup {
    /// Count the files the hyperstack script left out of loading.
    const fn count_excluded_files(&self) -> usize {
        self.files.len().saturating_sub(self.loaded_files.len())
    }
}

/// Count the files that load across groups, and those the hyperstack script left out.
fn count_group_files(groups: &HashMap<String, ImageGroup>) -> (usize, usize) {
    groups.values().fold((0, 0), |(loaded, excluded), group| {
        (
            loaded.saturating_add(group.loaded_files.len()),
            excluded.saturating_add(group.count_excluded_files()),
        )
    })
}

/// Describe how many images are processed, noting any the hyperstack script left out.
fn describe_image_totals(groups: &HashMap<String, ImageGroup>) -> String {
    let (loaded, excluded) = count_group_files(groups);
    if excluded == 0 {
        format!("{loaded} total image(s)")
    } else {
        format!("{loaded} total image(s) ({excluded} left out by the hyperstack script)")
    }
}

pub struct CliProcessor {
    args: Args,
    regex: regex::Regex,
    group_regex: Option<regex::Regex>,
    hyperstack_script: Option<String>,
}

impl CliProcessor {
    /// # Errors
    /// Returns an error if configuration, image processing, file I/O, or communication fails.
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

        let hyperstack_script = args
            .hyperstack
            .as_ref()
            .map(|path| {
                fs::read_to_string(path).with_context(|| {
                    format!("Failed to read hyperstack script: {}", path.display())
                })
            })
            .transpose()?;

        // Validate grouping configuration
        if matches!(args.group, GroupingMode::Regex) && args.group_regex.is_none() {
            return Err(anyhow!(
                "Group regex pattern is required when using regex grouping mode"
            ));
        }

        if matches!(args.group, GroupingMode::Subdirectory) && !args.recursive {
            print_warning(
                "Subdirectory grouping enabled but recursive is disabled. Consider enabling --recursive for better results.",
            );
        }

        Ok(Self {
            args,
            regex,
            group_regex,
            hyperstack_script,
        })
    }

    /// # Errors
    /// Returns an error if configuration, image processing, file I/O, or communication fails.
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
            print_warning("No images found matching the filter criteria.");
            return Ok(());
        }

        let groups = self.arrange_groups(self.group_images(image_files)?)?;
        warn_about_channel_mismatch(&groups);

        if self.args.verbose {
            print_success(format!("Found {} group(s) to process:", groups.len()));
            for (group_name, group) in &groups {
                println!(
                    "  {}: {} image(s)",
                    group_name.cyan(),
                    group.loaded_files.len()
                );
                let loaded: HashSet<&PathBuf> = group.loaded_files.iter().collect();
                for file in &group.files {
                    let line = if loaded.contains(file) {
                        file.display().to_string().dimmed()
                    } else {
                        format!("{} (left out by the hyperstack script)", file.display()).yellow()
                    };
                    println!("    {line}");
                }
            }
        }

        if self.args.dry_run {
            print_success(format!(
                "Dry run completed. {} group(s) with {} would be processed.",
                groups.len(),
                describe_image_totals(&groups)
            ));
            return Ok(());
        }

        self.process_groups(groups, output_path, pipeline_file)
            .await?;

        Ok(())
    }

    /// Validate input paths and create required output directories.
    fn validate_args(&self) -> Result<()> {
        if let Some(input) = &self.args.input
            && !input.exists()
        {
            return Err(anyhow!("Input path does not exist: {}", input.display()));
        }

        if let Some(output) = &self.args.output {
            // If input is a directory, output should be treated as a directory too
            let output_should_be_dir = self.args.input.as_ref().is_some_and(|i| i.is_dir());

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
                if let Some(parent) = output.parent()
                    && !parent.exists()
                {
                    if self.args.verbose {
                        println!("Creating output directory: {}", parent.display());
                    }
                    fs::create_dir_all(parent).with_context(|| {
                        format!("Failed to create output directory: {}", parent.display())
                    })?;
                }
            }
        }

        Ok(())
    }

    /// Collect matching input images and sort their paths.
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

    /// Collect matching files from a directory, recursing when requested.
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

    /// Check whether the filename matches the configured image filter.
    fn is_image_file(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|filename| self.regex.is_match(filename))
    }

    /// Group image paths individually, by parent directory, or by regex match.
    fn group_images(&self, image_files: Vec<PathBuf>) -> Result<HashMap<String, Vec<PathBuf>>> {
        let mut groups = HashMap::new();

        match &self.args.group {
            GroupingMode::None => {
                // Each image is its own group
                for (index, file) in image_files.into_iter().enumerate() {
                    let group_name = format!("image_{:04}", index.saturating_add(1));
                    groups.insert(group_name, vec![file]);
                }
            }
            GroupingMode::Subdirectory => {
                for file in image_files {
                    let group_name = self.get_subdirectory_group_name(&file);
                    groups.entry(group_name).or_insert_with(Vec::new).push(file);
                }
            }
            GroupingMode::Regex => {
                // Group by regex pattern match
                let group_regex = self
                    .group_regex
                    .as_ref()
                    .ok_or_else(|| anyhow!("Group regex is required"))?;
                for file in image_files {
                    let filename = file
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or_else(|| anyhow!("Invalid filename: {}", file.display()))?;

                    let group_name = group_regex
                        .captures(filename)
                        .and_then(|captures| captures.get(1).or_else(|| captures.get(0)))
                        .map_or_else(
                            || "ungrouped".to_string(),
                            |matched| matched.as_str().to_string(),
                        );

                    groups.entry(group_name).or_insert_with(Vec::new).push(file);
                }
            }
        }

        Ok(groups)
    }

    /// Name a file's subdirectory group by its directory's path relative to the input
    /// directory, so equally named directories in different places form separate groups.
    /// Files directly in the input directory, or outside it, are named by their directory.
    fn get_subdirectory_group_name(&self, file: &Path) -> String {
        let directory = file.parent().unwrap_or_else(|| Path::new(""));
        self.args
            .input
            .as_deref()
            .and_then(|input| directory.strip_prefix(input).ok())
            .filter(|relative| !relative.as_os_str().is_empty())
            .or_else(|| directory.file_name().map(Path::new))
            .map_or_else(
                || "root".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
    }

    /// Load the pipeline and process each image group through the backend.
    async fn process_groups(
        &self,
        groups: HashMap<String, ImageGroup>,
        output_path: &Path,
        pipeline_file: &Path,
    ) -> Result<()> {
        // Transient backend feedback accompanies completion on the frame channel.
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

        let (reply, result) = tokio::sync::oneshot::channel();
        view_sender
            .send(PipelineCommand::LoadPipeline {
                path: pipeline_file.to_path_buf(),
                cli: true,
                reply,
            })
            .map_err(|e| anyhow!("{e}"))?;
        let loaded = async {
            await_before(self.get_deadline()?, result, "loading pipeline")
                .await?
                .context("Pipeline closed while loading")?
                .map_err(|error| anyhow!("{error}"))
        }
        .await;
        if let Err(error) = loaded {
            pipeline_handle.abort();
            return Err(error.context("Failed to load pipeline"));
        }

        let total_groups = groups.len();
        let (total_images, _) = count_group_files(&groups);
        let input_is_dir = self.args.input.as_ref().is_some_and(|i| i.is_dir());
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

        let mut archive_count: usize = 0;
        for (group_index, (group_name, group)) in groups.iter().enumerate() {
            if self.args.verbose {
                println!(
                    "{} [{}/{}]: {} ({} image(s))",
                    "Processing group".green(),
                    group_index.saturating_add(1),
                    total_groups,
                    group_name.cyan(),
                    group.loaded_files.len()
                );
            }

            let scope = self
                .process_group(
                    (group_name, group),
                    output_path,
                    &view_sender,
                    &mut view_recv,
                    &mut frame_rx,
                    u64::try_from(group_index.saturating_add(1)).unwrap_or(u64::MAX),
                )
                .await
                .with_context(|| format!("Failed to process group: {group_name}"))?;
            if scope == ExportScope::AllViews {
                archive_count = archive_count.saturating_add(1);
            }
        }

        pipeline_handle.abort();
        self.warn_about_ignored_format(archive_count);

        if self.args.verbose {
            print_success(format!(
                "Successfully processed {total_groups} group(s) with {}.",
                describe_image_totals(&groups)
            ));
        }

        Ok(())
    }

    /// Load one image group, await its output frame, and save the result, returning whether it
    /// was saved as one image or as a ZIP of every image.
    async fn process_group(
        &self,
        group: (&str, &ImageGroup),
        output_path: &Path,
        view_sender: &strawberry_types::CommandSender,
        view_recv: &mut broadcast::Receiver<FrontendMessage>,
        frame_rx: &mut watch::Receiver<Option<OutputFrame>>,
        expected_generation: u64,
    ) -> Result<ExportScope> {
        let (group_name, image_group) = group;
        // One budget covers the whole group, so a slow load leaves less time to process.
        let deadline = self.get_deadline()?;
        // Only feedback generated by this group's run should be able to fail it.
        drain(view_recv);
        let (reply, loaded) = tokio::sync::oneshot::channel();
        view_sender
            .send(PipelineCommand::Load {
                files: image_group.loaded_files.clone(),
                arrangement: image_group.arrangement.clone(),
                reply,
            })
            .map_err(|e| anyhow!("Failed to send load command for group {group_name}: {e}"))?;
        await_before(deadline, loaded, "loading images")
            .await?
            .context("Pipeline closed while loading")?
            .map_err(|e| anyhow!("{e}"))?;

        let shape = self
            .await_generation(view_recv, frame_rx, expected_generation, deadline)
            .await
            .with_context(|| format!("Group {group_name} did not finish processing"))?;
        // Without a hyperstack script, files load as channels that are composited into the single
        // image the output path names. A hyperstack output holding several images is saved whole
        // as a ZIP, like the GUI's "All Images" export, rather than as its first image alone.
        let scope = if self.hyperstack_script.is_none() {
            ExportScope::CompositeView
        } else if shape.holds_several_images() {
            ExportScope::AllViews
        } else {
            ExportScope::CurrentView
        };
        let output_file = self.choose_output_file(group, output_path, scope)?;

        // The backend writes the file: the pipeline holds its output in BGRA (the
        // format GPUI renders), and only the backend owns the GPU colorspace
        // converter that turns it back into RGBA on the way to disk.
        let (reply, result) = tokio::sync::oneshot::channel();
        view_sender
            .send(PipelineCommand::SaveOutput {
                path: output_file.clone(),
                scope,
                reply,
            })
            .map_err(|e| anyhow!("{e}"))?;
        await_before(deadline, result, "saving output")
            .await?
            .context("Pipeline closed while saving")?
            .map_err(|e| anyhow!("{e}"))?;

        if self.args.verbose {
            println!(
                "  {} {}",
                "→".green(),
                output_file.display().to_string().bright_white()
            );
        }

        Ok(scope)
    }

    /// Place each group's files with the hyperstack script, or as channels without one.
    fn arrange_groups(
        &self,
        groups: HashMap<String, Vec<PathBuf>>,
    ) -> Result<HashMap<String, ImageGroup>> {
        groups
            .into_iter()
            .map(|(name, files)| {
                let group = self.arrange_group(files).with_context(|| {
                    format!("Failed to place the files of group {name} with the hyperstack script")
                })?;
                Ok((name, group))
            })
            .collect()
    }

    /// Place one group's files with the hyperstack script, or as channels without one.
    fn arrange_group(&self, files: Vec<PathBuf>) -> Result<ImageGroup> {
        let (loaded_files, arrangement) = match &self.hyperstack_script {
            Some(script) => HyperstackLayout::from_script(script, &collect_file_names(&files)?)?
                .arrange(files.clone()),
            None => (files.clone(), ImageArrangement::Channels),
        };
        let channels = arrangement.get_shape(loaded_files.len())?.c;
        Ok(ImageGroup {
            files,
            loaded_files,
            arrangement,
            channels,
        })
    }

    /// Choose where a group's output is written: a ZIP when it holds several images, with a
    /// warning when that replaces the extension of an output file the user named.
    fn choose_output_file(
        &self,
        (group_name, group): (&str, &ImageGroup),
        output_path: &Path,
        scope: ExportScope,
    ) -> Result<PathBuf> {
        let group_files = &group.files;
        let first_file = group_files
            .first()
            .ok_or_else(|| anyhow!("Image group is empty"))?;
        let input_is_dir = self.args.input.as_ref().is_some_and(|i| i.is_dir());
        let output_is_dir = output_path.is_dir() || input_is_dir;
        let output_file = if !output_is_dir {
            output_path.to_path_buf()
        } else if group_files.len() == 1 {
            // For single files, preserve the directory structure and filename
            self.generate_output_filename_with_structure(first_file, output_path)?
        } else {
            self.generate_group_output_path(group_name, first_file, output_path)?
        };
        let archive = match scope {
            ExportScope::CurrentView | ExportScope::CompositeView => return Ok(output_file),
            ExportScope::AllViews => output_file.with_extension("zip"),
        };
        if !output_is_dir && archive != output_file {
            print_warning(format!(
                "Saving {group_name} as a ZIP to {} instead of {}, since it holds several images.",
                archive.display(),
                output_file.display()
            ));
        }
        Ok(archive)
    }

    /// Warn that `--format` did not apply to the groups saved as ZIPs, whose entries are TIFF.
    fn warn_about_ignored_format(&self, archive_count: usize) {
        if archive_count > 0 && !matches!(self.args.format, OutputFormat::Preserve) {
            print_warning(format!(
                "{archive_count} group(s) held several images and were saved as ZIPs of TIFF images; --format {} applies only to single images.",
                self.args.format.extension()
            ));
        }
    }

    /// Return the deadline for an operation starting now, or `None` without `--timeout`.
    fn get_deadline(&self) -> Result<Option<Instant>> {
        self.args
            .timeout
            .map(|seconds| {
                Instant::now()
                    .checked_add(Duration::from_secs(seconds))
                    .ok_or_else(|| anyhow!("Timeout of {seconds} seconds is too long"))
            })
            .transpose()
    }

    /// Wait for the frame produced by `expected_generation` and return the output's shape,
    /// draining backend feedback meanwhile so a failed group reports why it failed instead of
    /// waiting in silence.
    async fn await_generation(
        &self,
        view_recv: &mut broadcast::Receiver<FrontendMessage>,
        frame_rx: &mut watch::Receiver<Option<OutputFrame>>,
        expected_generation: u64,
        deadline: Option<Instant>,
    ) -> Result<Shape> {
        loop {
            tokio::select! {
                changed = frame_rx.changed() => {
                    changed.map_err(|e| anyhow!("Pipeline communication error: {e}"))?;
                    let shape = frame_rx
                        .borrow_and_update()
                        .as_ref()
                        .filter(|frame| {
                            frame.load_generation == expected_generation && frame.is_current
                        })
                        .map(|frame| frame.shape);
                    if let Some(shape) = shape {
                        return Ok(shape);
                    }
                }
                message = view_recv.recv() => {
                    self.handle_feedback(message)?;
                }
                () = sleep_until_deadline(deadline) => {
                    return Err(anyhow!("Timeout waiting for processing to complete"));
                }
            }
        }
    }

    /// Turn one backend message into a hard failure if it reports an error, so a
    /// bad group aborts with the real reason rather than hanging or timing out.
    fn handle_feedback(
        &self,
        message: Result<FrontendMessage, broadcast::error::RecvError>,
    ) -> Result<()> {
        match message {
            Ok(FrontendMessage::UserFeedback(text, level)) => match level {
                FeedbackLevel::Error => Err(anyhow!("{text}")),
                FeedbackLevel::Success => {
                    if self.args.verbose {
                        println!("  {} {}", "•".green(), text);
                    }
                    Ok(())
                }
            },
            Ok(FrontendMessage::ToolEvent { event, .. }) => self.handle_feedback(Ok(*event)),
            // Overflow is progress chatter we do not act on; the frame channel
            // remains the authority on completion.
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
            Err(broadcast::error::RecvError::Closed) => {
                Err(anyhow!("Pipeline shut down unexpectedly"))
            }
        }
    }

    /// Build a group's output path under `output_path`, mirroring the subdirectories a nested
    /// group name holds and creating them.
    fn generate_group_output_path(
        &self,
        group_name: &str,
        reference_file: &Path,
        output_path: &Path,
    ) -> Result<PathBuf> {
        let group_path = Path::new(group_name);
        let directory = output_path.join(group_path.parent().unwrap_or_else(|| Path::new("")));
        fs::create_dir_all(&directory).with_context(|| {
            format!("Failed to create output directory: {}", directory.display())
        })?;
        let name = group_path
            .file_name()
            .map_or_else(|| group_name.into(), |name| name.to_string_lossy());
        Ok(directory.join(self.generate_group_output_filename(&name, reference_file)))
    }

    /// Build a group output filename using the configured format, prefix, and suffix.
    fn generate_group_output_filename(&self, group_name: &str, reference_file: &Path) -> String {
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

        output_filename
    }

    /// Build an output path preserving input subdirectories and create its parents.
    fn generate_output_filename_with_structure(
        &self,
        input_file: &Path,
        output_base: &Path,
    ) -> Result<PathBuf> {
        let input_base = self
            .args
            .input
            .as_ref()
            .ok_or_else(|| anyhow!("Input path is required"))?;

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
        let output_path = if matches!(self.args.format, OutputFormat::Preserve) {
            relative_path.to_path_buf()
        } else {
            let mut path = relative_path.to_path_buf();
            path.set_extension(self.args.format.extension());
            path
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

        if let Some(parent) = full_output_path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create output directory: {}", parent.display())
            })?;
        }

        Ok(full_output_path)
    }
}

/// # Errors
/// Returns an error if argument validation, processing, or file I/O fails.
pub async fn run_cli(args: Args) -> Result<()> {
    let processor = CliProcessor::new(args)?;
    processor.run().await
}

/// Parse command-line arguments and run headless image processing.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    run_cli(args).await
}
