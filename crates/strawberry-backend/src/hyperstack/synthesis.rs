use std::{cmp::Reverse, collections::HashMap, iter::once};

use indexmap::IndexMap;
use strawberry_types::Dimension;

use super::HyperstackLayout;

const HEADER: &str = r#"// parse_file_names should return a [file name, dimensions] tuple list, where dimensions
// is a comma-separated "key=value" string, such as x=0,y=2,z=1,c=5,t=1.
// If value is not a number, natural ordering is applied. There is no need to specify
// all dimensions (x,y,z,c,t) in the dimensions string; any dimensions not included
// will be taken to not exist. Any files not returned by parse_file_names will not be loaded.
// The captures(input_text,regex_pattern) function matches input text against the given pattern and
// returns the regex groups.
"#;

/// Places files along Z in natural name order when names share no structure.
const FALLBACK_BODY: &str = r#"fn parse_file_names(file_names) {
    let planes = [];
    for name in file_names {
        let label = name;
        label.replace(",", "");
        // We failed to find a program; this applies natural ordering.
        planes.push([name, `z=${label}`]);
    }
    planes
}
"#;

/// Words that name a dimension when they come just before a varying part of a file name.
const KEYS: [(Dimension, &[&str]); 5] = [
    (Dimension::X, &["x", "col", "column"]),
    (Dimension::Y, &["y", "row"]),
    (Dimension::Z, &["z", "slice", "plane"]),
    (Dimension::C, &["c", "ch", "channel", "w", "wavelength"]),
    (Dimension::T, &["t", "tp", "time", "timepoint", "frame"]),
];

/// How finely file names are split into runs before they are grouped and aligned.
#[derive(Debug, Clone, Copy)]
enum Granularity {
    /// Split only at separators.
    Words,
    /// Also split between letters and digits, so names that mix them differently, such as
    /// "img001" and "Thumbs", fall into separate groups.
    LettersAndDigits,
}

/// The characters a run of a file name holds, other than separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RunKind {
    /// Letters and digits together, when names are split only at separators.
    Word,
    Letters,
    Digits,
}

/// What a run must share with the runs at the same place in other names for the names to align.
#[derive(Debug, PartialEq, Eq, Hash)]
enum RunShape<'a> {
    /// Separators must match exactly.
    Separator(&'a str),
    /// Other runs need only hold the same kind of characters.
    Kind(RunKind),
}

/// File names sharing a shape, each split into its runs.
type NameGroup<'a> = Vec<Vec<&'a str>>;

impl Granularity {
    /// Return the kind of run a character belongs to, or `None` for a separator character.
    fn classify(self, character: char) -> Option<RunKind> {
        if !character.is_alphanumeric() {
            None
        } else if matches!(self, Self::Words) {
            Some(RunKind::Word)
        } else if character.is_ascii_digit() {
            Some(RunKind::Digits)
        } else {
            Some(RunKind::Letters)
        }
    }

    /// Return the shape of a run, which must match across names for them to align.
    fn get_run_shape(self, run: &str) -> RunShape<'_> {
        run.chars()
            .next()
            .and_then(|character| self.classify(character))
            .map_or(RunShape::Separator(run), RunShape::Kind)
    }
}

/// Text aligned across every file name.
#[derive(Debug)]
enum Column<'a> {
    /// Text shared by every name.
    Literal(&'a str),
    /// Text that differs between names, matched by a regex pattern.
    Field {
        pattern: String,
        values: Vec<&'a str>,
    },
}

/// Field derived from a varying column, enriched with inferred key metadata
struct EnrichedField<'a> {
    /// Index to column
    column: usize,
    /// Samples contained in this field.
    values: &'a [&'a str],
    /// The dimension inferred from the file names for this field, if any.
    key: Option<Dimension>,
    /// Whether the values are words, such as channel names, rather than numbers.
    is_text: bool,
}

/// Write a parser script that places as many of the file names as possible in a complete grid.
///
/// Names that share a shape are grouped and aligned, and each varying part is labeled with a
/// dimension. Files the script does not match are left out.
#[must_use]
pub fn synthesize_parser_script(file_names: &[String]) -> String {
    let mut best_script = None;
    let mut best_placed = 0;
    for granularity in [Granularity::Words, Granularity::LettersAndDigits] {
        let groups = group_names(file_names, granularity);
        let refinements: &[bool] = match granularity {
            Granularity::Words => &[true, false],
            // Splitting letters from digits only sets stray files apart; skip it when it would
            // divide real files, such as "DAPI" and "Cy5" channels, into competing groups.
            Granularity::LettersAndDigits if groups.get(1).is_some_and(|group| group.len() > 1) => {
                continue;
            }
            // Letter and digit runs need no refining, and matching one whole would mistake the
            // first character of the run after it for a separator.
            Granularity::LettersAndDigits => &[true],
        };
        for group in groups {
            // Groups come largest first, and a script rarely places names outside the group it
            // was written for, so lone names and groups no larger than the best score cannot win.
            if group.len() < 2 || group.len() <= best_placed {
                break;
            }
            for &refine in refinements {
                let script = render_script(&align_names(&group, refine));
                let placed = count_placed_files(&script, file_names);
                if placed == file_names.len() {
                    return script;
                }
                if best_script.is_none() || placed > best_placed {
                    best_placed = placed;
                    best_script = Some(script);
                }
            }
        }
    }
    best_script.unwrap_or_else(|| format!("{HEADER}{FALLBACK_BODY}"))
}

/// Count the files a script places, or zero if it fails or its placements do not form a
/// complete grid.
fn count_placed_files(script: &str, file_names: &[String]) -> usize {
    HyperstackLayout::from_script(script, file_names)
        .map_or(0, |layout| layout.count_placed_files())
}

/// Split file names into runs and group them by shape. Groups come largest first, and groups
/// of equal size in the order their first names appear.
fn group_names(file_names: &[String], granularity: Granularity) -> Vec<NameGroup<'_>> {
    let mut groups: IndexMap<Vec<RunShape>, NameGroup> = IndexMap::new();
    for name in file_names {
        let runs = split_runs(name, |character| granularity.classify(character));
        let shape = runs
            .iter()
            .map(|run| granularity.get_run_shape(run))
            .collect();
        groups.entry(shape).or_default().push(runs);
    }
    let mut groups: Vec<NameGroup> = groups.into_values().collect();
    groups.sort_by_key(|group| Reverse(group.len()));
    groups
}

/// Return whether text consists only of ASCII digits.
fn is_digits(text: &str) -> bool {
    text.chars().all(|character| character.is_ascii_digit())
}

/// Split text into maximal runs of characters that agree on `kind`.
fn split_runs<K: Copy + PartialEq>(text: &str, kind: impl Fn(char) -> K) -> Vec<&str> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut previous = None;
    for (index, character) in text.char_indices() {
        let current = kind(character);
        if previous.is_some_and(|previous| previous != current) {
            runs.extend(text.get(start..index));
            start = index;
        }
        previous = Some(current);
    }
    if !text.is_empty() {
        runs.extend(text.get(start..));
    }
    runs
}

/// Align a group's names into columns, refining varying segments into letter and digit runs
/// when `refine` is true. The group's shared shape keeps separators in literal columns.
fn align_names<'a>(group: &[Vec<&'a str>], refine: bool) -> Vec<Column<'a>> {
    let mut columns = Vec::new();
    let Some(first_file) = group.first() else {
        return columns;
    };
    for (index, &first_file_run) in first_file.iter().enumerate() {
        let runs: Vec<&str> = group
            .iter()
            .filter_map(|runs| runs.get(index).copied())
            .collect();
        if runs.iter().all(|&value| value == first_file_run) {
            columns.push(Column::Literal(first_file_run));
            continue;
        }
        let next_separator = first_file
            .get(index.saturating_add(1))
            .and_then(|separator| separator.chars().next());
        let refined = refine.then(|| refine_segment(&runs)).flatten();
        columns.extend(refined.unwrap_or_else(|| vec![match_opaque_field(runs, next_separator)]));
    }
    columns
}

/// Segment a varying segment into literal and varying letter or digit runs, if its runs line up.
fn refine_segment<'a>(runs: &[&'a str]) -> Option<Vec<Column<'a>>> {
    let segmented: Vec<Vec<&str>> = runs
        .iter()
        .map(|value| split_runs(value, |character| character.is_ascii_digit()))
        .collect();
    let first_run = segmented.first()?;
    let mut columns = Vec::new();
    for (index, &run) in first_run.iter().enumerate() {
        let runs: Vec<&str> = segmented
            .iter()
            .filter_map(|runs| runs.get(index).copied())
            .collect();
        if runs.len() != runs.len() || runs.iter().any(|&other| is_digits(other) != is_digits(run))
        {
            return None;
        }
        if runs.iter().all(|&other| other == run) {
            columns.push(Column::Literal(run));
        } else if is_digits(run) {
            columns.push(Column::Field {
                pattern: r"\d+".into(),
                values: runs,
            });
        } else if runs
            .iter()
            .all(|other| other.chars().all(char::is_alphabetic))
        {
            let ascii = runs.iter().all(|other| other.is_ascii());
            columns.push(Column::Field {
                pattern: if ascii { "[A-Za-z]+" } else { r"\p{L}+" }.into(),
                values: runs,
            });
        } else {
            return None;
        }
    }
    segmented
        .iter()
        .all(|runs| runs.len() == first_run.len())
        .then_some(columns)
}

/// Match a whole varying segment up to the separator that follows it.
fn match_opaque_field(values: Vec<&str>, next_separator: Option<char>) -> Column<'_> {
    let pattern = next_separator.map_or_else(
        || ".+".to_owned(),
        |separator| format!("[^{}]+", regex::escape(&separator.to_string())),
    );
    Column::Field { pattern, values }
}

/// Attempt to infer the dimension key for a given column, ignoring separators.
fn infer_dimension(columns: &[Column], column: usize) -> Option<Dimension> {
    let mut text = String::new();
    for previous in columns.get(..column)?.iter().rev() {
        match previous {
            Column::Literal(literal) => text.insert_str(0, literal),
            Column::Field { .. } => break,
        }
    }
    let trimmed = text.trim_end_matches(|character: char| !character.is_alphanumeric());
    let prefix = trimmed.trim_end_matches(|character: char| character.is_ascii_alphabetic());
    let word = trimmed.get(prefix.len()..)?;
    let camel_starts = word
        .as_bytes()
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| {
            matches!(pair, [before, after] if before.is_ascii_lowercase() && after.is_ascii_uppercase())
        })
        .map(|(index, _)| index.saturating_add(1));
    once(0).chain(camel_starts).find_map(|start| {
        let key = word.get(start..)?.to_ascii_lowercase();
        KEYS.iter()
            .find(|(_, words)| words.contains(&key.as_str()))
            .map(|&(dimension, _)| dimension)
    })
}

/// Return whether two fields' values determine each other.
fn determine_each_other(a: &[&str], b: &[&str]) -> bool {
    let mut forward = HashMap::new();
    let mut backward = HashMap::new();
    a.iter().zip(b).all(|(left, right)| {
        *forward.entry(left).or_insert(right) == right
            && *backward.entry(right).or_insert(left) == left
    })
}

/// Removes redundant fields, labels unlabeled fields, and produces column index to `Dimension` `HashMap`.
fn label_fields(fields: &[EnrichedField]) -> HashMap<usize, Dimension> {
    let mut labels: Vec<(&EnrichedField, Dimension)> = Vec::new();
    let is_redundant = |field: &EnrichedField, labels: &[(&EnrichedField, Dimension)]| {
        labels
            .iter()
            .any(|(other, _)| determine_each_other(field.values, other.values))
    };
    for field in fields {
        if let Some(key) = field.key {
            let shares_key = labels.iter().any(|&(_, dimension)| dimension == key);
            if !(shares_key && is_redundant(field, &labels)) {
                labels.push((field, key));
            }
        }
    }
    // Sort unkeys to ensure text fields get priority for their preferences, and vice versa for numeric fields.
    let mut unkeyed: Vec<&EnrichedField> =
        fields.iter().filter(|field| field.key.is_none()).collect();
    unkeyed.sort_by_key(|field| !field.is_text);
    for field in unkeyed {
        if is_redundant(field, &labels) {
            continue;
        }
        // The following are in preference order
        let preferred = if field.is_text {
            [Dimension::C, Dimension::Z, Dimension::T]
        } else {
            [Dimension::Z, Dimension::T, Dimension::C]
        };
        let dimension = preferred
            .into_iter()
            .find(|&dimension| labels.iter().all(|&(_, taken)| taken != dimension))
            .unwrap_or(Dimension::Z);
        labels.push((field, dimension));
    }
    labels
        .into_iter()
        .map(|(field, dimension)| (field.column, dimension))
        .collect()
}

/// Write the parser script for aligned columns.
fn render_script(columns: &[Column]) -> String {
    let fields: Vec<EnrichedField> = columns
        .iter()
        .enumerate()
        .filter_map(|(column, kind)| match kind {
            Column::Literal(_) => None,
            Column::Field { values, .. } => Some(EnrichedField {
                column,
                values,
                key: infer_dimension(columns, column),
                is_text: !values.iter().all(|value| is_digits(value)),
            }),
        })
        .collect();
    let field_labels = label_fields(&fields);

    let mut pattern = String::from("^");
    let mut captures: HashMap<Dimension, Vec<usize>> = HashMap::new();
    let mut group = 0usize;
    for (column, kind) in columns.iter().enumerate() {
        match (kind, field_labels.get(&column)) {
            (Column::Literal(text), _) => pattern.push_str(&regex::escape(text)),
            (Column::Field { pattern: field, .. }, Some(&dimension)) => {
                group = group.saturating_add(1);
                captures.entry(dimension).or_default().push(group);
                pattern.push('(');
                pattern.push_str(field);
                pattern.push(')');
            }
            (Column::Field { pattern: field, .. }, None) => pattern.push_str(field),
        }
    }
    pattern.push('$');

    let dimension_kv_pairs: Vec<String> = Dimension::ALL
        .into_iter()
        .filter_map(|dimension| {
            let groups = captures.get(&dimension)?;
            let value: Vec<String> = groups
                .iter()
                .map(|group| format!("${{f[{group}]}}"))
                .collect();
            Some(format!(
                "{}={}",
                dimension.label().to_lowercase(),
                value.join("_")
            ))
        })
        .collect();
    // Back-tick strings keep backslashes verbatim, which keeps the regex readable.
    let pattern = if pattern.contains('`') {
        format!("\"{}\"", pattern.replace('\\', r"\\").replace('"', "\\\""))
    } else {
        format!("`{pattern}`")
    };
    format!(
        "{HEADER}fn parse_file_names(file_names) {{
    let planes = [];
    for name in file_names {{
        let f = captures(name, {pattern});
        if f.is_empty() {{
            continue;
        }}
        planes.push([name, `{}`]);
    }}
    planes
}}
",
        dimension_kv_pairs.join(",")
    )
}
