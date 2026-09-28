//! The CodeQL evaluator log viewer (#578), as VS Code's "Show Evaluator
//! Log (Viewer)" shows it: every evaluated predicate with its time and
//! result size, slowest first, each expanding into the RA pipelines it ran
//! and the predicates it depends on.
//!
//! The input is what `codeql generate log-summary --format=predicates`
//! writes: one JSON object per line. Lines that are not predicate records
//! (headers, totals, anything a newer CLI adds) are skipped, and unknown
//! fields are ignored. The output is plain indented text, so the editor's
//! indentation folding makes it a tree.

use serde_json::Value;

/// One RA pipeline of a predicate: its name in the summary's `ra` map
/// (`pipeline` for a simple predicate, `base`, `standard` and so on for a
/// recursive one), how many times it ran, and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub name: String,
    pub runs: usize,
    pub lines: Vec<String>,
}

/// One evaluated predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Predicate {
    pub name: String,
    pub strategy: String,
    pub millis: u64,
    pub result_size: u64,
    /// How many iterations a recursive predicate took; 0 otherwise.
    pub iterations: usize,
    pub pipelines: Vec<Pipeline>,
    pub dependencies: Vec<String>,
}

/// The predicate records in a predicates-format summary, slowest first
/// (ties by name, so the order is stable).
pub fn parse(summary: &str) -> Vec<Predicate> {
    let mut predicates: Vec<Predicate> = summary
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter_map(|v| predicate(&v))
        .collect();
    predicates.sort_by(|a, b| b.millis.cmp(&a.millis).then_with(|| a.name.cmp(&b.name)));
    predicates
}

fn predicate(v: &Value) -> Option<Predicate> {
    let name = v.get("predicateName")?.as_str()?.to_string();
    let strategy = v.get("evaluationStrategy")?.as_str()?.to_string();
    let number = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
    let iterations = v
        .get("predicateIterationMillis")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut dependencies: Vec<String> = v
        .get("dependencies")
        .and_then(Value::as_object)
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default();
    dependencies.sort();
    Some(Predicate {
        name,
        strategy,
        millis: number("millis"),
        result_size: number("resultSize"),
        iterations,
        pipelines: pipelines(v),
        dependencies,
    })
}

/// The pipelines in `ra`, in the order `pipelineRuns` first ran them, with
/// any that never ran after. `ra` is a map of named pipelines, or a bare
/// list of lines from an older CLI.
fn pipelines(v: &Value) -> Vec<Pipeline> {
    let lines = |p: &Value| -> Vec<String> {
        p.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.trim().is_empty())
            .collect()
    };
    let runs: Vec<&str> = v
        .get("pipelineRuns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("raReference").and_then(Value::as_str))
        .collect();
    match v.get("ra") {
        Some(Value::Object(ra)) => {
            let mut names: Vec<&str> = Vec::new();
            for name in runs.iter().copied().chain(ra.keys().map(String::as_str)) {
                if ra.contains_key(name) && !names.contains(&name) {
                    names.push(name);
                }
            }
            names
                .into_iter()
                .map(|name| Pipeline {
                    name: name.to_string(),
                    runs: runs.iter().filter(|r| **r == name).count(),
                    lines: lines(&ra[name]),
                })
                .collect()
        }
        Some(ra @ Value::Array(_)) => vec![Pipeline {
            name: String::from("pipeline"),
            runs: runs.len(),
            lines: lines(ra),
        }],
        _ => Vec::new(),
    }
}

/// `n` with thousands separators: 1234567 is "1,234,567".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The header line of `p` in the tree.
pub fn header(p: &Predicate) -> String {
    let rows = if p.result_size == 1 { "row" } else { "rows" };
    let mut line = format!(
        "\u{25b8} {} ms  {} {rows}  {} ({})",
        grouped(p.millis),
        grouped(p.result_size),
        p.name,
        p.strategy
    );
    if p.iterations > 0 {
        let s = if p.iterations == 1 { "" } else { "s" };
        line.push_str(&format!(", {} iteration{s}", p.iterations));
    }
    line
}

/// The viewer's text for `predicates` run by `query`: a title, then each
/// predicate's header with its pipelines (RA lines indented beneath) and
/// dependencies as children. Children are indented, so each predicate
/// folds.
pub fn render(query: &str, predicates: &[Predicate]) -> String {
    let total: u64 = predicates.iter().map(|p| p.millis).sum();
    let count = predicates.len();
    let s = if count == 1 { "" } else { "s" };
    let mut out = format!(
        "Evaluator log for {query}: {count} predicate{s}, {} ms in all, slowest first\n",
        grouped(total)
    );
    for p in predicates {
        out.push('\n');
        out.push_str(&header(p));
        out.push('\n');
        for pipeline in &p.pipelines {
            out.push_str(&format!("    Pipeline {}", pipeline.name));
            if pipeline.runs > 1 {
                out.push_str(&format!(" ({} runs)", pipeline.runs));
            }
            out.push('\n');
            for line in &pipeline.lines {
                out.push_str("        ");
                out.push_str(line.trim_start());
                out.push('\n');
            }
        }
        if !p.dependencies.is_empty() {
            out.push_str(&format!("    Dependencies ({})\n", p.dependencies.len()));
            for d in &p.dependencies {
                out.push_str("        ");
                out.push_str(d);
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        r#"{"summaryLogVersion":"0.4.0","codeqlVersion":"2.19.0","startTime":"2026-09-28T10:00:00Z"}"#,
        "\n",
        r#"{"completionTime":"2026-09-28T10:00:01Z","raHash":"a1","predicateName":"Foo::bar#abc","appearsAs":{"Foo::bar#abc":{"q.ql":[1]}},"queryCausingWork":"q.ql","evaluationStrategy":"COMPUTE_SIMPLE","millis":1234,"resultSize":12345,"dependencies":{"Foo::baz#def":"b2","files":"c3"},"ra":{"pipeline":["    {2} r1 = SCAN files OUTPUT In.0, In.1","    {2} r2 = JOIN r1 WITH Foo::baz#def ON FIRST 1 OUTPUT Lhs.0, Rhs.1","","    return r2"]},"pipelineRuns":[{"raReference":"pipeline","counts":[10,12345]}],"futureField":{"x":1}}"#,
        "\n",
        "not json at all\n",
        r#"{"completionTime":"2026-09-28T10:00:02Z","raHash":"d4","predicateName":"Reach::step#rec","evaluationStrategy":"COMPUTE_RECURSIVE","millis":5000,"resultSize":1,"predicateIterationMillis":[100,2400,2500],"deltaSizes":[1,0,0],"dependencies":{"Foo::bar#abc":"a1"},"ra":{"base":["{1} r1 = Foo::bar#abc","return r1"],"standard":["{1} r1 = JOIN Reach::step#rec#prev_delta WITH Foo::bar#abc","return r1"]},"pipelineRuns":[{"raReference":"base","counts":[1]},{"raReference":"standard","counts":[0]},{"raReference":"standard","counts":[0]}]}"#,
        "\n",
        r#"{"completionTime":"2026-09-28T10:00:03Z","raHash":"e5","predicateName":"files","evaluationStrategy":"EXTENSIONAL","resultSize":3}"#,
        "\n",
    );

    #[test]
    fn parses_predicate_records_slowest_first_skipping_the_rest() {
        let ps = parse(SAMPLE);
        let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Reach::step#rec", "Foo::bar#abc", "files"]);

        let rec = &ps[0];
        assert_eq!(rec.strategy, "COMPUTE_RECURSIVE");
        assert_eq!((rec.millis, rec.result_size, rec.iterations), (5000, 1, 3));
        assert_eq!(rec.dependencies, ["Foo::bar#abc"]);
        assert_eq!(
            rec.pipelines,
            vec![
                Pipeline {
                    name: String::from("base"),
                    runs: 1,
                    lines: vec![
                        String::from("{1} r1 = Foo::bar#abc"),
                        String::from("return r1")
                    ],
                },
                Pipeline {
                    name: String::from("standard"),
                    runs: 2,
                    lines: vec![
                        String::from("{1} r1 = JOIN Reach::step#rec#prev_delta WITH Foo::bar#abc"),
                        String::from("return r1")
                    ],
                },
            ]
        );

        let simple = &ps[1];
        assert_eq!((simple.millis, simple.result_size), (1234, 12345));
        assert_eq!(simple.dependencies, ["Foo::baz#def", "files"]);
        assert_eq!(simple.pipelines.len(), 1);
        assert_eq!(simple.pipelines[0].lines.len(), 3, "blank RA lines drop");

        // No time and no RA: an extensional table read from the database.
        let ext = &ps[2];
        assert_eq!((ext.millis, ext.result_size), (0, 3));
        assert!(ext.pipelines.is_empty() && ext.dependencies.is_empty());
    }

    #[test]
    fn a_bare_ra_list_is_one_pipeline() {
        let ps = parse(
            r#"{"predicateName":"p","evaluationStrategy":"COMPUTE_SIMPLE","millis":1,"ra":["return r1"]}"#,
        );
        assert_eq!(ps[0].pipelines[0].name, "pipeline");
        assert_eq!(ps[0].pipelines[0].lines, ["return r1"]);
        assert!(parse("").is_empty());
    }

    #[test]
    fn renders_an_indented_tree() {
        let text = render("q.ql", &parse(SAMPLE));
        let expected = "\
Evaluator log for q.ql: 3 predicates, 6,234 ms in all, slowest first

\u{25b8} 5,000 ms  1 row  Reach::step#rec (COMPUTE_RECURSIVE), 3 iterations
    Pipeline base
        {1} r1 = Foo::bar#abc
        return r1
    Pipeline standard (2 runs)
        {1} r1 = JOIN Reach::step#rec#prev_delta WITH Foo::bar#abc
        return r1
    Dependencies (1)
        Foo::bar#abc

\u{25b8} 1,234 ms  12,345 rows  Foo::bar#abc (COMPUTE_SIMPLE)
    Pipeline pipeline
        {2} r1 = SCAN files OUTPUT In.0, In.1
        {2} r2 = JOIN r1 WITH Foo::baz#def ON FIRST 1 OUTPUT Lhs.0, Rhs.1
        return r2
    Dependencies (2)
        Foo::baz#def
        files

\u{25b8} 0 ms  3 rows  files (EXTENSIONAL)
";
        assert_eq!(text, expected);
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(999), "999");
    }
}
