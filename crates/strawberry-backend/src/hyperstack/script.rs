use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::Entry},
    rc::Rc,
};

use anyhow::{Result, anyhow};
use regex::Regex;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Scope};

const PARSER_FUNCTION: &str = "parse_file_names";
const MAX_OPERATIONS: u64 = 50_000_000;
const MAX_STRING_SIZE: usize = 1 << 20;
const MAX_ARRAY_SIZE: usize = 1 << 22;
const MAX_EXPRESSION_DEPTH: usize = 64;
const MAX_FUNCTION_EXPRESSION_DEPTH: usize = 32;

/// A file a parser script keeps, with the dimensions string the script assigned it.
pub(super) struct FileAssignment {
    pub(super) file_name: String,
    pub(super) dimensions: String,
}

/// Run a parser script's `parse_file_names` on file names, returning each file the script keeps
/// together with its dimensions string.
///
/// # Errors
/// Returns an error if the script does not compile, fails, exceeds its limits, or returns
/// anything other than `[file name, dimensions]` pairs of strings.
pub(super) fn run_parser_script(
    script: &str,
    file_names: &[String],
) -> Result<Vec<FileAssignment>> {
    let engine = build_parser_engine();
    // Rhai values are not thread-safe, so its errors are kept as text.
    let ast = engine.compile(script).map_err(|error| anyhow!("{error}"))?;
    let names: Array = file_names.iter().cloned().map(Dynamic::from).collect();
    let entries: Array = engine
        .call_fn(&mut Scope::new(), &ast, PARSER_FUNCTION, (names,))
        .map_err(|error| anyhow!("{error}"))?;
    entries
        .into_iter()
        .zip(1..)
        .map(|(entry, number)| read_entry(entry, number))
        .collect()
}

/// Build an engine offering regex captures, with limits that stop runaway scripts.
fn build_parser_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_strict_variables(true)
        .set_max_operations(MAX_OPERATIONS)
        .set_max_string_size(MAX_STRING_SIZE)
        .set_max_array_size(MAX_ARRAY_SIZE)
        .set_max_expr_depths(MAX_EXPRESSION_DEPTH, MAX_FUNCTION_EXPRESSION_DEPTH);
    // Scripts call regex for every file-name; hence, we cache constructed regexs to avoid work duplication.
    let cached_regex_patterns: Rc<RefCell<HashMap<String, Regex>>> = Rc::default();
    engine.register_fn(
        "captures",
        move |text: &str, pattern: &str| -> Result<Array, Box<EvalAltResult>> {
            let mut patterns = cached_regex_patterns.borrow_mut();
            let regex = match patterns.entry(pattern.to_owned()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(
                    Regex::new(pattern).map_err(|error| format!("Invalid pattern: {error}"))?,
                ),
            };
            Ok(regex.captures(text).map_or_else(Array::new, |captures| {
                captures
                    .iter()
                    .map(|group| {
                        Dynamic::from(group.map_or_else(String::new, |m| m.as_str().to_owned()))
                    })
                    .collect()
            }))
        },
    );
    engine
}

/// Read one returned entry, numbered from one, as a file name and its dimensions string.
fn read_entry(entry: Dynamic, number: usize) -> Result<FileAssignment> {
    let pair = entry.into_array().map_err(|found| {
        anyhow!("Entry {number} must be [file name, dimensions], found {found}")
    })?;
    let [name, dimensions] = <[Dynamic; 2]>::try_from(pair)
        .map_err(|pair| anyhow!("Entry {number} must hold 2 values, found {}", pair.len()))?;
    let read_text = |value: Dynamic, role: &str| {
        value
            .into_string()
            .map_err(|found| anyhow!("Entry {number}'s {role} must be a string, found {found}"))
    };
    Ok(FileAssignment {
        file_name: read_text(name, "file name")?,
        dimensions: read_text(dimensions, "dimensions")?,
    })
}
