//! Columnar table renderer with TTY detection, NO_COLOR support,
//! Unicode/ASCII box character switching.

use std::io::IsTerminal;

/// Determines if the output should use colors.
///
/// Colors are suppressed when:
/// - stdout is not a TTY (piped output)
/// - `NO_COLOR` environment variable is set (any value)
pub fn should_use_color() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Determines if the output should use Unicode box-drawing characters.
///
/// Unicode is used only when stdout is a TTY. Piped output uses ASCII.
pub fn should_use_unicode() -> bool {
    std::io::stdout().is_terminal() && std::env::var("NO_COLOR").is_err()
}

/// ANSI color codes.
pub mod ansi {
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const RED: &str = "\x1b[31m";
    pub const BOLD: &str = "\x1b[1m";
    pub const RESET: &str = "\x1b[0m";
    pub const DIM: &str = "\x1b[2m";
}

/// A simple columnar table renderer.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    use_color: bool,
    use_unicode: bool,
}

impl Table {
    pub fn new(headers: Vec<String>) -> Self {
        Self {
            headers,
            rows: Vec::new(),
            use_color: should_use_color(),
            use_unicode: should_use_unicode(),
        }
    }

    /// Override color settings (for testing).
    pub fn with_color(mut self, use_color: bool) -> Self {
        self.use_color = use_color;
        self
    }

    /// Override unicode settings (for testing).
    pub fn with_unicode(mut self, use_unicode: bool) -> Self {
        self.use_unicode = use_unicode;
        self
    }

    pub fn add_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    /// Render the table to a string.
    pub fn render(&self) -> String {
        let num_cols = self.headers.len();

        // Calculate column widths (max of header and all row values)
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    // Strip ANSI codes for width calculation
                    let plain = strip_ansi(cell);
                    widths[i] = widths[i].max(plain.len());
                }
            }
        }

        let mut output = String::new();

        // Separator characters
        let (h_bar, v_bar, cross) = if self.use_unicode {
            ("\u{2500}", "\u{2502}", "\u{253C}")
        } else {
            ("-", "|", "+")
        };

        // Header row
        let header_line: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let padded = format!("{:<width$}", h, width = widths[i]);
                if self.use_color {
                    format!("{}{}{}", ansi::BOLD, padded, ansi::RESET)
                } else {
                    padded
                }
            })
            .collect();
        output.push_str(&format!(
            " {} \n",
            header_line.join(&format!(" {} ", v_bar))
        ));

        // Separator line
        let sep_parts: Vec<String> = widths
            .iter()
            .map(|w| h_bar.repeat(w + 2))
            .collect();
        output.push_str(&sep_parts.join(cross));
        output.push('\n');

        // Data rows
        for row in &self.rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    if i < num_cols {
                        let plain_len = strip_ansi(cell).len();
                        let padding = widths[i].saturating_sub(plain_len);
                        format!("{}{}", cell, " ".repeat(padding))
                    } else {
                        cell.clone()
                    }
                })
                .collect();
            output.push_str(&format!(
                " {} \n",
                cells.join(&format!(" {} ", v_bar))
            ));
        }

        output
    }
}

/// Strip ANSI escape codes from a string for width calculation.
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_removes_color_codes() {
        let colored = format!("{}hello{}", ansi::GREEN, ansi::RESET);
        assert_eq!(strip_ansi(&colored), "hello");
    }

    #[test]
    fn test_strip_ansi_preserves_plain_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn test_table_ascii_rendering() {
        let mut table = Table::new(vec!["NAME".into(), "STATUS".into()])
            .with_color(false)
            .with_unicode(false);
        table.add_row(vec!["agent-1".into(), "running".into()]);
        let rendered = table.render();
        assert!(rendered.contains("NAME"));
        assert!(rendered.contains("STATUS"));
        assert!(rendered.contains("agent-1"));
        assert!(rendered.contains("running"));
        // ASCII separator
        assert!(rendered.contains("-"));
        assert!(rendered.contains("|"));
    }

    #[test]
    fn test_table_unicode_rendering() {
        let mut table = Table::new(vec!["NAME".into(), "STATUS".into()])
            .with_color(false)
            .with_unicode(true);
        table.add_row(vec!["agent-1".into(), "running".into()]);
        let rendered = table.render();
        assert!(rendered.contains("\u{2502}")); // │
        assert!(rendered.contains("\u{2500}")); // ─
    }
}
