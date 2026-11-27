use anyhow::{Context, Result, anyhow};
use image::RgbaImage;
use range_alloc::RangeAllocator;
use std::{
    fmt::{self, Debug, Formatter},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    mem,
    ops::Range,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

/// One hyperstack plane. Its address identifies it; its BGRA pixels may be in memory or in
/// the spill file, and move between the two without the plane becoming a different one.
pub struct Plane {
    width: u32,
    height: u32,
    pixels: Mutex<Pixels>,
}

enum Pixels {
    /// In memory. `disk_copy` is set once the same pixels are also in the spill file, which
    /// lets them leave memory again without being written a second time.
    Resident {
        image: Arc<RgbaImage>,
        disk_copy: Option<SpillSlot>,
    },
    /// Only in the spill file.
    Offloaded(SpillSlot),
}

impl Pixels {
    /// Give up the pixels and return their copy in the spill file, if they have one.
    fn into_disk_copy(self) -> Option<SpillSlot> {
        match self {
            Self::Resident { disk_copy, .. } => disk_copy,
            Self::Offloaded(slot) => Some(slot),
        }
    }
}

/// What an attempt to move a plane's pixels out of memory did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offload {
    /// The pixels left memory, freeing this many bytes.
    Released(u64),
    /// Something else holds the pixels, so they stayed.
    InUse,
    /// No pixels were in memory to release.
    AlreadyOffloaded,
    /// The spill file may not grow and has no free room.
    DiskFull,
}

impl Plane {
    /// Wrap freshly produced pixels as a new plane held in memory.
    pub fn new(image: impl Into<Arc<RgbaImage>>) -> Arc<Self> {
        let image = image.into();
        Arc::new(Self {
            width: image.width(),
            height: image.height(),
            pixels: Mutex::new(Pixels::Resident {
                image,
                disk_copy: None,
            }),
        })
    }

    /// Lock the pixel state. Every transition leaves it valid, so a lock poisoned by a
    /// panicking holder is still safe to use.
    fn lock_pixels(&self) -> MutexGuard<'_, Pixels> {
        self.pixels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Return the plane's width and height without loading its pixels.
    #[must_use]
    pub const fn get_dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Return whether the pixels are in memory.
    #[must_use]
    pub fn is_resident(&self) -> bool {
        matches!(*self.lock_pixels(), Pixels::Resident { .. })
    }

    /// Return the pixels, reading them back from the spill file if they were offloaded.
    ///
    /// # Errors
    /// Returns an error if the offloaded pixels cannot be read back.
    pub fn load(&self) -> Result<Arc<RgbaImage>> {
        let mut pixels = self.lock_pixels();
        let slot = match &*pixels {
            Pixels::Resident { image, .. } => return Ok(Arc::clone(image)),
            Pixels::Offloaded(slot) => slot,
        };
        let image = RgbaImage::from_raw(self.width, self.height, slot.read()?)
            .map(Arc::new)
            .context("Offloaded image no longer matches its dimensions")?;
        // The slot can only be moved out by replacing the state, so the pixels go in first
        // and the slot follows as their disk copy.
        let offloaded = mem::replace(
            &mut *pixels,
            Pixels::Resident {
                image: Arc::clone(&image),
                disk_copy: None,
            },
        );
        if let Pixels::Resident { disk_copy, .. } = &mut *pixels {
            *disk_copy = offloaded.into_disk_copy();
        }
        drop(pixels);
        Ok(image)
    }

    /// Move the pixels out of memory, writing them to `spill` first unless a copy is already
    /// there. Pixels that something else holds stay where they are.
    ///
    /// # Errors
    /// Returns an error if the pixels cannot be written; the plane then keeps them in memory.
    pub fn offload(&self, spill: &Arc<SpillFile>) -> Result<Offload> {
        let mut pixels = self.lock_pixels();
        let Pixels::Resident { image, disk_copy } = &mut *pixels else {
            return Ok(Offload::AlreadyOffloaded);
        };
        // An empty plane holds no memory worth releasing, and the spill file cannot store
        // an empty range.
        if image.as_raw().is_empty() {
            return Ok(Offload::AlreadyOffloaded);
        }
        // Another holder keeps the pixels in memory whatever happens here, so writing them
        // out would cost disk time and free nothing.
        if Arc::strong_count(image) > 1 {
            return Ok(Offload::InUse);
        }
        let slot = match disk_copy.take() {
            Some(slot) => slot,
            None => match spill.store(image.as_raw())? {
                Some(slot) => slot,
                None => return Ok(Offload::DiskFull),
            },
        };
        let released = slot.get_length();
        *pixels = Pixels::Offloaded(slot);
        drop(pixels);
        Ok(Offload::Released(released))
    }

    /// Give up the spill-file copy of pixels that are also in memory, returning whether one
    /// was freed. Pixels that are only in the spill file keep it.
    pub fn discard_disk_copy(&self) -> bool {
        match &mut *self.lock_pixels() {
            Pixels::Resident { disk_copy, .. } => disk_copy.take().is_some(),
            Pixels::Offloaded(_) => false,
        }
    }
}

impl Debug for Plane {
    /// Describe the plane by its size and where its pixels are, leaving the pixels out.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plane")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("is_resident", &self.is_resident())
            .finish_non_exhaustive()
    }
}

/// A temporary file holding the pixels of offloaded planes.
pub struct SpillFile {
    state: Mutex<SpillState>,
}

struct SpillState {
    file: File,
    /// Which byte ranges of the file hold a plane.
    ranges: RangeAllocator<u64>,
    /// The length the file may grow to.
    limit: u64,
}

impl SpillState {
    /// Return the length of the file: the end of its last range in use.
    fn get_length(&self) -> u64 {
        self.ranges
            .allocated_ranges()
            .last()
            .map_or(0, |range| range.end)
    }

    /// Reserve a range of `length` bytes, extending the tracked space when nothing fits.
    fn reserve(&mut self, length: u64) -> Result<Range<u64>> {
        if let Ok(range) = self.ranges.allocate_range(length) {
            return Ok(range);
        }
        let end = self
            .ranges
            .initial_range()
            .end
            .checked_add(length)
            .context("Spill file is too large")?;
        self.ranges.grow_to(end);
        self.ranges
            .allocate_range(length)
            .map_err(|_| anyhow!("Spill file could not make room"))
    }

    /// Return a range to the free space and cut the file back to its last range in use.
    fn release(&mut self, range: Range<u64>) {
        let length = self.get_length();
        self.ranges.free_range(range);
        let trimmed = self.get_length();
        if trimmed < length {
            // A slot is released while it is dropped, where a failure cannot be reported.
            // Space left untrimmed is reused by later slots.
            let _ = self.file.set_len(trimmed);
        }
    }
}

impl SpillFile {
    /// Start an empty spill file in `file`, which the caller creates as a temporary file.
    #[must_use]
    pub fn new(file: File) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SpillState {
                file,
                ranges: RangeAllocator::new(0..0),
                limit: u64::MAX,
            }),
        })
    }

    /// Lock the file and its ranges. Both stay consistent between operations, so a lock
    /// poisoned by a panicking holder is still safe to use.
    fn lock_state(&self) -> MutexGuard<'_, SpillState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Return the length of the file in bytes.
    #[must_use]
    pub fn get_length(&self) -> u64 {
        self.lock_state().get_length()
    }

    /// Allow the file to grow by at most `bytes` beyond its present length.
    pub fn limit_growth(&self, bytes: u64) {
        let mut state = self.lock_state();
        state.limit = state.get_length().saturating_add(bytes);
    }

    /// Write `bytes` into free room in the file, or return `None` if that would take the
    /// file past its limit. `bytes` must not be empty.
    fn store(self: &Arc<Self>, bytes: &[u8]) -> Result<Option<SpillSlot>> {
        let mut state = self.lock_state();
        let range = state.reserve(u64::try_from(bytes.len())?)?;
        if range.end > state.limit {
            state.release(range);
            return Ok(None);
        }
        let written = state
            .file
            .seek(SeekFrom::Start(range.start))
            .and_then(|_| state.file.write_all(bytes));
        if let Err(error) = written {
            state.release(range);
            return Err(error).context("Cannot write an image to the spill file");
        }
        drop(state);
        Ok(Some(SpillSlot {
            file: Arc::clone(self),
            range,
        }))
    }
}

/// One plane's bytes in the spill file; dropping it frees them.
struct SpillSlot {
    file: Arc<SpillFile>,
    range: Range<u64>,
}

impl SpillSlot {
    /// Return the number of bytes the slot holds.
    const fn get_length(&self) -> u64 {
        self.range.end.saturating_sub(self.range.start)
    }

    /// Read the slot's bytes back from the file.
    fn read(&self) -> Result<Vec<u8>> {
        let mut bytes = vec![0; usize::try_from(self.get_length())?];
        let mut state = self.file.lock_state();
        state
            .file
            .seek(SeekFrom::Start(self.range.start))
            .and_then(|_| state.file.read_exact(&mut bytes))
            .context("Cannot read an offloaded image back from the spill file")?;
        drop(state);
        Ok(bytes)
    }
}

impl Drop for SpillSlot {
    /// Return the slot's range to the file.
    fn drop(&mut self) {
        self.file.lock_state().release(self.range.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::{Offload, Plane, SpillFile};
    use image::{Rgba, RgbaImage};
    use std::{sync::Arc, thread};

    const SIDE: u32 = 8;
    const PLANE_BYTES: u64 = 8 * 8 * 4;

    /// Build an image whose every byte depends on `seed`, so planes are told apart.
    fn make_image(seed: u8) -> RgbaImage {
        RgbaImage::from_fn(SIDE, SIDE, |x, y| {
            let value = u8::try_from(x.wrapping_add(y.wrapping_mul(SIDE)) % 251)
                .unwrap()
                .wrapping_add(seed);
            Rgba([value, value.wrapping_add(1), value.wrapping_add(2), 255])
        })
    }

    fn make_spill() -> Arc<SpillFile> {
        SpillFile::new(tempfile::tempfile().unwrap())
    }

    #[test]
    fn a_new_plane_is_resident_and_knows_its_size() {
        let plane = Plane::new(make_image(0));
        assert!(plane.is_resident());
        assert_eq!(plane.get_dimensions(), (SIDE, SIDE));
        assert!(!plane.discard_disk_copy());
    }

    #[test]
    fn offloading_then_loading_returns_the_same_pixels() {
        let spill = make_spill();
        let plane = Plane::new(make_image(3));
        assert_eq!(
            plane.offload(&spill).unwrap(),
            Offload::Released(PLANE_BYTES)
        );
        assert!(!plane.is_resident());
        assert_eq!(plane.get_dimensions(), (SIDE, SIDE));
        assert_eq!(spill.get_length(), PLANE_BYTES);

        assert_eq!(*plane.load().unwrap(), make_image(3));
        assert!(plane.is_resident());
    }

    #[test]
    fn offloading_twice_releases_once() {
        let spill = make_spill();
        let plane = Plane::new(make_image(0));
        assert_eq!(
            plane.offload(&spill).unwrap(),
            Offload::Released(PLANE_BYTES)
        );
        assert_eq!(plane.offload(&spill).unwrap(), Offload::AlreadyOffloaded);
    }

    #[test]
    fn pixels_in_use_stay_in_memory() {
        let spill = make_spill();
        let plane = Plane::new(make_image(0));
        let held = plane.load().unwrap();
        assert_eq!(plane.offload(&spill).unwrap(), Offload::InUse);
        assert!(plane.is_resident());
        assert_eq!(spill.get_length(), 0);

        drop(held);
        assert_eq!(
            plane.offload(&spill).unwrap(),
            Offload::Released(PLANE_BYTES)
        );
    }

    #[test]
    fn a_reloaded_plane_is_offloaded_again_without_a_second_write() {
        let spill = make_spill();
        let plane = Plane::new(make_image(9));
        plane.offload(&spill).unwrap();
        drop(plane.load().unwrap());
        // With no room to grow, only the copy already in the file can serve the offload.
        spill.limit_growth(0);
        assert_eq!(
            plane.offload(&spill).unwrap(),
            Offload::Released(PLANE_BYTES)
        );
        assert_eq!(spill.get_length(), PLANE_BYTES);
        assert_eq!(*plane.load().unwrap(), make_image(9));
    }

    #[test]
    fn a_disk_copy_is_discarded_only_while_the_pixels_are_in_memory() {
        let spill = make_spill();
        let plane = Plane::new(make_image(0));
        plane.offload(&spill).unwrap();
        assert!(!plane.discard_disk_copy());
        assert_eq!(spill.get_length(), PLANE_BYTES);

        drop(plane.load().unwrap());
        assert!(plane.discard_disk_copy());
        assert!(!plane.discard_disk_copy());
        assert_eq!(spill.get_length(), 0);
        assert_eq!(*plane.load().unwrap(), make_image(0));
    }

    #[test]
    fn a_full_spill_file_leaves_the_plane_in_memory() {
        let spill = make_spill();
        spill.limit_growth(0);
        let plane = Plane::new(make_image(5));
        assert_eq!(plane.offload(&spill).unwrap(), Offload::DiskFull);
        assert!(plane.is_resident());
        assert_eq!(spill.get_length(), 0);
        assert_eq!(*plane.load().unwrap(), make_image(5));

        spill.limit_growth(PLANE_BYTES);
        assert_eq!(
            plane.offload(&spill).unwrap(),
            Offload::Released(PLANE_BYTES)
        );
    }

    #[test]
    fn an_empty_plane_has_nothing_to_offload() {
        let spill = make_spill();
        let plane = Plane::new(RgbaImage::new(0, 0));
        assert_eq!(plane.offload(&spill).unwrap(), Offload::AlreadyOffloaded);
        assert!(plane.is_resident());
    }

    #[test]
    fn a_dropped_plane_frees_its_slot_for_the_next_one() {
        let spill = make_spill();
        let first = Plane::new(make_image(1));
        let second = Plane::new(make_image(2));
        first.offload(&spill).unwrap();
        second.offload(&spill).unwrap();
        assert_eq!(spill.get_length(), 2 * PLANE_BYTES);

        drop(first);
        let third = Plane::new(make_image(3));
        third.offload(&spill).unwrap();
        assert_eq!(spill.get_length(), 2 * PLANE_BYTES);
        assert_eq!(*second.load().unwrap(), make_image(2));
        assert_eq!(*third.load().unwrap(), make_image(3));
    }

    #[test]
    fn the_file_is_cut_back_to_its_last_live_slot() {
        let spill = make_spill();
        let planes: Vec<_> = (0..3).map(|seed| Plane::new(make_image(seed))).collect();
        for plane in &planes {
            plane.offload(&spill).unwrap();
        }
        assert_eq!(spill.get_length(), 3 * PLANE_BYTES);

        let mut planes = planes.into_iter();
        let (first, second, third) = (
            planes.next().unwrap(),
            planes.next().unwrap(),
            planes.next().unwrap(),
        );
        drop(third);
        assert_eq!(spill.get_length(), 2 * PLANE_BYTES);
        // A hole before the last live slot cannot shorten the file.
        drop(first);
        assert_eq!(spill.get_length(), 2 * PLANE_BYTES);
        assert_eq!(*second.load().unwrap(), make_image(1));
        drop(second);
        assert_eq!(spill.get_length(), 0);
    }

    #[test]
    fn planes_of_different_sizes_share_the_file() {
        let spill = make_spill();
        let small = Plane::new(RgbaImage::from_pixel(2, 2, Rgba([7, 8, 9, 255])));
        let large = Plane::new(make_image(4));
        small.offload(&spill).unwrap();
        large.offload(&spill).unwrap();
        assert_eq!(spill.get_length(), 16 + PLANE_BYTES);
        assert_eq!(*large.load().unwrap(), make_image(4));
        assert_eq!(
            *small.load().unwrap(),
            RgbaImage::from_pixel(2, 2, Rgba([7, 8, 9, 255]))
        );
    }

    #[test]
    fn loading_while_another_thread_offloads_always_yields_the_pixels() {
        let spill = make_spill();
        let planes: Vec<_> = (0..4).map(|seed| Plane::new(make_image(seed))).collect();
        thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..200 {
                    for plane in &planes {
                        plane.offload(&spill).unwrap();
                    }
                }
            });
            for _ in 0..2 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        for (seed, plane) in (0..).zip(&planes) {
                            assert_eq!(*plane.load().unwrap(), make_image(seed));
                        }
                    }
                });
            }
        });
    }
}
