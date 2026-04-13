//! Indentation-aware source writer.
//!
//! Thin layer on top of a `String` that tracks indent depth so callers can
//! focus on emitting lines without manually managing spaces.

pub struct Emitter {
    buf: String,
    indent: usize,
    at_line_start: bool,
}

impl Emitter {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            indent: 0,
            at_line_start: true,
        }
    }

    pub fn indent(&mut self) {
        self.indent += 1;
    }

    pub fn dedent(&mut self) {
        assert!(self.indent > 0, "dedent below zero");
        self.indent -= 1;
    }

    /// Write a chunk of text (may contain no newlines).
    pub fn write(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.at_line_start {
            for _ in 0..self.indent {
                self.buf.push_str("    ");
            }
            self.at_line_start = false;
        }
        self.buf.push_str(s);
    }

    /// Write a full line: text + newline.
    pub fn writeln(&mut self, s: &str) {
        self.write(s);
        self.newline();
    }

    /// End the current line and mark the next write as needing indent.
    pub fn newline(&mut self) {
        self.buf.push('\n');
        self.at_line_start = true;
    }

    /// Emit a blank line regardless of current indent.
    pub fn blank_line(&mut self) {
        self.buf.push('\n');
        self.at_line_start = true;
    }

    pub fn finish(self) -> String {
        self.buf
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}
