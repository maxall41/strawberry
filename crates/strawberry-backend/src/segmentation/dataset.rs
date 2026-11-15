use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};
use strawberry_types::{
    AnnotationKind, AnnotationStroke, BrushRadius, ClassId, MAX_CLASSES, RgbColor,
    SegmentationClass, SegmentationClasses,
};
use zip::{
    CompressionMethod, ZipArchive, ZipWriter,
    read::{ArchiveOffset, Config},
    write::SimpleFileOptions,
};

const FORMAT: &str = "strawberry-segmentation-dataset";
const VERSION: u32 = 1;
const ENCODING: &str = "BGRA8";
const MANIFEST: &str = "dataset.json";
const MAX_SAMPLES: usize = 4096;
const MAX_STROKES: usize = 100_000;
const MAX_POINTS: usize = 2_000_000;
const MAX_NAME_BYTES: usize = 4096;
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 3 * 1024 * 1024 * 1024;

pub(crate) struct Dataset {
    pub classes: SegmentationClasses,
    pub samples: Vec<DatasetSample>,
}

pub(crate) struct DatasetSample {
    pub image: Arc<RgbaImage>,
    pub annotations: Vec<(AnnotationKind, AnnotationStroke)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: String,
    version: u32,
    encoding: String,
    classes: Vec<Class>,
    samples: Vec<Sample>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Class {
    id: ClassId,
    name: String,
    color: RgbColor,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    width: u32,
    height: u32,
    strokes: Vec<Stroke>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stroke {
    kind: AnnotationKind,
    visible: bool,
    radius: BrushRadius,
    points: Arc<Vec<(u32, u32)>>,
}

fn image_bytes(width: u32, height: u32) -> Result<usize> {
    ensure!(
        width > 0 && height > 0,
        "Dataset image dimensions must be nonzero"
    );
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .context("Dataset image size overflow")?;
    ensure!(bytes <= MAX_IMAGE_BYTES, "Dataset image exceeds size limit");
    usize::try_from(bytes).context("Dataset image does not fit in memory")
}

impl Document {
    fn validate(&self, existing: Option<&Document>) -> Result<()> {
        ensure!(
            self.format == FORMAT,
            "Not a Strawberry segmentation dataset"
        );
        ensure!(
            self.version == VERSION,
            "Unsupported dataset version: {}",
            self.version
        );
        ensure!(
            self.encoding == ENCODING,
            "Unsupported dataset pixel encoding"
        );
        ensure!(
            !self.classes.is_empty() && self.classes.len() <= MAX_CLASSES,
            "Invalid dataset class count"
        );
        ensure!(
            self.samples
                .len()
                .checked_add(existing.map_or(0, |dataset| dataset.samples.len()))
                .is_some_and(|count| count <= MAX_SAMPLES),
            "Dataset sample limit exceeded"
        );
        for (index, class) in self.classes.iter().enumerate() {
            ensure!(
                !class.name.trim().is_empty() && class.name.len() <= MAX_NAME_BYTES,
                "Invalid dataset class name"
            );
            ensure!(
                self.classes
                    .iter()
                    .take(index)
                    .all(|other| other.id != class.id),
                "Duplicate dataset class ID"
            );
        }
        let mut total_bytes = existing
            .iter()
            .flat_map(|document| &document.samples)
            .try_fold(0_u64, |total, sample| {
                total
                    .checked_add(u64::try_from(image_bytes(sample.width, sample.height)?)?)
                    .context("Dataset size overflow")
            })?;
        let mut total_strokes = existing
            .iter()
            .flat_map(|document| &document.samples)
            .map(|sample| sample.strokes.len())
            .try_fold(0_usize, usize::checked_add)
            .context("Dataset stroke count overflow")?;
        let mut total_points = existing
            .iter()
            .flat_map(|document| &document.samples)
            .flat_map(|sample| &sample.strokes)
            .map(|stroke| stroke.points.len())
            .try_fold(0_usize, usize::checked_add)
            .context("Dataset point count overflow")?;
        ensure!(
            total_bytes <= MAX_TOTAL_IMAGE_BYTES,
            "Dataset decoded size limit exceeded"
        );
        ensure!(
            total_strokes <= MAX_STROKES,
            "Dataset stroke limit exceeded"
        );
        ensure!(total_points <= MAX_POINTS, "Dataset point limit exceeded");
        for sample in &self.samples {
            total_bytes = total_bytes
                .checked_add(u64::try_from(image_bytes(sample.width, sample.height)?)?)
                .context("Dataset size overflow")?;
            ensure!(
                total_bytes <= MAX_TOTAL_IMAGE_BYTES,
                "Dataset decoded size limit exceeded"
            );
            ensure!(
                !sample.strokes.is_empty(),
                "Dataset sample has no annotations"
            );
            total_strokes = total_strokes
                .checked_add(sample.strokes.len())
                .context("Dataset stroke count overflow")?;
            ensure!(
                total_strokes <= MAX_STROKES,
                "Dataset stroke limit exceeded"
            );
            for stroke in &sample.strokes {
                if let AnnotationKind::Paint(id) = stroke.kind {
                    ensure!(
                        self.classes.iter().any(|class| class.id == id),
                        "Unknown dataset stroke class"
                    );
                }
                ensure!(
                    (BrushRadius::MIN..=BrushRadius::MAX).contains(&stroke.radius.get()),
                    "Invalid dataset brush radius"
                );
                ensure!(!stroke.points.is_empty(), "Dataset stroke has no points");
                total_points = total_points
                    .checked_add(stroke.points.len())
                    .context("Dataset point count overflow")?;
                ensure!(total_points <= MAX_POINTS, "Dataset point limit exceeded");
                ensure!(
                    stroke
                        .points
                        .iter()
                        .all(|&(x, y)| x < sample.width && y < sample.height),
                    "Dataset stroke point outside image"
                );
            }
        }
        Ok(())
    }
}

impl Dataset {
    fn document(&self) -> Result<Document> {
        ensure!(
            self.classes.len() <= MAX_CLASSES,
            "Dataset class limit exceeded"
        );
        ensure!(
            self.samples.len() <= MAX_SAMPLES,
            "Dataset sample limit exceeded"
        );
        let mut classes = Vec::new();
        let mut class_mapping = HashMap::new();
        classes.try_reserve_exact(self.classes.len())?;
        for (&id, class) in &self.classes {
            ensure!(
                class.name.len() <= MAX_NAME_BYTES,
                "Dataset class name too long"
            );
            let local_id = ClassId::new(u64::try_from(classes.len())?);
            class_mapping.insert(id, local_id);
            classes.push(Class {
                id: local_id,
                name: class.name.clone(),
                color: class.color,
            });
        }
        let mut samples = Vec::new();
        samples.try_reserve_exact(self.samples.len())?;
        let mut total_strokes = 0_usize;
        let mut total_points = 0_usize;
        for sample in &self.samples {
            if sample.annotations.is_empty() {
                continue;
            }
            ensure!(
                image_bytes(sample.image.width(), sample.image.height())?
                    == sample.image.as_raw().len(),
                "Invalid dataset image buffer length"
            );
            total_strokes = total_strokes
                .checked_add(sample.annotations.len())
                .context("Dataset stroke count overflow")?;
            ensure!(
                total_strokes <= MAX_STROKES,
                "Dataset stroke limit exceeded"
            );
            let mut strokes = Vec::new();
            strokes.try_reserve_exact(sample.annotations.len())?;
            for (kind, stroke) in &sample.annotations {
                total_points = total_points
                    .checked_add(stroke.points.len())
                    .context("Dataset point count overflow")?;
                ensure!(total_points <= MAX_POINTS, "Dataset point limit exceeded");
                let kind = match kind {
                    AnnotationKind::Paint(id) => AnnotationKind::Paint(
                        *class_mapping
                            .get(id)
                            .context("Unknown dataset stroke class")?,
                    ),
                    AnnotationKind::Eraser => AnnotationKind::Eraser,
                };
                strokes.push(Stroke {
                    kind,
                    visible: stroke.visible,
                    radius: stroke.radius,
                    points: Arc::clone(&stroke.points),
                });
            }
            samples.push(Sample {
                width: sample.image.width(),
                height: sample.image.height(),
                strokes,
            });
        }
        let document = Document {
            format: FORMAT.into(),
            version: VERSION,
            encoding: ENCODING.into(),
            classes,
            samples,
        };
        Ok(document)
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let document = self.document()?;
        document.validate(None)?;
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .context("Cannot create dataset temporary file")?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Zstd)
            .compression_level(Some(3))
            .large_file(true);
        {
            let mut archive = ZipWriter::new(temporary.as_file_mut());
            archive.start_file(MANIFEST, options)?;
            serde_json::to_writer(
                LimitedWriter {
                    inner: &mut archive,
                    remaining: MAX_METADATA_BYTES,
                },
                &document,
            )
            .context("Cannot write dataset metadata")?;
            for (index, sample) in self
                .samples
                .iter()
                .filter(|sample| !sample.annotations.is_empty())
                .enumerate()
            {
                archive.start_file(format!("samples/{index}.bgra"), options)?;
                archive
                    .write_all(sample.image.as_raw())
                    .context("Cannot write dataset image")?;
            }
            archive
                .finish()
                .context("Cannot finalize dataset archive")?;
        }
        temporary
            .as_file()
            .sync_all()
            .context("Cannot sync dataset archive")?;
        temporary
            .persist(path)
            .context("Cannot replace dataset destination")?;
        Ok(())
    }

    pub(crate) fn load(path: &Path, existing: Option<&Self>) -> Result<Self> {
        let existing = existing.map(Self::document).transpose()?;
        existing
            .iter()
            .try_for_each(|document| document.validate(None))?;
        let file = File::open(path).context("Cannot open dataset")?;
        ensure!(
            file.metadata()?.len() <= MAX_ARCHIVE_BYTES,
            "Dataset archive size limit exceeded"
        );
        let mut archive = ZipArchive::with_config(
            Config {
                archive_offset: ArchiveOffset::Known(0),
            },
            file,
        )
        .context("Invalid dataset ZIP archive")?;
        ensure!(
            !archive.has_duplicate_names(),
            "Duplicate dataset archive entry"
        );
        ensure!(
            archive.len() <= MAX_SAMPLES + 1,
            "Dataset archive entry limit exceeded"
        );
        archive
            .validate_header_consistency()
            .context("Inconsistent dataset ZIP headers")?;
        let document: Document = {
            let mut entry = archive
                .by_name(MANIFEST)
                .context("Dataset archive is missing dataset.json")?;
            validate_entry(&entry)?;
            ensure!(
                entry.size() <= u64::try_from(MAX_METADATA_BYTES)?,
                "Dataset metadata limit exceeded"
            );
            let bytes = read_exact_entry(&mut entry)?;
            serde_json::from_slice(&bytes).context("Invalid dataset metadata")?
        };
        document.validate(existing.as_ref())?;
        ensure!(
            archive.len() == document.samples.len() + 1,
            "Missing or unexpected dataset archive entries"
        );
        for name in archive.file_names() {
            let name = name.context("Invalid dataset archive entry name")?;
            if name == MANIFEST {
                continue;
            }
            let index = name
                .strip_prefix("samples/")
                .and_then(|name| name.strip_suffix(".bgra"))
                .and_then(|index| index.parse::<usize>().ok())
                .context("Unexpected dataset archive entry")?;
            ensure!(
                index < document.samples.len() && name == format!("samples/{index}.bgra"),
                "Unexpected dataset image entry"
            );
        }
        let mut samples = Vec::new();
        samples.try_reserve_exact(document.samples.len())?;
        for (index, sample) in document.samples.into_iter().enumerate() {
            let mut entry = archive
                .by_name(&format!("samples/{index}.bgra"))
                .context("Missing dataset image")?;
            validate_entry(&entry)?;
            let expected = image_bytes(sample.width, sample.height)?;
            ensure!(
                entry.size() == u64::try_from(expected)?,
                "Dataset image byte count mismatch"
            );
            let pixels = read_exact_entry(&mut entry)
                .with_context(|| format!("Cannot read dataset image {index}"))?;
            let image = RgbaImage::from_raw(sample.width, sample.height, pixels)
                .context("Invalid dataset image layout")?;
            let mut annotations = Vec::new();
            annotations.try_reserve_exact(sample.strokes.len())?;
            for stroke in sample.strokes {
                annotations.push((
                    stroke.kind,
                    AnnotationStroke {
                        visible: stroke.visible,
                        radius: stroke.radius,
                        points: stroke.points,
                    },
                ));
            }
            samples.push(DatasetSample {
                image: Arc::new(image),
                annotations,
            });
        }
        let classes = document
            .classes
            .into_iter()
            .map(|class| {
                (
                    class.id,
                    SegmentationClass {
                        name: class.name,
                        color: class.color,
                        annotations: Default::default(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let classes = classes.try_into()?;
        Ok(Self { classes, samples })
    }
}

fn validate_entry<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Result<()> {
    ensure!(
        entry.compression() == CompressionMethod::Zstd,
        "Unsupported dataset compression"
    );
    ensure!(
        !entry.encrypted() && entry.is_file() && !entry.is_symlink(),
        "Invalid dataset entry type"
    );
    Ok(())
}

fn read_exact_entry<R: Read>(entry: &mut zip::read::ZipFile<'_, R>) -> Result<Vec<u8>> {
    let size = usize::try_from(entry.size()).context("Dataset entry size overflow")?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .context("Cannot allocate dataset entry")?;
    bytes.resize(size, 0);
    entry
        .read_exact(&mut bytes)
        .context("Truncated or corrupt dataset entry")?;
    let mut extra = [0_u8; 1];
    ensure!(
        entry
            .read(&mut extra)
            .context("Dataset entry checksum failure")?
            == 0,
        "Dataset entry exceeds declared size"
    );
    Ok(bytes)
}

struct LimitedWriter<W> {
    inner: W,
    remaining: usize,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("Dataset metadata limit exceeded"));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
