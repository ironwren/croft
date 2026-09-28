//! The CodeQL Model Editor's endpoints (#578): the methods and classes of
//! a library that a model can make sources, sinks or summaries.
//!
//! VS Code's model editor gets them from the language's query pack: a
//! `@kind table` query tagged `modeleditor` and `endpoints`. For Python
//! that is `utils/modeleditor/FrameworkModeEndpoints.ql`, the library's own
//! public API ("framework mode"). Each row names the endpoint, its
//! namespace (the top package), its class (the module path, with the class
//! when it has one), its function and parameters, whether CodeQL already
//! models it, its file and what kind of endpoint it is.

use crate::codeql_query::CellLoc;
use std::collections::HashSet;

/// A query suite selecting `lang`'s model-editor endpoints query.
pub fn endpoints_suite(lang: &str) -> String {
    format!(
        "- from: codeql/{lang}-queries\n  queries: .\n- include:\n    kind: table\n    tags contain all:\n      - modeleditor\n      - endpoints\n"
    )
}

/// One endpoint a model can describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The top package: the `type` column a model names.
    pub namespace: String,
    /// The module path, with the class when there is one (`core.Runner`).
    pub class: String,
    /// The function's name; empty for a class itself.
    pub function: String,
    /// The parameter list as the query prints it: `(self,what)`.
    pub params: String,
    /// Whether CodeQL already models it.
    pub supported: bool,
    /// `Function`, `InstanceMethod`, `Class`, …
    pub kind: String,
    pub location: Option<CellLoc>,
}

impl Endpoint {
    /// The group it is listed under: `mylib.core` or `mylib.core.Runner`.
    pub fn group(&self) -> String {
        if self.class.is_empty() {
            self.namespace.clone()
        } else {
            format!("{}.{}", self.namespace, self.class)
        }
    }

    /// Its line in the list: `run(cmd,shell)`, or `class Runner`.
    pub fn label(&self) -> String {
        if self.function.is_empty() {
            let name = self.class.rsplit('.').next().unwrap_or(&self.class);
            format!("class {name}")
        } else {
            format!("{}{}", self.function, self.params)
        }
    }
}

/// Read the endpoints query's `#select`, decoded as JSON with entity
/// locations (`codeql_query::decode_locations_args`).
pub fn parse_endpoints(json: &str) -> Vec<Endpoint> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let text = |c: Option<&serde_json::Value>| -> String {
        match c {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Object(o)) => o
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    };
    let rows = v
        .get("tuples")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    let located = crate::codeql_query::parse_row_locations(json);
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let r = r.as_array()?;
            Some(Endpoint {
                namespace: text(r.get(1)),
                class: text(r.get(2)),
                function: text(r.get(3)),
                params: text(r.get(4)),
                supported: r.get(5).and_then(|b| b.as_bool()).unwrap_or(false),
                kind: text(r.get(8)),
                location: located
                    .get(i)
                    .and_then(|l| l.locs.first().cloned().flatten()),
            })
        })
        .collect()
}

/// The Model Editor the side bar's Method Modeling section shows: the
/// database and language it is for, the endpoints by group, and which
/// groups are folded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelView {
    pub database: String,
    pub language: String,
    pub endpoints: Vec<Endpoint>,
    pub folded: HashSet<String>,
}

/// One row of the section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRow {
    /// A group header and how many of its endpoints CodeQL models.
    Group(String, usize, usize),
    /// Endpoint `.0`, and its line.
    Endpoint(usize, String),
}

impl ModelView {
    /// The groups in order of first appearance, each with its endpoints'
    /// indices, sorted by name within the group.
    fn groups(&self) -> Vec<(String, Vec<usize>)> {
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, e) in self.endpoints.iter().enumerate() {
            let g = e.group();
            match out.iter_mut().find(|(name, _)| *name == g) {
                Some((_, members)) => members.push(i),
                None => out.push((g, vec![i])),
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, members) in &mut out {
            members.sort_by_key(|&i| self.endpoints[i].label());
        }
        out
    }

    /// The rows to show: each group, and its endpoints unless folded.
    /// Modeled endpoints are marked ✓, the rest ○.
    pub fn rows(&self) -> Vec<ModelRow> {
        let mut out = Vec::new();
        for (group, members) in self.groups() {
            let modeled = members
                .iter()
                .filter(|&&i| self.endpoints[i].supported)
                .count();
            let folded = self.folded.contains(&group);
            out.push(ModelRow::Group(group, modeled, members.len()));
            if folded {
                continue;
            }
            for i in members {
                let e = &self.endpoints[i];
                let mark = if e.supported { '\u{2713}' } else { '\u{25cb}' };
                out.push(ModelRow::Endpoint(i, format!("  {mark} {}", e.label())));
            }
        }
        out
    }

    /// Fold or unfold `group`.
    pub fn toggle(&mut self, group: &str) {
        if !self.folded.remove(group) {
            self.folded.insert(group.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Real `FrameworkModeEndpoints.ql` rows (python-queries 1.8.11) for a
    /// package with `core.run`, `class core.Runner` and `Runner.go`.
    const ROWS: &str = r#"{"columns":[],"tuples":[
      [{"label":"Function run","url":{"uri":"file:///w/mylib/core.py","startLine":3,"startColumn":1,"endLine":3,"endColumn":25}},"mylib","core","run","(cmd,shell)",false,"core.py","",{"label":"Function"}],
      [{"label":"Class Runner","url":{"uri":"file:///w/mylib/core.py","startLine":6,"startColumn":1,"endLine":6,"endColumn":13}},"mylib","core.Runner","","",false,"core.py","",{"label":"Class"}],
      [{"label":"Function go","url":{"uri":"file:///w/mylib/core.py","startLine":7,"startColumn":5,"endLine":7,"endColumn":23}},"mylib","core.Runner","go","(self,what)",true,"core.py","",{"label":"InstanceMethod"}]]}"#;

    #[test]
    fn endpoints_are_read_from_the_query_rows() {
        let eps = parse_endpoints(ROWS);
        assert_eq!(eps.len(), 3);
        assert_eq!(
            (
                eps[0].namespace.as_str(),
                eps[0].class.as_str(),
                eps[0].function.as_str()
            ),
            ("mylib", "core", "run")
        );
        assert_eq!(eps[0].label(), "run(cmd,shell)");
        assert_eq!(eps[0].group(), "mylib.core");
        assert_eq!(eps[0].kind, "Function");
        assert_eq!(
            eps[0].location,
            Some(CellLoc {
                path: PathBuf::from("/w/mylib/core.py"),
                line: 3,
                column: 1
            })
        );
        assert_eq!(eps[1].label(), "class Runner");
        assert_eq!(eps[2].group(), "mylib.core.Runner");
        assert!(eps[2].supported);
        assert!(parse_endpoints("oops").is_empty());
    }

    #[test]
    fn the_section_groups_endpoints_and_folds_groups() {
        let mut v = ModelView {
            database: String::from("lib"),
            language: String::from("python"),
            endpoints: parse_endpoints(ROWS),
            ..ModelView::default()
        };
        assert_eq!(
            v.rows(),
            [
                ModelRow::Group(String::from("mylib.core"), 0, 1),
                ModelRow::Endpoint(0, String::from("  \u{25cb} run(cmd,shell)")),
                ModelRow::Group(String::from("mylib.core.Runner"), 1, 2),
                ModelRow::Endpoint(1, String::from("  \u{25cb} class Runner")),
                ModelRow::Endpoint(2, String::from("  \u{2713} go(self,what)")),
            ]
        );
        v.toggle("mylib.core.Runner");
        assert_eq!(v.rows().len(), 3, "a folded group hides its endpoints");
    }

    #[test]
    fn the_endpoints_query_is_found_by_its_tags() {
        assert_eq!(
            endpoints_suite("python"),
            "- from: codeql/python-queries\n  queries: .\n- include:\n    kind: table\n    tags contain all:\n      - modeleditor\n      - endpoints\n"
        );
    }
}
