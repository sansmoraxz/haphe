/// Indentation-aware text builder for WIT output (two-space indent).
pub(crate) struct Printer {
    buf: String,
    indent: usize,
}

impl Printer {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            indent: 0,
        }
    }

    pub fn line(&mut self, s: &str) {
        if s.is_empty() {
            self.buf.push('\n');
            return;
        }
        for _ in 0..self.indent {
            self.buf.push_str("  ");
        }
        self.buf.push_str(s);
        self.buf.push('\n');
    }

    /// Emits `s {` and increases the indent.
    pub fn open(&mut self, s: &str) {
        self.line(&format!("{s} {{"));
        self.indent += 1;
    }

    /// Decreases the indent and emits `}`.
    pub fn close(&mut self) {
        self.indent -= 1;
        self.line("}");
    }

    /// Emits a doc comment as `///` lines, one per source line.
    pub fn doc(&mut self, doc: Option<&str>) {
        if let Some(doc) = doc {
            for line in doc.lines() {
                let line = line.trim();
                if line.is_empty() {
                    self.line("///");
                } else {
                    self.line(&format!("/// {line}"));
                }
            }
        }
    }

    pub fn finish(self) -> String {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_and_docs() {
        let mut p = Printer::new();
        p.doc(Some("An interface.\n\nMore detail."));
        p.open("interface foo");
        p.line("x: func() -> f64;");
        p.close();
        assert_eq!(
            p.finish(),
            "/// An interface.\n///\n/// More detail.\ninterface foo {\n  x: func() -> f64;\n}\n"
        );
    }
}
