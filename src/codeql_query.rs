//! Running a CodeQL query against the current database (#578), as VS
//! Code's "CodeQL: Run Query on Selected Database" does: `codeql` argument
//! lists and the query history, so the process itself stays with the
//! caller.
//!
//! A query whose metadata says `@kind problem` or `@kind path-problem`
//! produces alerts, so it runs through `database analyze` into SARIF and
//! opens in the SARIF viewer. Any other query produces a table, which runs
//! through `query run` and is decoded to CSV.

use std::path::{Path, PathBuf};

/// What a query's results look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Alerts, read as SARIF.
    Sarif,
    /// A result table, read as CSV.
    Table,
}

/// The `@kind` in a query's leading QLDoc comment, if it has one.
pub fn query_kind(source: &str) -> Option<String> {
    let doc = source.trim_start().strip_prefix("/**")?;
    let doc = &doc[..doc.find("*/")?];
    let after = &doc[doc.find("@kind")? + "@kind".len()..];
    after
        .split_whitespace()
        .next()
        .filter(|k| !k.starts_with('*') && !k.starts_with('@'))
        .map(str::to_string)
}

/// How a query with this source is run and read.
pub fn output_for(source: &str) -> Output {
    match query_kind(source).as_deref() {
        Some("problem" | "path-problem") => Output::Sarif,
        _ => Output::Table,
    }
}

fn path(p: &Path) -> String {
    p.display().to_string()
}

/// `codeql` arguments running `query` on `db` into SARIF at `out`.
/// `--rerun` because a history entry run again means run again, not
/// "reuse the cached answer".
pub fn analyze_args(query: &Path, db: &Path, out: &Path) -> Vec<String> {
    vec![
        String::from("database"),
        String::from("analyze"),
        path(db),
        path(query),
        String::from("--format=sarif-latest"),
        format!("--output={}", path(out)),
        String::from("--rerun"),
    ]
}

/// `codeql` arguments running `query` on `db` into a BQRS file.
pub fn run_args(query: &Path, db: &Path, bqrs: &Path) -> Vec<String> {
    vec![
        String::from("query"),
        String::from("run"),
        format!("--database={}", path(db)),
        format!("--output={}", path(bqrs)),
        path(query),
    ]
}

/// `codeql` arguments decoding a BQRS file to CSV at `out`.
pub fn decode_args(bqrs: &Path, out: &Path) -> Vec<String> {
    vec![
        String::from("bqrs"),
        String::from("decode"),
        String::from("--format=csv"),
        format!("--output={}", path(out)),
        path(bqrs),
    ]
}

/// The queries in one CodeQL pack, as the side bar's Queries section groups
/// them (#578).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPack {
    /// The pack's `name:`, its folder's name when it has none, or
    /// [`NO_PACK`] for queries outside any pack.
    pub name: String,
    /// The extractor id (`python`, `cpp`, …) the pack targets, when it says.
    pub language: Option<String>,
    /// The pack's folder; the workspace root for [`NO_PACK`]. Query rows are
    /// shown relative to it.
    pub dir: PathBuf,
    /// Its `.ql` files, sorted.
    pub queries: Vec<PathBuf>,
}

/// The group for queries with no `qlpack.yml` above them.
pub const NO_PACK: &str = "(no pack)";

/// Files [`discover`] looks at before it stops, so a huge tree cannot stall
/// opening the side bar.
pub const DISCOVER_CAP: usize = 50_000;

fn is_pack_file(name: &std::ffi::OsStr) -> bool {
    name == "qlpack.yml" || name == "codeql-pack.yml"
}

/// A pack file's top-level `name:`, unquoted. A line scan, not YAML: the
/// key is a plain scalar in every pack file the CLI writes.
pub fn pack_name(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let v = l.strip_prefix("name:")?.trim();
        let v = v.trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// The language a pack file targets: its `extractor:`, else the `<lang>`
/// of a `codeql/<lang>-all` dependency.
pub fn pack_language(text: &str) -> Option<String> {
    let extractor = text.lines().find_map(|l| {
        let v = l.strip_prefix("extractor:")?.trim();
        let v = v.trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty()).then(|| v.to_string())
    });
    extractor.or_else(|| {
        text.lines().find_map(|l| {
            let rest = &l[l.find("codeql/")? + "codeql/".len()..];
            rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .next()?
                .strip_suffix("-all")
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    })
}

/// Every `.ql` under `root`, each grouped under its nearest ancestor pack
/// file (#578). Ignored files and noise folders are skipped, as the file
/// finder skips them. Packs sort by name then folder, with [`NO_PACK`] last.
pub fn discover(root: &Path) -> Vec<QueryPack> {
    use std::collections::BTreeMap;
    let mut packs: BTreeMap<PathBuf, (String, Option<String>)> = BTreeMap::new();
    let mut queries = Vec::new();
    let mut seen = 0usize;
    for entry in ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .require_git(false)
        .hidden(false)
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_some_and(|t| t.is_dir())
                || !crate::widgets::file_finder::is_noise_dir(e.file_name())
        })
        .build()
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        seen += 1;
        if seen > DISCOVER_CAP {
            break;
        }
        let path = entry.into_path();
        if path.file_name().is_some_and(is_pack_file) {
            let Some(dir) = path.parent() else { continue };
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let name = pack_name(&text).unwrap_or_else(|| {
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            // Both files in one folder: the first read wins, as either names
            // the same pack.
            packs
                .entry(dir.to_path_buf())
                .or_insert((name, pack_language(&text)));
        } else if path.extension().is_some_and(|e| e == "ql") {
            queries.push(path);
        }
    }
    let mut grouped: BTreeMap<Option<PathBuf>, Vec<PathBuf>> = BTreeMap::new();
    for q in queries {
        let pack = q
            .ancestors()
            .skip(1)
            .take_while(|d| d.starts_with(root))
            .find(|d| packs.contains_key(*d))
            .map(Path::to_path_buf);
        grouped.entry(pack).or_default().push(q);
    }
    let mut out: Vec<QueryPack> = grouped
        .into_iter()
        .map(|(dir, mut queries)| {
            queries.sort();
            match dir {
                Some(dir) => {
                    let (name, language) = packs[&dir].clone();
                    QueryPack {
                        name,
                        language,
                        dir,
                        queries,
                    }
                }
                None => QueryPack {
                    name: NO_PACK.to_string(),
                    language: None,
                    dir: root.to_path_buf(),
                    queries,
                },
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.name == NO_PACK, &a.name, &a.dir).cmp(&(b.name == NO_PACK, &b.name, &b.dir))
    });
    out
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RunStatus {
    Running,
    Succeeded,
    /// The first line of what `codeql` said.
    Failed(String),
}

/// One query history entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub query: PathBuf,
    pub database: String,
    /// Seconds since the Unix epoch.
    pub started: u64,
    pub seconds: u64,
    pub status: RunStatus,
    /// The SARIF or CSV the run wrote.
    pub output: PathBuf,
}

impl HistoryEntry {
    /// A history line: "✓ query.ql · db · 12s", "✗ … · failed: why",
    /// "… running".
    pub fn label(&self) -> String {
        let name = self
            .query
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match &self.status {
            RunStatus::Succeeded => format!(
                "\u{2713} {name} \u{b7} {} \u{b7} {}s",
                self.database, self.seconds
            ),
            RunStatus::Failed(why) => format!(
                "\u{2717} {name} \u{b7} {} \u{b7} failed: {why}",
                self.database
            ),
            RunStatus::Running => {
                format!("\u{2026} {name} \u{b7} {} \u{b7} running", self.database)
            }
        }
    }
}

/// The query history, newest first, capped at [`History::CAP`] entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct History {
    #[serde(default)]
    pub entries: Vec<HistoryEntry>,
}

impl History {
    pub const CAP: usize = 100;

    pub fn load(path: &Path) -> History {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Record a new run at the top, dropping the oldest past the cap.
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(Self::CAP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBLEM: &str = "/**\n * @name SQL injection\n * @kind path-problem\n * @id rust/sql\n */\nimport rust\nselect 1";

    #[test]
    fn the_kind_comes_from_the_leading_qldoc() {
        assert_eq!(query_kind(PROBLEM).as_deref(), Some("path-problem"));
        assert_eq!(
            query_kind("/** @kind problem */ select 1").as_deref(),
            Some("problem")
        );
        assert_eq!(query_kind("import rust\nselect 1"), None);
        // A @kind after the query body starts is not metadata.
        assert_eq!(query_kind("select 1\n/** @kind problem */"), None);
    }

    #[test]
    fn problems_read_as_sarif_and_everything_else_as_a_table() {
        assert_eq!(output_for(PROBLEM), Output::Sarif);
        assert_eq!(output_for("/** @kind problem */ select 1"), Output::Sarif);
        assert_eq!(output_for("/** @kind graph */ select 1"), Output::Table);
        assert_eq!(output_for("select 1"), Output::Table);
    }

    #[test]
    fn argument_lists_name_the_database_query_and_output() {
        let (q, db) = (Path::new("/w/q.ql"), Path::new("/dbs/app"));
        assert_eq!(
            analyze_args(q, db, Path::new("/out/r.sarif")),
            vec![
                "database",
                "analyze",
                "/dbs/app",
                "/w/q.ql",
                "--format=sarif-latest",
                "--output=/out/r.sarif",
                "--rerun"
            ]
        );
        assert_eq!(
            run_args(q, db, Path::new("/out/r.bqrs")),
            vec![
                "query",
                "run",
                "--database=/dbs/app",
                "--output=/out/r.bqrs",
                "/w/q.ql"
            ]
        );
        assert_eq!(
            decode_args(Path::new("/out/r.bqrs"), Path::new("/out/r.csv")),
            vec![
                "bqrs",
                "decode",
                "--format=csv",
                "--output=/out/r.csv",
                "/out/r.bqrs"
            ]
        );
    }

    #[test]
    fn pack_files_give_a_name_and_a_language() {
        let text = "name: \"acme/py-queries\"\nversion: 0.0.1\ndependencies:\n  codeql/python-all: \"*\"\n";
        assert_eq!(pack_name(text).as_deref(), Some("acme/py-queries"));
        assert_eq!(pack_language(text).as_deref(), Some("python"));
        let lib = "name: acme/lib\nextractor: cpp\nlibraryPathDependencies: codeql/go-all\n";
        assert_eq!(pack_language(lib).as_deref(), Some("cpp"), "extractor wins");
        // An indented `name:` belongs to something else.
        assert_eq!(pack_name("deps:\n  name: x\n"), None);
        assert_eq!(pack_language("name: x\n"), None);
        assert_eq!(
            pack_language("dependencies:\n  - codeql/javascript-queries\n"),
            None,
            "only the -all library names a language"
        );
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn discover_groups_queries_under_their_nearest_pack() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(r, "outer/qlpack.yml", "name: acme/outer\nextractor: go\n");
        write(r, "outer/a.ql", "select 1");
        write(
            r,
            "outer/inner/codeql-pack.yml",
            "name: acme/inner\ndependencies:\n  codeql/rust-all: '*'\n",
        );
        write(r, "outer/inner/deep/b.ql", "select 1");
        write(r, "unnamed/qlpack.yml", "version: 1.0.0\n");
        write(r, "unnamed/c.ql", "select 1");
        write(r, "loose.ql", "select 1");
        write(r, "scratch/d.ql", "select 1");
        write(r, "lib.qll", "predicate p() { any() }");
        // Noise folders are never searched.
        write(r, "node_modules/pkg/e.ql", "select 1");
        write(r, "target/f.ql", "select 1");
        let packs = discover(r);
        let summary: Vec<(&str, Option<&str>, Vec<String>)> = packs
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.language.as_deref(),
                    p.queries
                        .iter()
                        .map(|q| q.strip_prefix(&p.dir).unwrap().display().to_string())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("acme/inner", Some("rust"), vec![String::from("deep/b.ql")]),
                ("acme/outer", Some("go"), vec![String::from("a.ql")]),
                ("unnamed", None, vec![String::from("c.ql")]),
                (
                    NO_PACK,
                    None,
                    vec![String::from("loose.ql"), String::from("scratch/d.ql")]
                ),
            ]
        );
        assert_eq!(packs[3].dir, r);
        assert!(discover(&r.join("missing")).is_empty());
    }

    fn entry(status: RunStatus) -> HistoryEntry {
        HistoryEntry {
            query: PathBuf::from("/w/sql.ql"),
            database: String::from("app"),
            started: 1,
            seconds: 12,
            status,
            output: PathBuf::from("/out/r.sarif"),
        }
    }

    #[test]
    fn history_labels_say_how_each_run_went() {
        assert_eq!(
            entry(RunStatus::Succeeded).label(),
            "\u{2713} sql.ql \u{b7} app \u{b7} 12s"
        );
        assert_eq!(
            entry(RunStatus::Failed(String::from("bad query"))).label(),
            "\u{2717} sql.ql \u{b7} app \u{b7} failed: bad query"
        );
        assert_eq!(
            entry(RunStatus::Running).label(),
            "\u{2026} sql.ql \u{b7} app \u{b7} running"
        );
    }

    #[test]
    fn history_is_newest_first_capped_and_survives_a_round_trip() {
        let mut h = History::default();
        for i in 0..(History::CAP as u64 + 5) {
            let mut e = entry(RunStatus::Succeeded);
            e.started = i;
            h.push(e);
        }
        assert_eq!(h.entries.len(), History::CAP);
        assert_eq!(h.entries[0].started, History::CAP as u64 + 4);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h/history.json");
        h.save(&path).unwrap();
        assert_eq!(History::load(&path), h);
        assert_eq!(
            History::load(&dir.path().join("missing.json")),
            History::default()
        );
    }
}
