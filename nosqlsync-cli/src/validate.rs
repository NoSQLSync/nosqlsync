//! `validate`: count parity plus a field-by-field comparison of sampled documents.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use nosqlsync_core::plan::CollectionPlan;
use nosqlsync_core::transform::{child_rows, transform_doc, Cell};
use nosqlsync_core::validate::{
    compare_named, compare_row, overall, verdict, Mismatch, TableReport, Verdict,
};
use serde_json::json;
use sqlx::PgConnection;

use crate::migrate::resolve_plan;
use crate::source::Source;
use crate::{pg, SourceArgs, TargetArgs, CLOUD_HINT, VERSION};

pub async fn run(
    args: &SourceArgs,
    target: &TargetArgs,
    plan: Option<&PathBuf>,
    compare: usize,
    report: Option<&PathBuf>,
) -> Result<()> {
    let source = Source::open(&args.source, args.db.as_deref()).await?;
    let plan = resolve_plan(args, &source, plan).await?;
    let mut conn = pg::connect(&target.target, &target.schema).await?;

    let mut tables = Vec::new();
    for c in &plan.collections {
        eprint!("  {}: checking … ", c.collection);
        let t = check(&source, &mut conn, c, compare)
            .await
            .with_context(|| format!("validating {}", c.collection))?;
        eprintln!("{}", label(t.verdict));
        tables.push(t);
    }
    let result = overall(&tables);
    let checked_at = now_iso();

    print!("{}", text_summary(&tables, result));
    if let Some(path) = report {
        let body = if path.extension().and_then(|e| e.to_str()) == Some("json") {
            serde_json::to_string_pretty(
                &json!({ "verdict": result, "checkedAt": checked_at, "generatedBy": VERSION, "tables": tables }),
            )?
        } else {
            markdown(&tables, result, &checked_at)
        };
        std::fs::write(path, body)?;
        println!("Report written to {}", path.display());
    }
    println!("{CLOUD_HINT}");
    if result == Verdict::Failed {
        std::process::exit(2);
    }
    Ok(())
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    nosqlsync_core::ejson::millis_to_iso(secs).unwrap_or_default()
}

fn pk_text(cell: &Cell) -> Option<String> {
    match cell {
        Cell::Text(s) => Some(s.clone()),
        Cell::Int(i) => Some(i.to_string()),
        Cell::Float(f) => Some(f.to_string()),
        Cell::Bool(b) => Some(b.to_string()),
        Cell::Timestamp(t) => Some(t.to_rfc3339()),
        Cell::Numeric(s) => Some(s.clone()),
        _ => None,
    }
}

async fn check(
    source: &Source,
    conn: &mut PgConnection,
    plan: &CollectionPlan,
    compare: usize,
) -> Result<TableReport> {
    let pk_index = plan
        .fields
        .iter()
        .position(|f| f.is_primary_key)
        .context("plan has no primary key")?;
    let pk = &plan.fields[pk_index];
    anyhow::ensure!(
        pg::table_exists(conn, &plan.table).await?,
        "table `{}` does not exist; run migrate first",
        plan.table
    );

    let source_count = source.count(&plan.collection).await.ok();
    let target_count = pg::count(conn, &plan.table).await?;
    let docs = source
        .spread_sample(&plan.collection, compare, source_count.unwrap_or(0))
        .await?;

    let mut missing = Vec::new();
    let mut mismatches = Vec::new();
    for doc in &docs {
        let mut issues = Vec::new();
        let expected = transform_doc(doc, &plan.fields, &mut issues);
        let Some(key) = pk_text(&expected[pk_index]) else {
            continue;
        };
        let Some(actual) =
            pg::row_json(conn, &plan.table, &pk.target_column, &pk.sql_type, &key).await?
        else {
            missing.push(key);
            continue;
        };
        let cols = compare_row(&expected, &plan.fields, &actual);
        if !cols.is_empty() {
            mismatches.push(Mismatch {
                primary_key: key.clone(),
                table: plan.table.clone(),
                columns: cols,
            });
        }

        for ct in &plan.child_tables {
            let want = child_rows(doc, ct, &expected[pk_index], &mut issues);
            let got = pg::child_rows_json(
                conn,
                &ct.table_name,
                &ct.parent_id_column,
                &pk.sql_type,
                &key,
            )
            .await?;
            let mut cols: Vec<String> = Vec::new();
            if want.len() != got.len() {
                cols.push(format!("row count {} ≠ {}", got.len(), want.len()));
            }
            for (w, g) in want.iter().zip(&got) {
                let named: Vec<(String, Cell)> =
                    std::iter::once(("array_index".to_string(), w[1].clone()))
                        .chain(
                            ct.fields
                                .iter()
                                .zip(&w[2..])
                                .map(|(f, c)| (f.target_column.clone(), c.clone())),
                        )
                        .collect();
                for c in compare_named(&named, g) {
                    if !cols.contains(&c) {
                        cols.push(c);
                    }
                }
            }
            if !cols.is_empty() {
                mismatches.push(Mismatch {
                    primary_key: key.clone(),
                    table: ct.table_name.clone(),
                    columns: cols,
                });
            }
        }
    }

    let count_match = source_count.map(|s| s == target_count);
    Ok(TableReport {
        collection: plan.collection.clone(),
        table: plan.table.clone(),
        source_count,
        target_count,
        count_match,
        sampled: docs.len(),
        verdict: verdict(count_match, docs.len(), missing.len(), mismatches.len()),
        missing,
        mismatches,
    })
}

fn label(v: Verdict) -> &'static str {
    match v {
        Verdict::Passed => "passed",
        Verdict::Failed => "FAILED",
        Verdict::Incomplete => "incomplete",
    }
}

fn text_summary(tables: &[TableReport], result: Verdict) -> String {
    let mut s = String::from("\n");
    for t in tables {
        let src = t
            .source_count
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
        let _ = writeln!(
            s,
            "  {:10} {} → {}: {src} source / {} target rows; {} compared, {} missing, {} mismatched",
            label(t.verdict),
            t.collection,
            t.table,
            t.target_count,
            t.sampled,
            t.missing.len(),
            t.mismatches.len()
        );
        for m in t.mismatches.iter().take(5) {
            let _ = writeln!(
                s,
                "             {} {}: {}",
                m.table,
                m.primary_key,
                m.columns.join(", ")
            );
        }
    }
    let _ = writeln!(s, "\nResult: {}", label(result));
    s
}

fn markdown(tables: &[TableReport], result: Verdict, checked_at: &str) -> String {
    let mut s = format!(
        "# Migration validation report\n\n**Result: {}** · checked {checked_at} · {VERSION}\n\n",
        label(result)
    );
    s.push_str("| Collection | Table | Source rows | Target rows | Compared | Missing | Mismatched | Result |\n|---|---|---|---|---|---|---|---|\n");
    for t in tables {
        let src = t
            .source_count
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".into());
        let _ = writeln!(
            s,
            "| {} | `{}` | {src} | {} | {} | {} | {} | {} |",
            t.collection,
            t.table,
            t.target_count,
            t.sampled,
            t.missing.len(),
            t.mismatches.len(),
            label(t.verdict)
        );
    }
    for t in tables
        .iter()
        .filter(|t| !t.missing.is_empty() || !t.mismatches.is_empty())
    {
        let _ = writeln!(s, "\n## {}\n", t.collection);
        for k in &t.missing {
            let _ = writeln!(s, "- `{k}`: no row in `{}`", t.table);
        }
        for m in &t.mismatches {
            let _ = writeln!(
                s,
                "- `{}` in `{}`: {}",
                m.primary_key,
                m.table,
                m.columns.join(", ")
            );
        }
    }
    s.push_str(
        "\n---\n\n**How this was checked.** Row counts are exact on both sides. Compared documents were sampled \
         from the source, run through the same transform the load used, and compared column by column with the \
         target row (and each child row). A sample can show a load is wrong; it cannot prove every row is right.\n\n",
    );
    let _ = writeln!(s, "_{CLOUD_HINT}_");
    s
}
