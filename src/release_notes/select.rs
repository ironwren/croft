//! Which release notes a build bakes into its welcome card.
//!
//! `build.rs` includes this file by path and the crate's tests compile it as
//! `release_notes::select`, so the build and the tests that check it read one
//! definition. It uses `std` alone: a build script has no dependencies.

use std::path::{Path, PathBuf};

/// The README that explains `unreleased/` and keeps the directory in git
/// while no note is pending. It is never a note.
pub const UNRELEASED_README: &str = "README.md";

/// What a build bakes in.
pub struct Baked {
    /// The notes, in the form `release_notes::parse` reads.
    pub text: String,
    /// True when the notes are pending fragments: the build carries changes
    /// that no release has yet.
    pub unreleased: bool,
}

/// The filter `release_notes::parse` applies: a line that is neither blank
/// nor a `#` heading. A file holding only a `# 0.1.808` heading is non-blank
/// and still paints an empty card.
pub fn has_highlight(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#'))
}

/// The pending fragments in `dir/unreleased`, in file-name order, the README
/// left out.
pub fn fragments(dir: &Path) -> Vec<PathBuf> {
    let _ = dir;
    Vec::new()
}

/// The notes a build at `version` carries, read from `dir`
/// (`src/release_notes`).
pub fn baked(dir: &Path, version: &str) -> Result<Baked, String> {
    let _ = fragments(dir);
    let path = dir.join(format!("{version}.md"));
    match std::fs::read_to_string(&path) {
        Ok(text) if has_highlight(&text) => Ok(Baked {
            text,
            unreleased: false,
        }),
        Ok(_) => Err(format!(
            "{} carries no highlights (blank, or nothing but headings). Write \
             one per line, each prefixed `feature:` or `fix:`.",
            path.display()
        )),
        Err(e) => Err(format!(
            "{} could not be read ({e}). Every version needs a notes file, so \
             the welcome panel always describes the binary it is in.",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `src/release_notes` with the 0.2.11 release and `unreleased/`
    /// holding only its README.
    fn notes_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("0.2.11.md"), "fix: The last release.\n").unwrap();
        std::fs::create_dir(dir.path().join("unreleased")).unwrap();
        std::fs::write(
            dir.path().join("unreleased").join(UNRELEASED_README),
            "fix: Not a note.\n",
        )
        .unwrap();
        dir
    }

    fn pend(dir: &tempfile::TempDir, name: &str, text: &str) {
        std::fs::write(dir.path().join("unreleased").join(name), text).unwrap();
    }

    /// A build carrying merged changes that no release has yet describes
    /// those changes, not the last release's: the fragments, in file-name
    /// order, each ending in a newline, the way `scripts/release.py cut`
    /// later writes them into the next version's notes.
    #[test]
    fn pending_fragments_are_baked_in_name_order_and_marked_unreleased() {
        let dir = notes_dir();
        pend(&dir, "862-hot-exit.md", "feature: Hot exit.");
        pend(&dir, "847-plot.md", "fix: Plot lines join.\nfix: Labels.\n");
        let baked = baked(dir.path(), "0.2.11").unwrap();
        assert_eq!(
            baked.text,
            "fix: Plot lines join.\nfix: Labels.\nfeature: Hot exit.\n"
        );
        assert!(baked.unreleased, "the build carries unreleased changes");
    }

    /// A pending fragment wins even where no file for the version exists, as
    /// in a build of a release-less checkout of a fork.
    #[test]
    fn pending_fragments_need_no_notes_file_for_the_version() {
        let dir = notes_dir();
        pend(&dir, "847-plot.md", "fix: Plot.\n");
        let baked = baked(dir.path(), "0.9.0").unwrap();
        assert_eq!(baked.text, "fix: Plot.\n");
        assert!(baked.unreleased);
    }

    /// A fragment that says nothing is an error even where the version has
    /// notes of its own: the build would otherwise describe the last release
    /// while carrying a change the card is silent about.
    #[test]
    fn a_pending_fragment_without_a_highlight_is_an_error() {
        let dir = notes_dir();
        pend(&dir, "847-plot.md", "fix: Plot.\n");
        pend(&dir, "848-edit.md", "# Edit\n\n");
        let err = baked(dir.path(), "0.2.11").err().expect("an error");
        assert!(err.contains("848-edit.md"), "{err}");
    }

    /// Right after a release no fragment is pending, and the build describes
    /// its own version. The README is not a fragment.
    #[test]
    fn with_nothing_pending_the_versions_own_notes_are_baked() {
        let dir = notes_dir();
        let baked = baked(dir.path(), "0.2.11").unwrap();
        assert_eq!(baked.text, "fix: The last release.\n");
        assert!(!baked.unreleased);
    }

    /// Only `.md` files are notes, and never a dot-file: an editor's backup
    /// or lock file, or a stray `.txt`, in the directory is not baked into
    /// the card. `scripts/release.py` skips the same files.
    #[test]
    fn only_markdown_files_are_fragments() {
        let dir = notes_dir();
        pend(&dir, "847-plot.md~", "fix: A backup.\n");
        pend(&dir, ".#847-plot.md", "fix: A lock file.\n");
        pend(&dir, "notes.txt", "fix: A text file.\n");
        let baked = baked(dir.path(), "0.2.11").unwrap();
        assert_eq!(baked.text, "fix: The last release.\n");
        assert!(!baked.unreleased);
    }

    /// The guarantee the single notes file gave: a binary always describes
    /// itself, so a build with neither a pending note nor notes for its
    /// version does not build.
    #[test]
    fn with_nothing_pending_and_no_notes_for_the_version_it_is_an_error() {
        let dir = notes_dir();
        let err = baked(dir.path(), "0.2.12").err().expect("an error");
        assert!(err.contains("0.2.12.md"), "{err}");
    }

    #[test]
    fn a_versions_notes_without_a_highlight_are_an_error() {
        let dir = notes_dir();
        std::fs::write(dir.path().join("0.2.11.md"), "# 0.2.11\n").unwrap();
        let err = baked(dir.path(), "0.2.11").err().expect("an error");
        assert!(err.contains("0.2.11.md"), "{err}");
    }
}
