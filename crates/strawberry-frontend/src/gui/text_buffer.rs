use std::ops::Range;

/// Editable text with an insertion cursor, kept as a byte offset on a character boundary.
#[derive(Debug, Clone, Default)]
pub struct TextBuffer {
    text: String,
    cursor: usize,
}

impl TextBuffer {
    /// Create a buffer holding `text` with the cursor at its start.
    pub const fn new(text: String) -> Self {
        Self { text, cursor: 0 }
    }

    /// Return the buffer's text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Return the cursor's byte offset.
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Return the byte offset of the character boundary before `offset`, or `offset` at the start.
    pub fn find_boundary_before(&self, offset: usize) -> usize {
        self.text
            .get(..offset)
            .unwrap_or_default()
            .chars()
            .next_back()
            .map_or(offset, |c| offset.saturating_sub(c.len_utf8()))
    }

    /// Return the byte offset of the character boundary after `offset`, or `offset` at the end.
    pub fn find_boundary_after(&self, offset: usize) -> usize {
        self.text
            .get(offset..)
            .unwrap_or_default()
            .chars()
            .next()
            .map_or(offset, |c| offset.saturating_add(c.len_utf8()))
    }

    /// Move the cursor to a byte offset, clamped to the text.
    pub fn move_cursor(&mut self, offset: usize) {
        self.cursor = offset.min(self.text.len());
    }

    /// Replace a byte range with `text` and place the cursor after the inserted text.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let start = range.start.min(self.text.len());
        let end = range.end.clamp(start, self.text.len());
        self.text.replace_range(start..end, text);
        self.cursor = start.saturating_add(text.len());
    }

    /// Insert text at the cursor.
    pub fn insert(&mut self, text: &str) {
        self.replace_range(self.cursor..self.cursor, text);
    }

    /// Delete the text between the cursor and `offset`; return whether anything was deleted.
    pub fn delete_to(&mut self, offset: usize) -> bool {
        if offset == self.cursor {
            return false;
        }
        self.replace_range(self.cursor.min(offset)..self.cursor.max(offset), "");
        true
    }
}
