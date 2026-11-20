use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use strawberry_types::{Coordinates, Dimension, ImageArrangement, Shape};

use super::script::{FileAssignment, run_parser_script};

/// Return a list containing every file name for the given input files. File names must be unique. File names should be ASCII.
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
    /// values are ordered naturally and numbered from zero.
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
                let mut labels: Vec<&str> = values
                    .iter()
                    .flatten()
                    .map(|labels| labels.get(&dimension).copied().unwrap_or_default())
                    .collect();
                sort_and_dedup_naturally(&mut labels);
                let numbers = labels.into_iter().zip(0..).collect();
                (dimension, numbers)
            })
            .collect();
        let shape = Dimension::ALL
            .into_iter()
            .fold(Shape::default(), |shape, dimension| {
                let extent = numbering.get(&dimension).map_or(0, HashMap::len);
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
        //
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

/// Sort labels in natural order and remove repeats; positions along a dimension are numbered
/// in this order.
pub(super) fn sort_and_dedup_naturally(labels: &mut Vec<&str>) {
    labels.sort_by(|a, b| natord::compare(a, b));
    labels.dedup();
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
            .find(|dimension| dimension.label().eq_ignore_ascii_case(key))
            .ok_or_else(|| anyhow!("Unknown dimension \"{key}\"; use x, y, z, c, or t"))?;
        let value = value.trim();
        ensure!(
            !value.is_empty(),
            "Dimension {} has no value",
            dimension.label()
        );
        ensure!(
            values.insert(dimension, value).is_none(),
            "Dimension {} is given more than once",
            dimension.label()
        );
    }
    Ok(values)
}
