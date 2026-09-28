//! Submitting a CodeQL variant analysis (#578): the query goes to GitHub as
//! a bundled query pack, posted to the controller repository, which runs it
//! on Actions against the selected repositories, list or owner.
//!
//! The pack is built as VS Code's CodeQL extension builds it. A query inside
//! a pack travels with that pack's own sources and dependencies; one outside
//! any pack gets a generated pack depending on `codeql/<language>-all`.
//! Either way the pack's `defaultSuite` names the one query, so the run
//! evaluates that query and nothing else in the pack.

use std::path::{Path, PathBuf};

use crate::codeql_variant::{Selection, VariantConfig};

/// What a run targets, in the request's own terms. A user-defined list is
/// sent as its repositories: the API's `repository_lists` names GitHub's
/// lists (`top_100`), not the user's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    pub repositories: Vec<String>,
    pub owners: Vec<String>,
}

impl Targets {
    /// How the status line names what the run went to.
    pub fn describe(&self) -> String {
        match (self.repositories.as_slice(), self.owners.as_slice()) {
            ([one], []) => one.clone(),
            (many, []) => format!("{} repositories", many.len()),
            ([], [owner]) => format!("every repository of {owner}"),
            _ => String::from("the selected owners"),
        }
    }
}

/// The targets of `config`'s selection, or why there is nothing to run on.
pub fn targets(config: &VariantConfig) -> Result<Targets, String> {
    match &config.selected {
        None => Err(String::from(
            "Select a repository list, repository or owner under Variant Analysis Repositories",
        )),
        Some(Selection::List { list }) => {
            let repos = config
                .lists
                .iter()
                .find(|l| &l.name == list)
                .map(|l| l.repos.clone())
                .ok_or_else(|| format!("The list {list} is no longer there"))?;
            if repos.is_empty() {
                return Err(format!("The list {list} has no repositories"));
            }
            Ok(Targets {
                repositories: repos,
                owners: Vec::new(),
            })
        }
        Some(Selection::Repo { nwo, .. }) => Ok(Targets {
            repositories: vec![nwo.clone()],
            owners: Vec::new(),
        }),
        Some(Selection::Owner { owner }) => Ok(Targets {
            repositories: Vec::new(),
            owners: vec![owner.clone()],
        }),
    }
}

fn pack_file_in(dir: &Path) -> Option<PathBuf> {
    ["qlpack.yml", "codeql-pack.yml"]
        .into_iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

/// The folder of the nearest pack holding `query`, and its pack file.
pub fn enclosing_pack(query: &Path) -> Option<(PathBuf, PathBuf)> {
    query
        .ancestors()
        .skip(1)
        .find_map(|dir| pack_file_in(dir).map(|file| (dir.to_path_buf(), file)))
}

/// `text`, a pack file, with any top-level `defaultSuite` or
/// `defaultSuiteFile` (and the lines nested under it) replaced by one naming
/// only `query`, a path relative to the pack. A line scan, like the other
/// pack-file readers: top-level keys start in column 0.
pub fn with_default_suite(text: &str, query: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let top_level = !line.starts_with([' ', '\t', '-', '#']) && !line.trim().is_empty();
        if top_level {
            let key = line.split(':').next().unwrap_or("").trim();
            skipping = key == "defaultSuite" || key == "defaultSuiteFile";
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "defaultSuite:\n  - query: {}\n",
        yaml_string(query)
    ));
    out
}

fn yaml_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The pack file of a generated pack running `file` alone, for `language`.
pub fn generated_pack_file(language: &str, file: &str) -> String {
    format!(
        "name: codeql-remote/query\nversion: 0.0.0\ndependencies:\n  codeql/{language}-all: \"*\"\ndefaultSuite:\n  - query: {}\n",
        yaml_string(file)
    )
}

/// Whether a file of a pack travels in the bundle: sources, pack files and
/// the lock file, not databases, results or test output.
fn is_pack_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ql" | "qll" | "yml" | "yaml" | "dbscheme")
    )
}

/// Most files a pack copy takes before refusing, so a query picked from
/// inside a huge checkout cannot fill the disk.
pub const PACK_FILE_CAP: usize = 20_000;

fn copy_pack_sources(from: &Path, to: &Path, copied: &mut usize) -> Result<(), String> {
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        // `.codeql` holds downloaded dependencies and caches; hidden
        // folders never hold pack sources.
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            copy_pack_sources(&path, &to.join(&name), copied)?;
        } else if kind.is_file() && is_pack_source(&path) {
            *copied += 1;
            if *copied > PACK_FILE_CAP {
                return Err(format!(
                    "The query's pack holds over {PACK_FILE_CAP} source files; move the query into a smaller pack"
                ));
            }
            std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
            std::fs::copy(&path, to.join(&name)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Lay out in `dest` (created, and expected empty) the pack that runs
/// `query`: a copy of its own pack's sources with the default suite pointed
/// at it, or a generated pack for `language` when it is in none. Returns
/// the pack's folder.
pub fn prepare_pack(query: &Path, language: &str, dest: &Path) -> Result<PathBuf, String> {
    let pack = dest.join("pack");
    match enclosing_pack(query) {
        Some((dir, file)) => {
            let rel = query
                .strip_prefix(&dir)
                .map_err(|_| format!("{} is outside its pack", query.display()))?;
            copy_pack_sources(&dir, &pack, &mut 0)?;
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let rel = rel.to_string_lossy().replace('\\', "/");
            let name = file.file_name().unwrap_or_default();
            std::fs::write(pack.join(name), with_default_suite(&text, &rel))
                .map_err(|e| format!("{}: {e}", pack.display()))?;
        }
        None => {
            let name = query
                .file_name()
                .ok_or_else(|| format!("{} is not a query file", query.display()))?;
            std::fs::create_dir_all(&pack).map_err(|e| format!("{}: {e}", pack.display()))?;
            std::fs::copy(query, pack.join(name))
                .map_err(|e| format!("{}: {e}", query.display()))?;
            std::fs::write(
                pack.join("qlpack.yml"),
                generated_pack_file(language, &name.to_string_lossy()),
            )
            .map_err(|e| format!("{}: {e}", pack.display()))?;
        }
    }
    Ok(pack)
}

/// `codeql` arguments bundling the pack in `dir` into the archive `out`.
pub fn pack_bundle_args(dir: &Path, out: &Path) -> Vec<String> {
    vec![
        String::from("pack"),
        String::from("bundle"),
        format!("--output={}", out.display()),
        String::from("--"),
        dir.display().to_string(),
    ]
}

/// The body of the submission: the bundled pack, base64 encoded, the
/// query's language, and what it runs on.
pub fn submission_body(language: &str, bundle: &[u8], targets: &Targets) -> serde_json::Value {
    use base64::Engine;
    let mut body = serde_json::json!({
        "language": language,
        "query_pack": base64::engine::general_purpose::STANDARD.encode(bundle),
    });
    if !targets.repositories.is_empty() {
        body["repositories"] = serde_json::json!(targets.repositories);
    }
    if !targets.owners.is_empty() {
        body["repository_owners"] = serde_json::json!(targets.owners);
    }
    body
}

/// `gh` arguments posting the body in `body_file` to `controller`.
pub fn submit_args(controller: &str, body_file: &Path) -> Vec<String> {
    vec![
        String::from("api"),
        String::from("--method"),
        String::from("POST"),
        format!("repos/{controller}/code-scanning/codeql/variant-analyses"),
        String::from("--input"),
        body_file.display().to_string(),
    ]
}

/// What GitHub said about a submitted run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Submitted {
    pub id: u64,
    pub controller: String,
    pub query: PathBuf,
    pub language: String,
    /// The Actions run that does the work, once GitHub has started it.
    #[serde(default)]
    pub workflow_run: Option<u64>,
    /// Repositories GitHub will not run on (no database, private, …).
    #[serde(default)]
    pub skipped: usize,
    pub submitted_at: u64,
}

impl Submitted {
    /// Where to watch the run on GitHub: its Actions run when known, else
    /// the controller's Actions page.
    pub fn url(&self) -> String {
        match self.workflow_run {
            Some(run) => format!("https://github.com/{}/actions/runs/{run}", self.controller),
            None => format!("https://github.com/{}/actions", self.controller),
        }
    }
}

/// The run GitHub's answer to a submission describes.
pub fn parse_submission(
    json: &str,
    controller: &str,
    query: &Path,
    language: &str,
    submitted_at: u64,
) -> Result<Submitted, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("GitHub's answer was not JSON: {e}"))?;
    let id = v
        .get("id")
        .and_then(|i| i.as_u64())
        .ok_or_else(|| String::from("GitHub's answer named no variant analysis"))?;
    // Each kind of skip lists its repositories under `repositories`.
    let skipped = v
        .get("skipped_repositories")
        .and_then(|s| s.as_object())
        .map(|kinds| {
            kinds
                .values()
                .filter_map(|k| k.get("repository_count").and_then(|c| c.as_u64()))
                .sum::<u64>() as usize
        })
        .unwrap_or(0);
    Ok(Submitted {
        id,
        controller: controller.to_string(),
        query: query.to_path_buf(),
        language: language.to_string(),
        workflow_run: v.get("actions_workflow_run_id").and_then(|r| r.as_u64()),
        skipped,
        submitted_at,
    })
}

/// The submitted runs croft remembers, newest last, for following them up.
pub fn load_submitted(path: &Path) -> Vec<Submitted> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn record_submitted(path: &Path, run: Submitted) -> std::io::Result<()> {
    let mut runs = load_submitted(path);
    runs.push(run);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&runs).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_targets_the_selection_and_a_user_list_goes_as_its_repositories() {
        let mut c = VariantConfig::default();
        assert!(targets(&c).is_err(), "nothing selected");
        c.add_list("top").unwrap();
        c.select(crate::codeql_variant::Item::List(0)).unwrap();
        assert!(targets(&c).unwrap_err().contains("no repositories"));
        c.add_repo(Some(0), "a/b").unwrap();
        c.add_repo(Some(0), "c/d").unwrap();
        let t = targets(&c).unwrap();
        assert_eq!(t.repositories, ["a/b", "c/d"]);
        assert_eq!(t.describe(), "2 repositories");
        let body = submission_body("python", b"tgz", &t);
        assert_eq!(body["repositories"], serde_json::json!(["a/b", "c/d"]));
        assert_eq!(body["query_pack"], "dGd6");
        assert!(body.get("repository_lists").is_none());
        assert!(body.get("repository_owners").is_none());

        c.select(crate::codeql_variant::Item::Repo(Some(0), 1))
            .unwrap();
        assert_eq!(targets(&c).unwrap().describe(), "c/d");
        c.add_owner("octo").unwrap();
        c.select(crate::codeql_variant::Item::Owner(0)).unwrap();
        let t = targets(&c).unwrap();
        assert_eq!(t.describe(), "every repository of octo");
        let body = submission_body("go", b"", &t);
        assert_eq!(body["repository_owners"], serde_json::json!(["octo"]));
        assert!(body.get("repositories").is_none());
    }

    #[test]
    fn the_default_suite_is_replaced_by_the_one_query() {
        let text = "name: me/q\ndefaultSuiteFile: suites/all.qls\ndependencies:\n  codeql/python-all: \"*\"\ndefaultSuite:\n  - queries: .\n  - exclude:\n      kind: diagnostic\nversion: 1.0.0\n";
        let out = with_default_suite(text, "src/Find Me.ql");
        assert_eq!(
            out,
            "name: me/q\ndependencies:\n  codeql/python-all: \"*\"\nversion: 1.0.0\ndefaultSuite:\n  - query: \"src/Find Me.ql\"\n"
        );
    }

    #[test]
    fn a_query_in_a_pack_travels_with_its_sources_and_one_outside_gets_a_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("ws/pack");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::create_dir_all(src.join(".codeql/deps")).unwrap();
        std::fs::create_dir_all(src.join("queries")).unwrap();
        std::fs::write(src.join("qlpack.yml"), "name: me/q\nversion: 0.0.1\n").unwrap();
        std::fs::write(src.join("codeql-pack.lock.yml"), "lockVersion: 1.0.0\n").unwrap();
        std::fs::write(src.join("lib/Helpers.qll"), "predicate p() { any() }").unwrap();
        std::fs::write(src.join("queries/Q.ql"), "select 1").unwrap();
        std::fs::write(src.join("queries/notes.md"), "not a source").unwrap();
        std::fs::write(src.join(".codeql/deps/Big.qll"), "cached").unwrap();

        let out = tmp.path().join("out");
        let pack = prepare_pack(&src.join("queries/Q.ql"), "python", &out).unwrap();
        assert!(pack.join("lib/Helpers.qll").is_file());
        assert!(pack.join("queries/Q.ql").is_file());
        assert!(pack.join("codeql-pack.lock.yml").is_file());
        assert!(!pack.join("queries/notes.md").exists());
        assert!(!pack.join(".codeql").exists());
        let file = std::fs::read_to_string(pack.join("qlpack.yml")).unwrap();
        assert!(
            file.ends_with("defaultSuite:\n  - query: \"queries/Q.ql\"\n"),
            "{file}"
        );
        assert!(file.starts_with("name: me/q\n"), "{file}");

        let loose = tmp.path().join("ws/Loose.ql");
        std::fs::write(&loose, "select 2").unwrap();
        let out = tmp.path().join("out2");
        let pack = prepare_pack(&loose, "go", &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(pack.join("Loose.ql")).unwrap(),
            "select 2"
        );
        let file = std::fs::read_to_string(pack.join("qlpack.yml")).unwrap();
        assert!(file.contains("codeql/go-all: \"*\""), "{file}");
        assert!(file.contains("- query: \"Loose.ql\""), "{file}");
    }

    #[test]
    fn githubs_answer_names_the_run_and_its_skips() {
        let json = r#"{"id": 42, "query_language": "python", "status": "in_progress",
            "actions_workflow_run_id": 777,
            "skipped_repositories": {
                "access_mismatch_repos": {"repository_count": 2, "repositories": []},
                "no_codeql_db_repos": {"repository_count": 1, "repositories": []},
                "not_found_repos": {"repository_count": 0, "repository_full_names": []}
            }}"#;
        let run = parse_submission(json, "me/ctl", Path::new("/q.ql"), "python", 9).unwrap();
        assert_eq!((run.id, run.workflow_run, run.skipped), (42, Some(777), 3));
        assert_eq!(run.url(), "https://github.com/me/ctl/actions/runs/777");
        let bare = parse_submission(r#"{"id": 1}"#, "me/ctl", Path::new("/q.ql"), "go", 0).unwrap();
        assert_eq!(bare.url(), "https://github.com/me/ctl/actions");
        assert!(
            parse_submission(r#"{"message": "x"}"#, "me/ctl", Path::new("/q"), "go", 0).is_err()
        );
    }

    #[test]
    fn submitted_runs_are_remembered_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("runs.json");
        assert!(load_submitted(&path).is_empty());
        for id in [5, 6] {
            let run = parse_submission(
                &format!(r#"{{"id": {id}}}"#),
                "a/b",
                Path::new("/q.ql"),
                "go",
                id,
            )
            .unwrap();
            record_submitted(&path, run).unwrap();
        }
        let ids: Vec<u64> = load_submitted(&path).iter().map(|r| r.id).collect();
        assert_eq!(ids, [5, 6]);
    }
}
