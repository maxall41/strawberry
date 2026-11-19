use std::{collections::HashMap, iter::once};

use strawberry_types::Dimension;

use super::HyperstackLayout;

const HEADER: &str = r#"// parse_file_names should return a [file name, dimensions] tuple list, where dimensions
// is a comma-seperated "key=value" string, such as x=0,y=2,z=1,c=5,t=1.
// There is no need to specify all dimensions (x,y,z,c,t) in the dimensions string; any dimensions not included
// will be taken to not exist. Any files not returned by parse_file_names will not be loaded.
// the captures(input_text,regex_pattern) function matches input text against the given pattern and
// returns the regex groups.
"#;

/// Places files along Z in natural name order when names share no structure.
const FALLBACK_BODY: &str = r#"fn parse_file_names(file_names) {
    let planes = [];
    for name in file_names {
        let label = name;
        label.replace(",", "");
        // We failed to find a program; this applies natrual ordering.
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

/// A varying column and what its values say about the dimension it numbers.
struct Field<'a> {
    column: usize,
    values: &'a [&'a str],
    /// The dimension named by the text before the field, if any.
    key: Option<Dimension>,
    /// Whether the values are words, such as channel names, rather than numbers.
    is_text: bool,
}

/// Write a parser script for the file names: align the names, label each varying part with
/// a dimension, and keep the first program whose output places every file in a complete grid.
#[must_use]
pub fn synthesize_parser_script(file_names: &[String]) -> String {
    let names: Vec<&str> = file_names.iter().map(String::as_str).collect();
    let candidates: Vec<String> = [true, false]
        .into_iter()
        .filter_map(|refine| align_names(&names, refine))
        .map(|columns| render_script(&columns))
        .collect();
    candidates
        .iter()
        .find(|script| places_every_file(script, file_names))
        .or_else(|| candidates.first())
        .cloned()
        .unwrap_or_else(|| format!("{HEADER}{FALLBACK_BODY}"))
}

/// Return whether a script runs and places every file in a complete grid.
fn places_every_file(script: &str, file_names: &[String]) -> bool {
    HyperstackLayout::from_script(script, file_names)
        .is_ok_and(|layout| layout.coordinates.iter().all(Option::is_some))
}

/// Split text into maximal runs of characters that agree on `kind`.
fn split_runs(text: &str, kind: impl Fn(char) -> bool) -> Vec<&str> {
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

/// Align names into columns at their separators, refining varying segments into letter and
/// digit runs when `refine` holds; `None` if the names do not share their separators.
fn align_names<'a>(names: &[&'a str], refine: bool) -> Option<Vec<Column<'a>>> {
    let segmented: Vec<Vec<&str>> = names
        .iter()
        .map(|name| split_runs(name, char::is_alphanumeric))
        .collect();
    let first = segmented.first()?;
    if segmented.iter().any(|runs| runs.len() != first.len()) {
        return None;
    }
    let mut columns = Vec::new();
    for (index, &run) in first.iter().enumerate() {
        let values: Vec<&str> = segmented
            .iter()
            .filter_map(|runs| runs.get(index).copied())
            .collect();
        if values.iter().all(|&value| value == run) {
            columns.push(Column::Literal(run));
            continue;
        }
        if !run.chars().all(char::is_alphanumeric) {
            return None;
        }
        let next_separator = first
            .get(index.saturating_add(1))
            .and_then(|separator| separator.chars().next());
        let refined = refine.then(|| refine_segment(&values)).flatten();
        columns.extend(refined.unwrap_or_else(|| vec![opaque_field(values, next_separator)]));
    }
    Some(columns)
}

/// Split a varying segment into literal and varying letter or digit runs, if its runs line up.
fn refine_segment<'a>(values: &[&'a str]) -> Option<Vec<Column<'a>>> {
    let split: Vec<Vec<&str>> = values
        .iter()
        .map(|value| split_runs(value, |character| character.is_ascii_digit()))
        .collect();
    let first = split.first()?;
    let is_digits = |run: &str| run.chars().all(|character| character.is_ascii_digit());
    let mut columns = Vec::new();
    for (index, &run) in first.iter().enumerate() {
        let runs: Vec<&str> = split
            .iter()
            .filter_map(|runs| runs.get(index).copied())
            .collect();
        if runs.len() != values.len()
            || runs.iter().any(|&other| is_digits(other) != is_digits(run))
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
    split
        .iter()
        .all(|runs| runs.len() == first.len())
        .then_some(columns)
}

/// Match a whole varying segment up to the separator that follows it.
fn opaque_field(values: Vec<&str>, next_separator: Option<char>) -> Column<'_> {
    let pattern = next_separator.map_or_else(
        || ".+".to_owned(),
        |separator| format!("[^{}]+", regex::escape(&separator.to_string())),
    );
    Column::Field { pattern, values }
}

/// Return the dimension named by the letters just before a column, ignoring separators.
fn key_before(columns: &[Column], column: usize) -> Option<Dimension> {
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
    // camelCase names join the key to the word before it ("stackTime"), so also try each
    // suffix starting at a lowercase-to-uppercase change; longest first keeps "timePoint" whole.
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

/// Return whether two fields' values determine each other, so one of them adds nothing.
fn determine_each_other(a: &[&str], b: &[&str]) -> bool {
    let mut forward = HashMap::new();
    let mut backward = HashMap::new();
    a.iter().zip(b).all(|(left, right)| {
        *forward.entry(left).or_insert(right) == right
            && *backward.entry(right).or_insert(left) == left
    })
}

/// Choose the dimension of each field worth capturing, keyed fields first, so every file keeps
/// distinct coordinates; fields that repeat another field's information are left uncaptured.
fn label_fields(fields: &[Field]) -> HashMap<usize, Dimension> {
    let mut labels: Vec<(&Field, Dimension)> = Vec::new();
    let is_redundant = |field: &Field, labels: &[(&Field, Dimension)]| {
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
    let unkeyed = fields.iter().filter(|field| field.key.is_none());
    let (text, numbers): (Vec<&Field>, Vec<&Field>) = unkeyed.partition(|field| field.is_text);
    for field in text.into_iter().chain(numbers) {
        if is_redundant(field, &labels) {
            continue;
        }
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
    let fields: Vec<Field> = columns
        .iter()
        .enumerate()
        .filter_map(|(column, kind)| match kind {
            Column::Literal(_) => None,
            Column::Field { pattern, values } => Some(Field {
                column,
                values,
                key: key_before(columns, column),
                is_text: pattern != r"\d+",
            }),
        })
        .collect();
    let labels = label_fields(&fields);

    let mut pattern = String::from("^");
    let mut captures: HashMap<Dimension, Vec<usize>> = HashMap::new();
    let mut group = 0usize;
    for (column, kind) in columns.iter().enumerate() {
        match (kind, labels.get(&column)) {
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

    let dimensions: Vec<String> = Dimension::ALL
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
        dimensions.join(",")
    )
}
