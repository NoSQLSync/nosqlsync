//! `analyze` and `plan`.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use nosqlsync_core::flatten::NestedDecision;
use nosqlsync_core::plan::{analyze_collection, Analysis, Plan};
use nosqlsync_core::risks::Severity;
use nosqlsync_core::Dialect;
use serde_json::json;

use crate::source::Source;
use crate::{read_plan, Format, SourceArgs, VERSION};

/// Sample and analyze every selected collection, keeping edits from `previous`.
pub async fn analyze_all(
    args: &SourceArgs,
    source: &Source,
    previous: Option<&Plan>,
) -> Result<Vec<Analysis>> {
    let mut out = Vec::new();
    for name in source.collections(&args.collections).await? {
        let docs = source.sample(&name, args.sample).await?;
        if docs.is_empty() {
            eprintln!("  {name}: empty, skipped");
            continue;
        }
        let prev = previous.and_then(|p| p.collections.iter().find(|c| c.collection == name));
        out.push(analyze_collection(
            &name,
            &docs,
            Dialect::Postgresql,
            source.detect_mode(),
            prev,
        ));
    }
    anyhow::ensure!(
        !out.is_empty(),
        "no documents found in {}",
        source.describe()
    );
    Ok(out)
}

pub async fn analyze(args: &SourceArgs, format: Format) -> Result<()> {
    let source = Source::open(&args.source, args.db.as_deref()).await?;
    eprintln!("Sampling {} …", source.describe());
    let analyses = analyze_all(args, &source, None).await?;
    match format {
        Format::Json => {
            let v: Vec<_> = analyses
                .iter()
                .map(|a| json!({ "collection": a.plan.collection, "plan": a.plan, "model": a.model, "risks": a.risks }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Format::Text | Format::Markdown => {
            print!("{}", render(&analyses, format == Format::Markdown))
        }
    }
    Ok(())
}

pub async fn plan(
    args: &SourceArgs,
    out: &PathBuf,
    sql: &PathBuf,
    previous: Option<&PathBuf>,
) -> Result<()> {
    let previous = previous.map(read_plan).transpose()?;
    let source = Source::open(&args.source, args.db.as_deref()).await?;
    eprintln!("Sampling {} …", source.describe());
    let analyses = analyze_all(args, &source, previous.as_ref()).await?;
    let plan = Plan::new(
        Dialect::Postgresql,
        VERSION,
        analyses.iter().map(|a| a.plan.clone()).collect(),
    );
    std::fs::write(out, serde_json::to_string_pretty(&plan)? + "\n")?;
    std::fs::write(sql, plan.sql_script())?;

    for a in &analyses {
        let p = &a.plan;
        let children = p
            .child_tables
            .iter()
            .map(|c| c.table_name.as_str())
            .collect::<Vec<_>>();
        println!(
            "  {} → {} ({} columns{}{})",
            p.collection,
            p.table,
            p.fields.len(),
            if children.is_empty() {
                String::new()
            } else {
                format!(", child tables: {}", children.join(", "))
            },
            if p.indexes.is_empty() {
                String::new()
            } else {
                format!(", {} indexes", p.indexes.len())
            },
        );
        let warnings = a
            .risks
            .iter()
            .filter(|r| r.severity == Severity::Warning)
            .count();
        if warnings > 0 {
            println!("    {warnings} warning(s) — run `nosqlsync analyze` for details");
        }
    }
    println!("\nWrote {} and {}.", out.display(), sql.display());
    println!(
        "Edit `decisions` (flatten | jsonb | child-table) and re-run with --plan {} to apply them,",
        out.display()
    );
    println!(
        "or rename/retype columns directly in the plan. Then: nosqlsync migrate --plan {}",
        out.display()
    );
    Ok(())
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// Human-readable analysis; Markdown when `md` is set.
pub fn render(analyses: &[Analysis], md: bool) -> String {
    let mut s = String::new();
    for a in analyses {
        let p = &a.plan;
        if md {
            let _ = writeln!(
                s,
                "## `{}` → `{}`\n\n{} documents sampled.\n",
                p.collection, p.table, p.documents_sampled
            );
            let _ = writeln!(
                s,
                "| Column | Type | Source | Non-null | Types seen |\n|---|---|---|---|---|"
            );
        }
        let width = p
            .fields
            .iter()
            .map(|f| f.target_column.len())
            .max()
            .unwrap_or(0)
            .max(6);
        if !md {
            let _ = writeln!(
                s,
                "\n{} → table {} ({} documents sampled)\n\n  {:width$}  {:18} {:>8}  source field",
                p.collection, p.table, p.documents_sampled, "column", "type", "non-null"
            );
        }
        for f in &p.fields {
            let types = f
                .mongo_types
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let present = pct(1.0 - f.null_fraction);
            let key = if f.is_primary_key { " PK" } else { "" };
            if md {
                let _ = writeln!(
                    s,
                    "| `{}`{key} | `{}` | `{}` | {present} | {types} |",
                    f.target_column, f.sql_type, f.source_field
                );
            } else {
                let _ = writeln!(
                    s,
                    "  {:width$}  {:18} {:>8}  {}{key}",
                    f.target_column, f.sql_type, present, f.source_field
                );
            }
        }

        if !p.child_tables.is_empty() || !p.decisions.is_empty() {
            let _ = writeln!(
                s,
                "{}",
                if md {
                    "\n**Relational structure**\n"
                } else {
                    "\n  Relational structure"
                }
            );
            for (path, d) in &p.decisions {
                let what = match d {
                    NestedDecision::Flatten => "flattened into columns".to_string(),
                    NestedDecision::Jsonb => "kept as JSONB".to_string(),
                    NestedDecision::ChildTable => {
                        let t = p
                            .child_tables
                            .iter()
                            .find(|c| &c.source_path == path)
                            .map(|c| c.table_name.as_str())
                            .unwrap_or("?");
                        format!("child table {t}")
                    }
                };
                let reason = a
                    .model
                    .nested
                    .iter()
                    .find(|n| &n.path == path)
                    .map(|n| n.reason.as_str())
                    .unwrap_or("");
                let _ = writeln!(
                    s,
                    "{}{path}: {what}. {reason}",
                    if md { "- " } else { "    " }
                );
            }
        }
        if !p.indexes.is_empty() {
            let cols = p
                .indexes
                .iter()
                .map(|i| i.column.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                s,
                "{}Indexes on reference columns: {cols}",
                if md { "\n" } else { "\n  " }
            );
        }

        if a.risks.is_empty() {
            let _ = writeln!(s, "{}No problems found.", if md { "\n" } else { "\n  " });
        } else {
            let _ = writeln!(
                s,
                "{}",
                if md {
                    "\n**What will bite you**\n"
                } else {
                    "\n  What will bite you"
                }
            );
            for r in &a.risks {
                let tag = match r.severity {
                    Severity::Warning => "warn",
                    Severity::Info => "info",
                };
                let _ = writeln!(
                    s,
                    "{}[{tag}] {}: {}",
                    if md { "- " } else { "    " },
                    r.path,
                    r.message
                );
            }
        }
        if md {
            s.push('\n');
        }
    }
    s
}
