//! Moves cached planes out of memory when the system runs short of it.

use std::{
    cmp::Reverse,
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow};
use indexmap::IndexMap;
use strawberry_memory::{MemorySample, OsPressure, get_available_disk_space, sample_memory};
use strawberry_types::{FrontendMessage, ImageCollection, Offload, Plane, Position, SpillFile};
use tokio::sync::broadcast;

const GIBIBYTE: u64 = 1 << 30;

/// How long a memory reading is trusted before the system is asked again.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
/// Offloading starts once less than this share of memory is available, or less than
/// `LOW_WATER_LIMIT` on machines where that share would be more.
const LOW_WATER_PERCENT: u64 = 15;
const LOW_WATER_LIMIT: u64 = 4 * GIBIBYTE;
/// Offloading aims to bring available memory back up to this share, or to `TARGET_LIMIT`.
/// The gap above the low-water mark keeps it from starting and stopping in turn.
const TARGET_PERCENT: u64 = 25;
const TARGET_LIMIT: u64 = 8 * GIBIBYTE;
/// The least to release while memory is short, which covers a kernel warning that arrives
/// while the available figure still looks comfortable.
const MINIMUM_RELEASE: u64 = GIBIBYTE;
/// How long one round of offloading may run between batches of commands.
pub const IDLE_SLICE: Duration = Duration::from_millis(50);
/// Free space the spill file always leaves on its volume.
const DISK_RESERVE: u64 = 4 * GIBIBYTE;
/// How long offloading rests after it fails, so a persistent fault is reported at intervals
/// and not on every attempt.
const FAULT_COOLDOWN: Duration = Duration::from_secs(60);
/// The least time between two warnings that memory is low.
const WARNING_INTERVAL: Duration = Duration::from_secs(60);
/// Environment variable that forces offloading on (`always`) or off (`never`).
const MODE_VARIABLE: &str = "STRAWBERRY_OFFLOAD";
const LOW_MEMORY_WARNING: &str = "Memory is low. Cached images are being moved to disk.";

/// How strongly memory needs relieving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    Normal,
    /// Planes nobody is working with should leave memory.
    Elevated,
    /// Every plane that is not in use right now should leave memory.
    Critical,
}

/// A reading of memory pressure and the bytes to release to clear it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pressure {
    pub level: PressureLevel,
    pub deficit: u64,
}

impl Pressure {
    const NONE: Self = Self {
        level: PressureLevel::Normal,
        deficit: 0,
    };
}

/// Return `percent` hundredths of `total`.
const fn get_share(total: u64, percent: u64) -> u64 {
    total.saturating_mul(percent) / 100
}

/// Judge how short memory is and how much to release to bring it back to the target.
#[must_use]
pub fn assess_pressure(sample: &MemorySample) -> Pressure {
    let low_water = get_share(sample.total, LOW_WATER_PERCENT).min(LOW_WATER_LIMIT);
    let target = get_share(sample.total, TARGET_PERCENT).min(TARGET_LIMIT);
    let level = if sample.os_pressure == OsPressure::Critical || sample.available < low_water / 2 {
        PressureLevel::Critical
    } else if sample.os_pressure == OsPressure::Warning || sample.available < low_water {
        PressureLevel::Elevated
    } else {
        return Pressure::NONE;
    };
    Pressure {
        level,
        deficit: target.saturating_sub(sample.available).max(MINIMUM_RELEASE),
    }
}

/// Whether offloading follows memory pressure or is forced one way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OffloadMode {
    Auto,
    /// Offload everything that is not in use, whatever the system reports.
    Always,
    Never,
}

impl OffloadMode {
    /// Read the mode from the environment, following memory pressure unless told otherwise.
    fn from_environment() -> Self {
        match env::var(MODE_VARIABLE).as_deref() {
            Ok("always") => Self::Always,
            Ok("never") => Self::Never,
            _ => Self::Auto,
        }
    }
}

/// The system readings offloading depends on, which tests replace with a script.
trait SystemProbe: Send {
    /// Measure physical memory.
    fn sample_memory(&mut self) -> Result<MemorySample>;

    /// Return the bytes still free on the volume holding `path`.
    fn get_available_disk_space(&mut self, path: &Path) -> Result<u64>;
}

struct OsProbe;

impl SystemProbe for OsProbe {
    /// Measure physical memory as the operating system accounts for it.
    fn sample_memory(&mut self) -> Result<MemorySample> {
        sample_memory()
    }

    /// Ask the operating system for the free space of the volume holding `path`.
    fn get_available_disk_space(&mut self, path: &Path) -> Result<u64> {
        get_available_disk_space(path)
    }
}

/// Return the directory the spill file is created in.
fn get_spill_directory() -> PathBuf {
    // On Linux the default temporary directory is often kept in memory, where offloaded
    // images would relieve nothing.
    #[cfg(target_os = "linux")]
    if Path::new("/var/tmp").is_dir() {
        return PathBuf::from("/var/tmp");
    }
    env::temp_dir()
}

/// The image collections one pipeline stage keeps between runs.
pub struct StageImages<'a> {
    pub collections: Vec<&'a ImageCollection>,
    pub is_enabled: bool,
}

/// How much a plane is worth keeping in memory; the least valuable sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Heat {
    /// Hot planes are needed for the next edit or are on screen.
    is_hot: bool,
    /// A disabled tool's cache only matters if the tool is enabled again.
    is_enabled: bool,
    /// Stages close to a hot stage are the likeliest to be edited next.
    stage_nearness: Reverse<usize>,
    /// Caches after the edit point are rewritten by every pass, so writing them to disk
    /// is repeated work; the ones before it are written once.
    is_downstream: bool,
    /// Planes near the viewed position are the next ones scrubbed to.
    position_nearness: Reverse<usize>,
}

/// Return how many steps along time, depth, and the mosaic separate two positions.
const fn get_position_distance(from: Position, to: Position) -> usize {
    from.t
        .abs_diff(to.t)
        .saturating_add(from.z.abs_diff(to.z))
        .saturating_add(from.y.abs_diff(to.y))
        .saturating_add(from.x.abs_diff(to.x))
}

/// Order every retained plane for offloading, coldest first.
///
/// `stages` run from the loaded input through each tool to the last output. `hot_stages`
/// are the stages whose images the next processing reads, and `view` is the position on
/// screen. A plane that several stages share is ranked by the stage that needs it most.
#[must_use]
pub fn rank_offload_candidates(
    stages: &[StageImages<'_>],
    hot_stages: &[usize],
    view: Position,
) -> OffloadQueue {
    let mut ranked: IndexMap<*const Plane, (Heat, Weak<Plane>)> = IndexMap::new();
    for (index, stage) in stages.iter().enumerate() {
        let nearest_hot = hot_stages
            .iter()
            .copied()
            .min_by_key(|hot| hot.abs_diff(index));
        // With no hot stage named, no stage is close to one.
        let stage_distance = nearest_hot.map_or(usize::MAX, |hot| hot.abs_diff(index));
        for collection in &stage.collections {
            let coordinates = collection.get_shape().coordinates();
            for (coordinates, plane) in coordinates.zip(collection.planes()) {
                let position_distance = get_position_distance(coordinates.position(), view);
                let heat = Heat {
                    is_hot: stage.is_enabled && (stage_distance == 0 || position_distance == 0),
                    is_enabled: stage.is_enabled,
                    stage_nearness: Reverse(stage_distance),
                    is_downstream: nearest_hot.is_some_and(|hot| index > hot),
                    position_nearness: Reverse(position_distance),
                };
                ranked
                    .entry(Arc::as_ptr(plane))
                    .and_modify(|(known, _)| *known = heat.max(*known))
                    .or_insert_with(|| (heat, Arc::downgrade(plane)));
            }
        }
    }
    ranked.sort_by(|_, (first, _), _, (second, _)| first.cmp(second));
    let hot_start = ranked.values().take_while(|(heat, _)| !heat.is_hot).count();
    let planes: Vec<_> = ranked.into_values().map(|(_, plane)| plane).collect();
    OffloadQueue {
        next: 0,
        hot_start,
        reclaim_from: planes.len(),
        planes,
    }
}

/// Planes that may leave memory, coldest first. Weak references keep the queue from holding
/// on to planes their stage has already dropped.
pub struct OffloadQueue {
    planes: Vec<Weak<Plane>>,
    /// The next plane to consider.
    next: usize,
    /// Where the hot tier starts; its planes only leave memory under critical pressure.
    hot_start: usize,
    /// One past the hottest plane not yet asked to give up its disk copy.
    reclaim_from: usize,
}

impl OffloadQueue {
    /// Return the next plane that still exists and that `level` allows to leave memory.
    fn take_next(&mut self, level: PressureLevel) -> Option<Arc<Plane>> {
        let end = if level == PressureLevel::Critical {
            self.planes.len()
        } else {
            self.hot_start
        };
        while self.next < end {
            let plane = self.planes.get(self.next).and_then(Weak::upgrade);
            self.next = self.next.saturating_add(1);
            if plane.is_some() {
                return plane;
            }
        }
        None
    }

    /// Free room in the spill file by dropping the disk copy of a plane that is also in
    /// memory, starting from the hottest, and return whether one was dropped.
    fn reclaim_disk_copy(&mut self) -> bool {
        while let Some(index) = self.reclaim_from.checked_sub(1) {
            self.reclaim_from = index;
            let plane = self.planes.get(index).and_then(Weak::upgrade);
            if plane.is_some_and(|plane| plane.discard_disk_copy()) {
                return true;
            }
        }
        false
    }
}

/// A pressure reading and when it was taken.
struct Reading {
    taken: Instant,
    pressure: Pressure,
}

struct OffloaderState {
    probe: Box<dyn SystemProbe>,
    mode: OffloadMode,
    feedback: broadcast::Sender<FrontendMessage>,
    directory: PathBuf,
    /// The last reading, reused until it is `SAMPLE_INTERVAL` old.
    reading: Option<Reading>,
    /// Bytes released since that reading, which its figures cannot reflect yet.
    released_since_reading: u64,
    /// Created at the first offload, so a session that never runs short never touches disk.
    spill: Option<Arc<SpillFile>>,
    /// Whether the spill file's growth limit predates the last reading.
    growth_limit_is_stale: bool,
    /// Set after a failed write or a full disk; offloading rests until then.
    paused_until: Option<Instant>,
    /// Whether this spell of pressure has shown its warning; cleared by a normal reading.
    warned_this_episode: bool,
    /// When the last warning was sent, so pressure hovering around the mark cannot repeat it.
    last_warning: Option<Instant>,
}

impl OffloaderState {
    /// Return the pressure at `now`, asking the system only when the last reading is stale.
    fn read_pressure(&mut self, now: Instant) -> Pressure {
        if self.paused_until.is_some_and(|until| now < until) {
            return Pressure::NONE;
        }
        if let Some(reading) = &self.reading
            && now.saturating_duration_since(reading.taken) < SAMPLE_INTERVAL
        {
            return reading.pressure;
        }
        let pressure = match self.mode {
            OffloadMode::Never => Pressure::NONE,
            OffloadMode::Always => Pressure {
                level: PressureLevel::Critical,
                deficit: u64::MAX,
            },
            OffloadMode::Auto => match self.probe.sample_memory() {
                Ok(sample) => assess_pressure(&sample),
                Err(error) => {
                    self.pause_after_fault(now, &error.context("Cannot measure memory"));
                    Pressure::NONE
                }
            },
        };
        if self.released_since_reading > 0 {
            println!(
                "Offloaded {} MiB of cached images to disk",
                self.released_since_reading >> 20
            );
        }
        self.reading = Some(Reading {
            taken: now,
            pressure,
        });
        self.released_since_reading = 0;
        self.growth_limit_is_stale = true;
        if pressure.level == PressureLevel::Normal {
            self.warned_this_episode = false;
        }
        pressure
    }

    /// Return the spill file, creating it on first use, with its growth limited to the
    /// space its volume can spare.
    fn prepare_spill(&mut self) -> Result<Arc<SpillFile>> {
        let spill = if let Some(spill) = &self.spill {
            Arc::clone(spill)
        } else {
            let file = tempfile::tempfile_in(&self.directory)
                .with_context(|| format!("Cannot create a file in {}", self.directory.display()))?;
            let spill = SpillFile::new(file);
            self.spill = Some(Arc::clone(&spill));
            spill
        };
        if self.growth_limit_is_stale {
            let free = self.probe.get_available_disk_space(&self.directory)?;
            spill.limit_growth(free.saturating_sub(DISK_RESERVE));
            self.growth_limit_is_stale = false;
        }
        Ok(spill)
    }

    /// Tell the user why offloading stopped and rest it for a while. The planes it could not
    /// move stay in memory, so nothing is lost.
    fn pause_after_fault(&mut self, now: Instant, error: &anyhow::Error) {
        eprintln!("Offloading failed: {error:#}");
        let _ = self.feedback.send(FrontendMessage::warning(format!(
            "Could not move cached images to disk: {error:#}"
        )));
        self.paused_until = now.checked_add(FAULT_COOLDOWN);
    }

    /// Warn the user that images are being moved to disk, once per spell of pressure and
    /// not more often than `WARNING_INTERVAL`.
    fn warn_of_low_memory(&mut self, now: Instant) {
        let warned_recently = self
            .last_warning
            .is_some_and(|last| now.saturating_duration_since(last) < WARNING_INTERVAL);
        if self.warned_this_episode || warned_recently {
            return;
        }
        self.warned_this_episode = true;
        self.last_warning = Some(now);
        let _ = self
            .feedback
            .send(FrontendMessage::warning(LOW_MEMORY_WARNING));
    }

    /// Offload planes from `queue` until the pressure at `now` is covered, the queue is
    /// spent, or `time_budget` has run out.
    fn relieve(&mut self, queue: &mut OffloadQueue, time_budget: Option<Duration>, now: Instant) {
        let pressure = self.read_pressure(now);
        // A reading cannot show memory released after it was taken, so what was released
        // since counts against its deficit until the next one.
        let mut remaining = pressure.deficit.saturating_sub(self.released_since_reading);
        if pressure.level == PressureLevel::Normal || remaining == 0 {
            return;
        }
        let spill = match self.prepare_spill() {
            Ok(spill) => spill,
            Err(error) => {
                self.pause_after_fault(now, &error);
                return;
            }
        };
        let started = Instant::now();
        while remaining > 0
            && let Some(plane) = queue.take_next(pressure.level)
        {
            let outcome = loop {
                match plane.offload(&spill) {
                    Ok(Offload::DiskFull) if queue.reclaim_disk_copy() => {}
                    outcome => break outcome,
                }
            };
            match outcome {
                Ok(Offload::Released(bytes)) => {
                    remaining = remaining.saturating_sub(bytes);
                    self.released_since_reading = self.released_since_reading.saturating_add(bytes);
                    self.warn_of_low_memory(now);
                }
                Ok(Offload::InUse | Offload::AlreadyOffloaded) => {}
                Ok(Offload::DiskFull) => {
                    self.pause_after_fault(now, &anyhow!("not enough free disk space"));
                    return;
                }
                Err(error) => {
                    self.pause_after_fault(now, &error);
                    return;
                }
            }
            if time_budget.is_some_and(|budget| started.elapsed() >= budget) {
                return;
            }
        }
    }
}

/// Moves cached planes to a spill file while memory is short.
pub struct PlaneOffloader {
    state: Mutex<OffloaderState>,
}

impl PlaneOffloader {
    /// Create an offloader that follows the system's memory pressure, unless the
    /// `STRAWBERRY_OFFLOAD` environment variable forces it on or off. Warnings go to `feedback`.
    #[must_use]
    pub fn new(feedback: broadcast::Sender<FrontendMessage>) -> Self {
        Self::with_probe(Box::new(OsProbe), OffloadMode::from_environment(), feedback)
    }

    /// Create an offloader that takes its system readings from `probe`.
    fn with_probe(
        probe: Box<dyn SystemProbe>,
        mode: OffloadMode,
        feedback: broadcast::Sender<FrontendMessage>,
    ) -> Self {
        Self {
            state: Mutex::new(OffloaderState {
                probe,
                mode,
                feedback,
                directory: get_spill_directory(),
                reading: None,
                released_since_reading: 0,
                spill: None,
                growth_limit_is_stale: true,
                paused_until: None,
                warned_this_episode: false,
                last_warning: None,
            }),
        }
    }

    /// Lock the offloader, which the pipeline and a stage's worker thread share. Its fields
    /// are only bookkeeping, so a lock poisoned by a panicking holder is still usable.
    fn lock_state(&self) -> MutexGuard<'_, OffloaderState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Return how short memory is, asking the system at most every `SAMPLE_INTERVAL`.
    #[must_use]
    pub fn get_pressure(&self) -> Pressure {
        self.lock_state().read_pressure(Instant::now())
    }

    /// Offload planes from `queue`, coldest first, until memory pressure is covered, the
    /// queue is spent, or `time_budget` has run out. At least one plane is tried.
    pub fn relieve_memory_pressure(&self, queue: &mut OffloadQueue, time_budget: Option<Duration>) {
        self.lock_state()
            .relieve(queue, time_budget, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DISK_RESERVE, FAULT_COOLDOWN, GIBIBYTE, LOW_MEMORY_WARNING, OffloadMode, OffloadQueue,
        PlaneOffloader, Pressure, PressureLevel, Reading, SAMPLE_INTERVAL, StageImages,
        SystemProbe, WARNING_INTERVAL, assess_pressure, rank_offload_candidates,
    };
    use anyhow::{Result, anyhow};
    use image::{Rgba, RgbaImage};
    use std::{
        collections::VecDeque,
        path::Path,
        ptr, slice,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };
    use strawberry_memory::{MemorySample, OsPressure};
    use strawberry_types::{
        FeedbackLevel, FrontendMessage, ImageArrangement, ImageCollection, Plane, Position,
    };
    use tokio::sync::broadcast;

    const PLANE_BYTES: u64 = 4 * 4 * 4;

    fn make_plane() -> Arc<Plane> {
        Plane::new(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255])))
    }

    /// Stack planes along Z, so plane `n` sits at depth `n`.
    fn make_stack(planes: &[Arc<Plane>]) -> ImageCollection {
        ImageCollection::arrange(planes.to_vec(), &ImageArrangement::Stack).unwrap()
    }

    fn sample(available: u64, os_pressure: OsPressure) -> MemorySample {
        MemorySample {
            total: 16 * GIBIBYTE,
            available,
            footprint: GIBIBYTE,
            os_pressure,
        }
    }

    /// Readings handed out in order; the last one repeats.
    #[derive(Clone)]
    struct Script {
        samples: Arc<Mutex<VecDeque<Result<MemorySample, String>>>>,
        free_disk: Arc<AtomicU64>,
    }

    impl Script {
        fn new(samples: impl IntoIterator<Item = MemorySample>) -> Self {
            Self {
                samples: Arc::new(Mutex::new(samples.into_iter().map(Ok).collect())),
                free_disk: Arc::new(AtomicU64::new(u64::MAX)),
            }
        }

        fn failing() -> Self {
            let script = Self::new([]);
            script
                .samples
                .lock()
                .unwrap()
                .push_back(Err("no reading".into()));
            script
        }
    }

    impl SystemProbe for Script {
        fn sample_memory(&mut self) -> Result<MemorySample> {
            let mut samples = self.samples.lock().unwrap();
            let next = if samples.len() > 1 {
                samples.pop_front()
            } else {
                samples.front().cloned()
            };
            next.unwrap().map_err(|error| anyhow!(error))
        }

        fn get_available_disk_space(&mut self, _path: &Path) -> Result<u64> {
            Ok(self.free_disk.load(Ordering::Relaxed))
        }
    }

    struct Harness {
        offloader: PlaneOffloader,
        feedback: broadcast::Receiver<FrontendMessage>,
        start: Instant,
    }

    impl Harness {
        fn new(script: &Script, mode: OffloadMode) -> Self {
            let (sender, feedback) = broadcast::channel(16);
            Self {
                offloader: PlaneOffloader::with_probe(Box::new(script.clone()), mode, sender),
                feedback,
                start: Instant::now(),
            }
        }

        /// Run one round of offloading as if `elapsed` had passed since the harness began.
        fn relieve(&self, queue: &mut OffloadQueue, budget: Option<Duration>, elapsed: Duration) {
            self.offloader.lock_state().relieve(
                queue,
                budget,
                self.start.checked_add(elapsed).unwrap(),
            );
        }

        /// Pin the pressure the next round sees, in place of an assessed sample.
        fn set_pressure(&self, level: PressureLevel, deficit: u64, elapsed: Duration) {
            let mut state = self.offloader.lock_state();
            state.reading = Some(Reading {
                taken: self.start.checked_add(elapsed).unwrap(),
                pressure: Pressure { level, deficit },
            });
            state.released_since_reading = 0;
            state.growth_limit_is_stale = true;
        }

        fn take_warnings(&mut self) -> Vec<String> {
            let mut warnings = Vec::new();
            while let Ok(message) = self.feedback.try_recv() {
                match message {
                    FrontendMessage::UserFeedback(text, FeedbackLevel::Warning) => {
                        warnings.push(text);
                    }
                    other => panic!("unexpected feedback: {other:?}"),
                }
            }
            warnings
        }
    }

    /// Queue `cold` planes then `hot` ones, as the ranking would.
    fn make_queue(cold: &[Arc<Plane>], hot: &[Arc<Plane>]) -> OffloadQueue {
        let planes: Vec<_> = cold.iter().chain(hot).map(Arc::downgrade).collect();
        OffloadQueue {
            next: 0,
            hot_start: cold.len(),
            reclaim_from: planes.len(),
            planes,
        }
    }

    fn count_resident(planes: &[Arc<Plane>]) -> usize {
        planes.iter().filter(|plane| plane.is_resident()).count()
    }

    /// Offer one new plane for offloading at `elapsed` and return whether it left memory.
    fn offload_one(harness: &Harness, elapsed: Duration) -> bool {
        let plane = make_plane();
        harness.relieve(&mut make_queue(slice::from_ref(&plane), &[]), None, elapsed);
        !plane.is_resident()
    }

    #[test]
    fn pressure_follows_the_available_memory_marks() {
        // On 16 GiB the low-water mark is 2.4 GiB and the target 4 GiB.
        let normal = assess_pressure(&sample(3 * GIBIBYTE, OsPressure::Normal));
        assert_eq!(normal, Pressure::NONE);

        let elevated = assess_pressure(&sample(2 * GIBIBYTE, OsPressure::Normal));
        assert_eq!(elevated.level, PressureLevel::Elevated);
        assert_eq!(elevated.deficit, 2 * GIBIBYTE);

        let critical = assess_pressure(&sample(GIBIBYTE, OsPressure::Normal));
        assert_eq!(critical.level, PressureLevel::Critical);
        assert_eq!(critical.deficit, 3 * GIBIBYTE);
    }

    #[test]
    fn the_kernel_verdict_counts_even_when_memory_looks_available() {
        let warned = assess_pressure(&sample(10 * GIBIBYTE, OsPressure::Warning));
        assert_eq!(warned.level, PressureLevel::Elevated);
        assert_eq!(warned.deficit, GIBIBYTE);

        let critical = assess_pressure(&sample(10 * GIBIBYTE, OsPressure::Critical));
        assert_eq!(critical.level, PressureLevel::Critical);
    }

    #[test]
    fn the_marks_stop_growing_on_large_machines() {
        let large = |available| MemorySample {
            total: 128 * GIBIBYTE,
            ..sample(available, OsPressure::Normal)
        };
        assert_eq!(assess_pressure(&large(5 * GIBIBYTE)), Pressure::NONE);
        let elevated = assess_pressure(&large(3 * GIBIBYTE));
        assert_eq!(elevated.level, PressureLevel::Elevated);
        assert_eq!(elevated.deficit, 5 * GIBIBYTE);
    }

    /// Return where each of `planes` sits in the queue.
    fn get_ranks(queue: &OffloadQueue, planes: &[Arc<Plane>]) -> Vec<usize> {
        planes
            .iter()
            .map(|plane| {
                queue
                    .planes
                    .iter()
                    .position(|queued| ptr::eq(queued.as_ptr(), Arc::as_ptr(plane)))
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn stages_far_from_the_edit_point_are_offloaded_first() {
        // Five stages of one plane each, edited at the end: the last stage reads stage 3.
        let planes: Vec<_> = (0..5).map(|_| make_plane()).collect();
        let collections: Vec<_> = planes
            .iter()
            .map(|plane| make_stack(slice::from_ref(plane)))
            .collect();
        let stages: Vec<_> = collections
            .iter()
            .map(|collection| StageImages {
                collections: vec![collection],
                is_enabled: true,
            })
            .collect();
        // Viewing a position none of the planes is at leaves only the hot stage hot.
        let elsewhere = Position {
            z: 9,
            ..Position::default()
        };
        let queue = rank_offload_candidates(&stages, &[3], elsewhere);
        assert_eq!(get_ranks(&queue, &planes), [0, 1, 2, 4, 3]);
        assert_eq!(queue.hot_start, 4);
    }

    #[test]
    fn upstream_goes_before_downstream_at_the_same_distance() {
        let planes: Vec<_> = (0..5).map(|_| make_plane()).collect();
        let collections: Vec<_> = planes
            .iter()
            .map(|plane| make_stack(slice::from_ref(plane)))
            .collect();
        let stages: Vec<_> = collections
            .iter()
            .map(|collection| StageImages {
                collections: vec![collection],
                is_enabled: true,
            })
            .collect();
        let elsewhere = Position {
            z: 9,
            ..Position::default()
        };
        // Edited in the middle: stages 0 and 4 are two away, 1 and 3 one away.
        let queue = rank_offload_candidates(&stages, &[2], elsewhere);
        // Coldest first that is stage 0, 4, 1, 3, then the hot stage 2.
        assert_eq!(get_ranks(&queue, &planes), [0, 2, 4, 3, 1]);
    }

    #[test]
    fn planes_far_from_the_view_go_first_and_the_viewed_one_is_hot() {
        let planes: Vec<_> = (0..4).map(|_| make_plane()).collect();
        let stack = make_stack(&planes);
        let hot = make_stack(&[make_plane()]);
        let stages = [
            StageImages {
                collections: vec![&stack],
                is_enabled: true,
            },
            StageImages {
                collections: vec![&hot],
                is_enabled: true,
            },
            StageImages {
                collections: vec![],
                is_enabled: true,
            },
        ];
        let view = Position {
            z: 1,
            ..Position::default()
        };
        let queue = rank_offload_candidates(&stages, &[2], view);
        // Depth 3 is farthest from the view, then 0 and 2 at one step; depth 1 is on screen.
        let ranks = get_ranks(&queue, &planes);
        assert_eq!(ranks[3], 0);
        assert!(ranks[0] < queue.hot_start && ranks[2] < queue.hot_start);
        assert!(ranks[1] >= queue.hot_start);
    }

    #[test]
    fn without_a_hot_stage_only_the_viewed_position_is_hot() {
        let planes = [make_plane(), make_plane()];
        let stack = make_stack(&planes);
        let stages = [StageImages {
            collections: vec![&stack],
            is_enabled: true,
        }];
        let queue = rank_offload_candidates(&stages, &[], Position::default());
        assert_eq!(get_ranks(&queue, &planes), [1, 0]);
        assert_eq!(queue.hot_start, 1);
    }

    #[test]
    fn a_disabled_tool_gives_up_its_cache_first() {
        let disabled = make_plane();
        let far = make_plane();
        let disabled_stack = make_stack(slice::from_ref(&disabled));
        let far_stack = make_stack(slice::from_ref(&far));
        let stages = [
            StageImages {
                collections: vec![&far_stack],
                is_enabled: true,
            },
            StageImages {
                collections: vec![],
                is_enabled: true,
            },
            // Next to the hot stage and on the viewed position, yet still the coldest.
            StageImages {
                collections: vec![&disabled_stack],
                is_enabled: false,
            },
            StageImages {
                collections: vec![],
                is_enabled: true,
            },
        ];
        let queue = rank_offload_candidates(&stages, &[3], Position::default());
        assert_eq!(get_ranks(&queue, &[disabled, far]), [0, 1]);
    }

    #[test]
    fn a_shared_plane_is_ranked_by_the_stage_that_needs_it_most() {
        // A pass-through tool hands its input on, so one plane sits in a far and a hot stage.
        let shared = make_plane();
        let other = make_plane();
        let far = make_stack(&[Arc::clone(&shared)]);
        let middle = make_stack(slice::from_ref(&other));
        let hot = make_stack(&[Arc::clone(&shared)]);
        let stages = [&far, &middle, &hot].map(|collection| StageImages {
            collections: vec![collection],
            is_enabled: true,
        });
        let elsewhere = Position {
            z: 9,
            ..Position::default()
        };
        let queue = rank_offload_candidates(&stages, &[2], elsewhere);
        assert_eq!(queue.planes.len(), 2);
        assert_eq!(get_ranks(&queue, &[other, shared]), [0, 1]);
        assert_eq!(queue.hot_start, 1);
    }

    #[test]
    fn offloading_stops_once_the_deficit_is_covered() {
        let harness = Harness::new(&Script::new([]), OffloadMode::Auto);
        let planes: Vec<_> = (0..4).map(|_| make_plane()).collect();
        let mut queue = make_queue(&planes, &[]);
        harness.set_pressure(PressureLevel::Elevated, 2 * PLANE_BYTES, Duration::ZERO);
        harness.relieve(&mut queue, None, Duration::ZERO);
        assert_eq!(count_resident(&planes), 2);
        assert!(!planes[0].is_resident() && !planes[1].is_resident());
    }

    #[test]
    fn a_deficit_is_not_released_twice_before_the_next_reading() {
        let script = Script::new([sample(8 * GIBIBYTE, OsPressure::Normal)]);
        let harness = Harness::new(&script, OffloadMode::Auto);
        let planes: Vec<_> = (0..4).map(|_| make_plane()).collect();
        harness.set_pressure(PressureLevel::Elevated, PLANE_BYTES, Duration::ZERO);
        harness.relieve(&mut make_queue(&planes, &[]), None, Duration::ZERO);
        harness.relieve(
            &mut make_queue(&planes, &[]),
            None,
            Duration::from_millis(100),
        );
        assert_eq!(count_resident(&planes), 3);

        // The next reading is normal, so nothing more leaves memory.
        harness.relieve(&mut make_queue(&planes, &[]), None, SAMPLE_INTERVAL);
        assert_eq!(count_resident(&planes), 3);
    }

    #[test]
    fn hot_planes_stay_unless_pressure_is_critical() {
        let harness = Harness::new(&Script::new([]), OffloadMode::Auto);
        let cold = [make_plane()];
        let hot = [make_plane()];
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, Duration::ZERO);
        harness.relieve(&mut make_queue(&cold, &hot), None, Duration::ZERO);
        assert!(!cold[0].is_resident());
        assert!(hot[0].is_resident());

        harness.set_pressure(PressureLevel::Critical, u64::MAX, Duration::ZERO);
        harness.relieve(&mut make_queue(&cold, &hot), None, Duration::ZERO);
        assert!(!hot[0].is_resident());
    }

    #[test]
    fn planes_in_use_or_already_gone_are_passed_over() {
        let harness = Harness::new(&Script::new([]), OffloadMode::Auto);
        let planes: Vec<_> = (0..3).map(|_| make_plane()).collect();
        let held = planes[0].load().unwrap();
        let mut queue = make_queue(&planes, &[]);
        let dropped = make_plane();
        queue.planes.insert(0, Arc::downgrade(&dropped));
        queue.hot_start = 4;
        queue.reclaim_from = 4;
        drop(dropped);

        harness.set_pressure(PressureLevel::Elevated, PLANE_BYTES, Duration::ZERO);
        harness.relieve(&mut queue, None, Duration::ZERO);
        assert!(planes[0].is_resident());
        assert!(!planes[1].is_resident());
        assert!(planes[2].is_resident());
        drop(held);
    }

    #[test]
    fn a_spent_time_budget_ends_the_round_after_one_plane() {
        let harness = Harness::new(&Script::new([]), OffloadMode::Auto);
        let planes: Vec<_> = (0..3).map(|_| make_plane()).collect();
        let mut queue = make_queue(&planes, &[]);
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, Duration::ZERO);
        harness.relieve(&mut queue, Some(Duration::ZERO), Duration::ZERO);
        assert_eq!(count_resident(&planes), 2);
        harness.relieve(&mut queue, Some(Duration::ZERO), Duration::ZERO);
        assert_eq!(count_resident(&planes), 1);
    }

    #[test]
    fn a_full_disk_takes_back_the_copy_of_a_plane_that_is_in_memory() {
        let script = Script::new([]);
        let harness = Harness::new(&script, OffloadMode::Auto);
        let hot = [make_plane()];
        let cold = [make_plane()];

        // Room for exactly one plane, which the hot plane takes and then is read back.
        script
            .free_disk
            .store(DISK_RESERVE + PLANE_BYTES, Ordering::Relaxed);
        harness.set_pressure(PressureLevel::Critical, u64::MAX, Duration::ZERO);
        harness.relieve(&mut make_queue(&[], &hot), None, Duration::ZERO);
        assert!(!hot[0].is_resident());
        drop(hot[0].load().unwrap());

        script.free_disk.store(DISK_RESERVE, Ordering::Relaxed);
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, Duration::ZERO);
        harness.relieve(&mut make_queue(&cold, &hot), None, Duration::ZERO);
        assert!(!cold[0].is_resident());
        assert!(hot[0].is_resident());
        assert!(!hot[0].discard_disk_copy());
    }

    #[test]
    fn a_full_disk_with_nothing_to_take_back_warns_once_and_rests() {
        let script = Script::new([]);
        let mut harness = Harness::new(&script, OffloadMode::Auto);
        let planes = [make_plane(), make_plane()];
        script.free_disk.store(DISK_RESERVE, Ordering::Relaxed);
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, Duration::ZERO);
        harness.relieve(&mut make_queue(&planes, &[]), None, Duration::ZERO);
        assert_eq!(count_resident(&planes), 2);
        let warnings = harness.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not enough free disk space"));

        // The disk has room again, but offloading rests until the cooldown has passed.
        script.free_disk.store(u64::MAX, Ordering::Relaxed);
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, Duration::from_secs(1));
        harness.relieve(&mut make_queue(&planes, &[]), None, Duration::from_secs(1));
        assert_eq!(count_resident(&planes), 2);
        assert_eq!(harness.take_warnings(), Vec::<String>::new());

        harness.set_pressure(PressureLevel::Elevated, u64::MAX, FAULT_COOLDOWN);
        harness.relieve(&mut make_queue(&planes, &[]), None, FAULT_COOLDOWN);
        assert_eq!(count_resident(&planes), 0);
    }

    #[test]
    fn the_first_release_of_an_episode_warns_once() {
        let mut harness = Harness::new(&Script::new([]), OffloadMode::Auto);
        let planes: Vec<_> = (0..3).map(|_| make_plane()).collect();
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, Duration::ZERO);
        harness.relieve(&mut make_queue(&planes, &[]), None, Duration::ZERO);
        assert_eq!(harness.take_warnings(), [LOW_MEMORY_WARNING]);

        // More planes leave memory in the same spell of pressure, well after the interval.
        let more = [make_plane()];
        let later = WARNING_INTERVAL * 2;
        harness.set_pressure(PressureLevel::Elevated, u64::MAX, later);
        harness.relieve(&mut make_queue(&more, &[]), None, later);
        assert!(!more[0].is_resident());
        assert_eq!(harness.take_warnings(), Vec::<String>::new());
    }

    #[test]
    fn nothing_released_means_no_warning() {
        let mut harness = Harness::new(&Script::new([]), OffloadMode::Auto);
        let plane = make_plane();
        let held = plane.load().unwrap();
        harness.set_pressure(PressureLevel::Critical, u64::MAX, Duration::ZERO);
        harness.relieve(
            &mut make_queue(slice::from_ref(&plane), &[]),
            None,
            Duration::ZERO,
        );
        assert_eq!(harness.take_warnings(), Vec::<String>::new());
        drop(held);
    }

    #[test]
    fn a_new_episode_warns_again_only_after_the_interval() {
        let low = sample(2 * GIBIBYTE, OsPressure::Normal);
        let fine = sample(8 * GIBIBYTE, OsPressure::Normal);
        let script = Script::new([low, fine, low, fine, low]);
        let mut harness = Harness::new(&script, OffloadMode::Auto);

        assert!(offload_one(&harness, Duration::ZERO));
        assert_eq!(harness.take_warnings().len(), 1);
        // Pressure clears and returns at once: a new episode, but too soon to warn again.
        assert!(!offload_one(&harness, SAMPLE_INTERVAL));
        assert!(offload_one(&harness, SAMPLE_INTERVAL * 2));
        assert_eq!(harness.take_warnings(), Vec::<String>::new());

        assert!(!offload_one(&harness, SAMPLE_INTERVAL * 3));
        assert!(offload_one(
            &harness,
            WARNING_INTERVAL + SAMPLE_INTERVAL * 4
        ));
        assert_eq!(harness.take_warnings().len(), 1);
    }

    #[test]
    fn the_mode_overrides_what_the_system_reports() {
        let short = Script::new([sample(GIBIBYTE / 2, OsPressure::Critical)]);
        let never = Harness::new(&short, OffloadMode::Never);
        let plane = make_plane();
        never.relieve(
            &mut make_queue(slice::from_ref(&plane), &[]),
            None,
            Duration::ZERO,
        );
        assert!(plane.is_resident());
        assert_eq!(never.offloader.get_pressure(), Pressure::NONE);

        let plentiful = Script::new([sample(12 * GIBIBYTE, OsPressure::Normal)]);
        let always = Harness::new(&plentiful, OffloadMode::Always);
        let hot = make_plane();
        always.relieve(
            &mut make_queue(&[], slice::from_ref(&hot)),
            None,
            Duration::ZERO,
        );
        assert!(!hot.is_resident());
    }

    #[test]
    fn a_failed_reading_warns_and_moves_nothing() {
        let mut harness = Harness::new(&Script::failing(), OffloadMode::Auto);
        let plane = make_plane();
        harness.relieve(
            &mut make_queue(slice::from_ref(&plane), &[]),
            None,
            Duration::ZERO,
        );
        assert!(plane.is_resident());
        let warnings = harness.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Cannot measure memory"));
    }
}
