use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use strawberry_types::{Coordinates, Dimension, ImageArrangement, Shape};

use super::script::{FileAssignment, run_parser_script};

/// Return a list containing every file name for the given input files. File names must be unique.
///
/// # Errors
/// Returns an error if a path has no file name or two files share a name.
pub fn collect_file_names(files: &[PathBuf]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    files
        .iter()
        .map(|file| {
            let Some(name) = file.file_name() else {
                bail!("\"{}\" is not a file", file.display());
            };
            let name = name.to_string_lossy().into_owned();
            if !seen.insert(name.clone()) {
                bail!("Several files are named \"{name}\"; hyperstack files need distinct names");
            }
            Ok(name)
        })
        .collect()
}

/// Where a parser script places each file in a hyperstack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperstackLayout {
    /// Each input file's coordinates, or `None` when the script leaves the file out.
    pub coordinates: Vec<Option<Coordinates>>,
    pub shape: Shape,
}

impl HyperstackLayout {
    /// Run a parser script on file names and categorize the files according to its results.
    ///
    /// # Errors
    /// Returns an error if the script fails or its placements do not form a complete grid.
    pub fn from_script(script: &str, file_names: &[String]) -> Result<Self> {
        Self::from_assignments(file_names, &run_parser_script(script, file_names)?)
    }

    /// Place files by the dimension values a parser script assigned them; each dimension's
    /// values are numbered from zero by `number_labels`.
    ///
    /// # Errors
    /// Returns an error if an assignment names an unknown or repeated file, its dimensions
    /// string is malformed, or the placed files do not form a complete grid.
    fn from_assignments(file_names: &[String], assignments: &[FileAssignment]) -> Result<Self> {
        let file_indices: HashMap<&str, usize> = file_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index))
            .collect();
        let mut values: Vec<Option<HashMap<Dimension, &str>>> = vec![None; file_names.len()];
        for FileAssignment {
            file_name,
            dimensions,
        } in assignments
        {
            let slot = file_indices
                .get(file_name.as_str())
                .and_then(|&index| values.get_mut(index))
                .ok_or_else(|| anyhow!("\"{file_name}\" is not one of the files"))?;
            ensure!(slot.is_none(), "\"{file_name}\" is returned more than once");
            *slot = Some(
                parse_dimensions(dimensions)
                    .with_context(|| format!("Invalid dimensions for \"{file_name}\""))?,
            );
        }
        ensure!(
            values.iter().any(Option::is_some),
            "The script must return at least one file"
        );

        let numbering: HashMap<Dimension, HashMap<&str, usize>> = Dimension::ALL
            .into_iter()
            .map(|dimension| {
                let labels = values
                    .iter()
                    .flatten()
                    .map(|labels| labels.get(&dimension).copied().unwrap_or_default())
                    .collect();
                (dimension, number_labels(labels))
            })
            .collect();
        let shape = Dimension::ALL
            .into_iter()
            .fold(Shape::default(), |shape, dimension| {
                let extent = numbering
                    .get(&dimension)
                    .and_then(|numbers| numbers.values().max())
                    .map_or(0, |&last| last.saturating_add(1));
                shape.with_extent(dimension, extent)
            });
        let coordinates: Vec<Option<Coordinates>> = values
            .iter()
            .map(|labels| {
                labels.as_ref().map(|labels| {
                    let mut coordinates = Coordinates::default();
                    for dimension in Dimension::ALL {
                        let label = labels.get(&dimension).copied().unwrap_or_default();
                        let number = numbering
                            .get(&dimension)
                            .and_then(|numbers| numbers.get(label))
                            .copied()
                            .unwrap_or_default();
                        coordinates.set(dimension, number);
                    }
                    coordinates
                })
            })
            .collect();

        // Enrich grid collision errors with file names before the grid check reports the collision anonymously
        let mut placed_files: HashMap<Coordinates, &str> = HashMap::new();
        for (name, coordinates) in file_names.iter().zip(&coordinates) {
            if let Some(coordinates) = coordinates
                && let Some(other) = placed_files.insert(*coordinates, name)
            {
                bail!(
                    "\"{other}\" and \"{name}\" are both placed at {}",
                    coordinates.describe(shape)
                );
            }
        }
        let placed: Vec<Coordinates> = coordinates.iter().flatten().copied().collect();
        Shape::from_coordinates(&placed)?;
        Ok(Self { coordinates, shape })
    }

    /// Count the files the script placed, leaving out those it did not return.
    #[must_use]
    pub fn count_placed_files(&self) -> usize {
        self.coordinates.iter().flatten().count()
    }

    /// Removes files not placed in coordinates and return them with their generated arrangement.
    #[must_use]
    pub fn arrange<T>(&self, files: Vec<T>) -> (Vec<T>, ImageArrangement) {
        let (files, coordinates) = files
            .into_iter()
            .zip(&self.coordinates)
            .filter_map(|(file, coordinates)| coordinates.map(|coordinates| (file, coordinates)))
            .unzip();
        (files, ImageArrangement::Hyperstack(coordinates))
    }
}

/// Sort labels and remove repeats; positions along a dimension are numbered in this order.
/// Whole numbers sort by value and other labels naturally. Exact spelling breaks ties, so labels
/// natural ordering finds equal, such as "Green 1" and "Green  1", each keep one position.
pub(super) fn sort_and_dedup_labels(labels: &mut Vec<&str>) {
    let numeric = are_whole_numbers(labels);
    labels.sort_by(|a, b| {
        let order = if numeric {
            compare_numerically(a, b)
        } else {
            natord::compare(a, b)
        };
        order.then_with(|| a.cmp(b))
    });
    labels.dedup();
}

/// Number labels from zero in the order `sort_and_dedup_labels` gives them. Whole numbers of
/// equal value, such as "1" and "01", share a number, as they name the same position.
fn number_labels(mut labels: Vec<&str>) -> HashMap<&str, usize> {
    let numeric = are_whole_numbers(&labels);
    sort_and_dedup_labels(&mut labels);
    let mut numbers = HashMap::with_capacity(labels.len());
    let mut previous: Option<&str> = None;
    let mut number = 0usize;
    for label in labels {
        if let Some(previous) = previous
            && !(numeric && compare_numerically(previous, label) == Ordering::Equal)
        {
            number = number.saturating_add(1);
        }
        numbers.insert(label, number);
        previous = Some(label);
    }
    numbers
}

/// Return whether every label is a whole number written in ASCII digits.
fn are_whole_numbers(labels: &[&str]) -> bool {
    labels
        .iter()
        .all(|label| !label.is_empty() && is_digits(label))
}

/// Compare whole numbers written in ASCII digits by value, ignoring leading zeros
fn compare_numerically(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Return whether text consists only of ASCII digits.
pub(super) fn is_digits(text: &str) -> bool {
    text.chars().all(|character| character.is_ascii_digit())
}

/// Parse a dimensions string such as "c=DAPI,z=003" into each dimension's value.
fn parse_dimensions(text: &str) -> Result<HashMap<Dimension, &str>> {
    let mut values = HashMap::new();
    for pair in text
        .split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("\"{pair}\" must be written as key=value"))?;
        let key = key.trim();
        let dimension = Dimension::ALL
            .into_iter()
            .find(|dimension| dimension.get_label().eq_ignore_ascii_case(key))
            .ok_or_else(|| anyhow!("Unknown dimension \"{key}\"; use x, y, z, c, or t"))?;
        let value = value.trim();
        ensure!(
            !value.is_empty(),
            "Dimension {} has no value",
            dimension.get_label()
        );
        ensure!(
            values.insert(dimension, value).is_none(),
            "Dimension {} is given more than once",
            dimension.get_label()
        );
    }
    Ok(values)
}
