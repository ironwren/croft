//! A small, dependency-free Markdown linter (VS Code has no bundled
//! Markdown linter of its own; `davidanson.vscode-markdownlint` is the
//! popular third-party fill — see `src/vscode_extensions.rs`). Runs
//! entirely on the buffer text, no LSP server required, so it works the
//! moment a `.md` file is opened.
//!
//! Deliberately covers a handful of the most common `markdownlint` rules
//! rather than the whole rule set, chosen for a very low false-positive
//! rate: multiple top-level headings, no space after a heading's `#`,
//! trailing whitespace, and runs of blank lines.

use crate::lsp::manager::{Diagnostic, DiagnosticSeverity};

/// Lints Markdown `text` and returns one diagnostic per violation, in
/// document order. `start_line`/`end_line` are 0-based, matching the LSP
/// convention `crate::lsp::manager::Diagnostic` already uses everywhere
/// else, so these diagnostics splice into the same per-line decode path
/// (`Editor::apply_diagnostics`) as a real language server's.
pub fn lint(text: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut seen_h1 = false;
    let mut blank_run_start: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed_start = line.trim_start();
        let heading_hashes = trimmed_start.chars().take_while(|&c| c == '#').count();

        // MD025: more than one top-level (`# `) heading in a document.
        if heading_hashes == 1 && trimmed_start[1..].starts_with(' ') {
            if seen_h1 {
                out.push(diag(
                    i,
                    0,
                    line.len(),
                    DiagnosticSeverity::Warning,
                    "MD025: multiple top-level headings in the same document",
                ));
            }
            seen_h1 = true;
        }

        // MD018: an ATX heading's `#`s must be followed by a space (`#Heading`
        // is not a heading in CommonMark, a common authoring slip).
        if (1..=6).contains(&heading_hashes) {
            let rest = &trimmed_start[heading_hashes..];
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('#') {
                let col = line.len() - trimmed_start.len();
                out.push(diag(
                    i,
                    col,
                    col + heading_hashes,
                    DiagnosticSeverity::Warning,
                    "MD018: no space after '#' in the heading marker",
                ));
            }
        }

        // MD009: trailing whitespace, excluding Markdown's two-space hard
        // line break (exactly two trailing spaces) and blank lines.
        let trimmed_end = line.trim_end_matches(' ');
        let trailing = line.len() - trimmed_end.len();
        if trailing > 0 && trailing != 2 && !trimmed_end.is_empty() {
            out.push(diag(
                i,
                trimmed_end.len(),
                line.len(),
                DiagnosticSeverity::Hint,
                "MD009: trailing whitespace",
            ));
        }

        // MD012: more than one consecutive blank line.
        if line.trim().is_empty() {
            blank_run_start.get_or_insert(i);
        } else {
            if let Some(start) = blank_run_start
                && i - start > 1
            {
                out.push(diag(
                    start + 1,
                    0,
                    0,
                    DiagnosticSeverity::Hint,
                    "MD012: multiple consecutive blank lines",
                ));
            }
            blank_run_start = None;
        }
    }
    if let Some(start) = blank_run_start
        && lines.len().saturating_sub(start) > 1
    {
        out.push(diag(
            start + 1,
            0,
            0,
            DiagnosticSeverity::Hint,
            "MD012: multiple consecutive blank lines",
        ));
    }

    out
}

fn diag(
    line: usize,
    start_char: usize,
    end_char: usize,
    severity: DiagnosticSeverity,
    message: &str,
) -> Diagnostic {
    Diagnostic {
        start_line: line as u32,
        start_char: start_char as u32,
        end_line: line as u32,
        end_char: end_char as u32,
        severity,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_a_second_top_level_heading() {
        let text = "# Title\n\nSome text.\n\n# Another Title\n";
        let diags = lint(text);
        assert!(
            diags.iter().any(|d| d.message.contains("MD025")),
            "found: {diags:?}"
        );
    }

    #[test]
    fn single_h1_is_not_flagged() {
        let text = "# Title\n\nSome text.\n\n## Subheading\n";
        let diags = lint(text);
        assert!(!diags.iter().any(|d| d.message.contains("MD025")));
    }

    #[test]
    fn flags_missing_space_after_hash() {
        let text = "#Title with no space\n";
        let diags = lint(text);
        assert!(diags.iter().any(|d| d.message.contains("MD018")));
    }

    #[test]
    fn flags_trailing_whitespace_but_not_hard_break() {
        let text = "trailing spaces here   \nhard break here  \nclean line\n";
        let diags = lint(text);
        let md009: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("MD009"))
            .collect();
        assert_eq!(md009.len(), 1, "found: {diags:?}");
        assert_eq!(md009[0].start_line, 0);
    }

    #[test]
    fn flags_multiple_consecutive_blank_lines() {
        let text = "one\n\n\n\ntwo\n";
        let diags = lint(text);
        assert!(
            diags.iter().any(|d| d.message.contains("MD012")),
            "found: {diags:?}"
        );
    }

    #[test]
    fn single_blank_line_is_not_flagged() {
        let text = "one\n\ntwo\n";
        let diags = lint(text);
        assert!(!diags.iter().any(|d| d.message.contains("MD012")));
    }

    #[test]
    fn clean_document_has_no_diagnostics() {
        let text = "# Title\n\nA paragraph with *emphasis* and a [link](https://example.com).\n\n## Section\n\nMore text.\n";
        assert!(lint(text).is_empty(), "found: {:?}", lint(text));
    }
}
