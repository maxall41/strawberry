use std::ops::Range;

use super::is_word_character;

/// Rhai keywords, other than the boolean literals.
const KEYWORDS: [&str; 24] = [
    "as", "break", "catch", "const", "continue", "do", "else", "export", "fn", "for", "global",
    "if", "import", "in", "let", "loop", "private", "return", "switch", "this", "throw", "try",
    "until", "while",
];
const BOOLEANS: [&str; 2] = ["true", "false"];

/// The kind of script text a highlight colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Comment,
    /// Keywords, and the `${` and `}` around an expression interpolated into a string.
    Keyword,
    /// A function's name where it is called or defined.
    Function,
    String,
    /// Numbers and booleans.
    Constant,
}

/// A byte range of script text and the syntax it belongs to.
#[derive(Debug, Clone)]
pub struct Highlight {
    pub range: Range<usize>,
    pub syntax: Syntax,
}

/// Return the highlighted spans of a Rhai script in text order; text outside them is plain.
pub fn highlight_rhai(script: &str) -> Vec<Highlight> {
    let mut lexer = Lexer {
        script,
        offset: 0,
        highlights: Vec::new(),
        interpolations: Vec::new(),
    };
    lexer.lex_code();
    lexer.highlights
}

/// Scans a script once from start to end, recording highlights as it goes.
struct Lexer<'a> {
    script: &'a str,
    offset: usize,
    highlights: Vec<Highlight>,
    /// Braces left open inside each enclosing string interpolation, innermost last. A `}` that
    /// closes none of them ends the interpolation and resumes its string.
    interpolations: Vec<usize>,
}

impl<'a> Lexer<'a> {
    /// Return the text not yet scanned.
    fn rest(&self) -> &'a str {
        self.script.get(self.offset..).unwrap_or_default()
    }

    /// Move forward `len` bytes.
    fn advance(&mut self, len: usize) {
        self.offset = self.offset.saturating_add(len).min(self.script.len());
    }

    /// Highlight the text from `start` to the current offset as `syntax`.
    fn mark(&mut self, start: usize, syntax: Syntax) {
        if start < self.offset {
            self.highlights.push(Highlight {
                range: start..self.offset,
                syntax,
            });
        }
    }

    /// Scan code to the end of the script.
    fn lex_code(&mut self) {
        while let Some(character) = self.rest().chars().next() {
            let start = self.offset;
            let rest = self.rest();
            match character {
                '/' if rest.starts_with("//") => {
                    self.advance(rest.find('\n').unwrap_or(rest.len()));
                    self.mark(start, Syntax::Comment);
                }
                '/' if rest.starts_with("/*") => {
                    self.skip_block_comment();
                    self.mark(start, Syntax::Comment);
                }
                '"' | '\'' => {
                    self.skip_quoted(character);
                    self.mark(start, Syntax::String);
                }
                '#' if rest.trim_start_matches('#').starts_with('"') => {
                    self.skip_raw_string();
                    self.mark(start, Syntax::String);
                }
                '`' => {
                    self.advance(1);
                    self.lex_backtick_text(start);
                }
                '{' => {
                    if let Some(open) = self.interpolations.last_mut() {
                        *open = open.saturating_add(1);
                    }
                    self.advance(1);
                }
                '}' => {
                    self.advance(1);
                    match self.interpolations.last_mut() {
                        Some(0) => {
                            self.interpolations.pop();
                            self.mark(start, Syntax::Keyword);
                            self.lex_backtick_text(self.offset);
                        }
                        Some(open) => *open = open.saturating_sub(1),
                        None => {}
                    }
                }
                _ if character.is_ascii_digit() => {
                    self.skip_number();
                    self.mark(start, Syntax::Constant);
                }
                _ if is_word_character(character) => self.lex_word(),
                _ => self.advance(character.len_utf8()),
            }
        }
    }

    /// Move past a block comment, including any comments nested inside it.
    fn skip_block_comment(&mut self) {
        let mut depth = 0usize;
        loop {
            let rest = self.rest();
            if rest.starts_with("/*") {
                depth = depth.saturating_add(1);
                self.advance(2);
            } else if rest.starts_with("*/") {
                self.advance(2);
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return;
                }
            } else if let Some(character) = rest.chars().next() {
                self.advance(character.len_utf8());
            } else {
                return;
            }
        }
    }

    /// Move past a string or character quoted by `quote`, stopping before a line break the
    /// literal leaves unterminated.
    fn skip_quoted(&mut self, quote: char) {
        let rest = self.rest();
        let mut characters = rest.char_indices().skip(1);
        let mut end = rest.len();
        while let Some((index, character)) = characters.next() {
            if character == '\\' {
                // Skipping the escaped character also continues the string past an escaped line
                // break.
                characters.next();
            } else if character == '\n' {
                end = index;
                break;
            } else if character == quote {
                end = index.saturating_add(1);
                break;
            }
        }
        self.advance(end);
    }

    /// Move past a raw string such as `#"text"#`, which ends at a quote followed by as many `#`
    /// as open it.
    fn skip_raw_string(&mut self) {
        let rest = self.rest();
        let hashes = rest
            .len()
            .saturating_sub(rest.trim_start_matches('#').len());
        self.advance(hashes.saturating_add(1));
        let terminator = format!("\"{}", "#".repeat(hashes));
        let body = self.rest();
        self.advance(
            body.find(&terminator)
                .map_or(body.len(), |end| end.saturating_add(terminator.len())),
        );
    }

    /// Scan back-tick string text from `start` through its closing back-tick, or up to an
    /// interpolation, which the lexer then enters as code.
    fn lex_backtick_text(&mut self, start: usize) {
        loop {
            let rest = self.rest();
            if rest.starts_with("``") || rest.starts_with("\\$") {
                // A doubled back-tick is a literal one, and a backslash keeps `${` literal.
                self.advance(2);
            } else if rest.starts_with('`') {
                self.advance(1);
                break;
            } else if rest.starts_with("${") {
                self.mark(start, Syntax::String);
                let open = self.offset;
                self.advance(2);
                self.mark(open, Syntax::Keyword);
                self.interpolations.push(0);
                return;
            } else if let Some(character) = rest.chars().next() {
                self.advance(character.len_utf8());
            } else {
                break;
            }
        }
        self.mark(start, Syntax::String);
    }

    /// Move past a number, such as `42`, `0xFF`, `1_000`, or `2.5e-3`.
    fn skip_number(&mut self) {
        let rest = self.rest();
        let is_hex = rest.starts_with("0x");
        let mut characters = rest.char_indices().peekable();
        let mut previous = None;
        let mut end = rest.len();
        while let Some((index, character)) = characters.next() {
            let next = characters.peek().map(|&(_, next)| next);
            let continues = character.is_alphanumeric()
                || character == '_'
                || (character == '.' && next.is_some_and(|next| next.is_ascii_digit()))
                || (matches!(character, '+' | '-')
                    && !is_hex
                    && matches!(previous, Some('e' | 'E')));
            if !continues {
                end = index;
                break;
            }
            previous = Some(character);
        }
        self.advance(end);
    }

    /// Move past a word, highlighting it if it is a keyword, a boolean, or a function name.
    fn lex_word(&mut self) {
        let start = self.offset;
        let word = self
            .rest()
            .split(|character: char| !is_word_character(character))
            .next()
            .unwrap_or_default();
        self.advance(word.len());
        let syntax = if KEYWORDS.contains(&word) {
            Syntax::Keyword
        } else if BOOLEANS.contains(&word) {
            Syntax::Constant
        } else if self.rest().trim_start_matches([' ', '\t']).starts_with('(') {
            Syntax::Function
        } else {
            return;
        };
        self.mark(start, syntax);
    }
}
