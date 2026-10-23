//! `migrate`: bulk load with COPY, one transaction per batch, resumable.

use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use nosqlsync_core::plan::{CollectionPlan, Plan};
use nosqlsync_core::transform::{child_rows, copy_row, transform_doc, Cell, CoercionIssue};
use nosqlsync_core::Dialect;
use serde_json::Value;
use sqlx::{Connection, PgConnection};

use crate::analyze::analyze_all;
use crate::source::Source;
use crate::{pg, read_plan, SourceArgs, TargetArgs, CLOUD_HINT, VERSION};

/// The plan to work from: the file given, or one inferred with the defaults.
pub async fn resolve_plan(
    args: &SourceArgs,
    source: &Source,
    plan: Option<&PathBuf>,
) -> Result<Plan> {
    match plan {
        Some(path) => {
            let mut p = read_plan(path)?;
            if !args.collections.is_empty() {
                p.collections
                    .retain(|c| args.collections.contains(&c.collection));
                anyhow::ensure!(
                    !p.collections.is_empty(),
                    "none of --collections are in {}",
                    path.display()
                );
            }
            Ok(p)
        }
        None => {
            eprintln!("No --plan given; inferring one with the defaults (run `nosqlsync plan` to review it first).");
            let analyses = analyze_all(args, source, None).await?;
            Ok(Plan::new(
                Dialect::Postgresql,
                VERSION,
                analyses.into_iter().map(|a| a.plan).collect(),
            ))
        }
    }
}

pub async fn run(
    args: &SourceArgs,
    target: &TargetArgs,
    plan: Option<&PathBuf>,
    batch: usize,
    fresh: bool,
) -> Result<()> {
    let source = Source::open(&args.source, args.db.as_deref()).await?;
    let plan = resolve_plan(args, &source, plan).await?;
    let mut conn = pg::connect(&target.target, &target.schema).await?;
    pg::ensure_checkpoints(&mut conn).await?;

    let mut failures = 0;
    for c in &plan.collections {
        if let Err(e) = load_collection(&source, &mut conn, c, batch, fresh).await {
            eprintln!("\n  {}: failed: {e:#}", c.collection);
            eprintln!("  Fix the cause and re-run the same command; it resumes from the last committed batch.");
            failures += 1;
        }
    }
    println!("\nNext: nosqlsync validate --plan <plan> to compare source and target.");
    println!("{CLOUD_HINT}");
    if failures > 0 {
        bail!("{failures} collection(s) failed");
    }
    Ok(())
}

#[derive(Default)]
struct LoadStats {
    coercion: BTreeMap<String, (usize, CoercionIssue)>,
    unmapped: BTreeMap<String, usize>,
}

async fn load_collection(
    source: &Source,
    conn: &mut PgConnection,
    plan: &CollectionPlan,
    batch: usize,
    fresh: bool,
) -> Result<()> {
    let pk_index = plan.fields.iter().position(|f| f.is_primary_key).context(
        "documents have no `_id` field, so there is no primary key to load or resume by",
    )?;
    let pk = &plan.fields[pk_index];

    for stmt in plan.create_statements(Dialect::Postgresql, true) {
        pg::execute(conn, &stmt).await?;
    }

    let checkpoint = pg::checkpoint(conn, &plan.table).await?;
    if fresh {
        pg::execute(conn, &format!("TRUNCATE {} CASCADE", pg::q(&plan.table))).await?;
        pg::clear_checkpoint(conn, &plan.table).await?;
    }
    let checkpoint = if fresh { None } else { checkpoint };
    match &checkpoint {
        Some(c) if c.completed => {
            println!("  {}: already loaded ({} rows). Use --fresh to reload.", plan.collection, c.rows_loaded);
            return Ok(());
        }
        Some(_) if pk.mongo_types.len() > 1 => bail!(
            "`_id` holds several types, and MongoDB only compares `_id` within one type, so resuming could skip documents. Re-run with --fresh."
        ),
        Some(c) => println!("  {}: resuming after {} rows", plan.collection, c.rows_loaded),
        None if pg::has_rows(conn, &plan.table).await? => bail!(
            "table `{}` already has rows that nosqlsync didn't load. Use --fresh to truncate it, or load into another --schema.",
            plan.table
        ),
        None => {}
    }

    let total = source.estimated_count(&plan.collection).await;
    let mut rows_loaded = checkpoint.as_ref().map(|c| c.rows_loaded).unwrap_or(0);
    let mut cursor = checkpoint.and_then(|c| c.cursor).unwrap_or(Value::Null);
    let mut rx = source
        .stream(
            &plan.collection,
            (!cursor.is_null()).then(|| cursor.clone()),
            batch,
        )
        .await?;

    let known: HashSet<&str> = plan
        .fields
        .iter()
        .map(|f| f.source_field.split('.').next().unwrap())
        .chain(
            plan.child_tables
                .iter()
                .map(|c| c.source_path.split('.').next().unwrap()),
        )
        .collect();
    let parent_cols: Vec<String> = plan
        .fields
        .iter()
        .map(|f| f.target_column.clone())
        .collect();
    let child_cols: Vec<Vec<String>> = plan
        .child_tables
        .iter()
        .map(|c| {
            [c.parent_id_column.clone(), "array_index".into()]
                .into_iter()
                .chain(c.fields.iter().map(|f| f.target_column.clone()))
                .collect()
        })
        .collect();

    let mut stats = LoadStats::default();
    let started = Instant::now();
    let started_rows = rows_loaded;
    let mut child_total = 0usize;

    while let Some(b) = rx.recv().await {
        let b = b?;
        let mut parent = String::with_capacity(b.docs.len() * 256);
        let mut children = vec![String::new(); plan.child_tables.len()];
        for doc in &b.docs {
            let mut issues = Vec::new();
            let row = transform_doc(doc, &plan.fields, &mut issues);
            if row[pk_index] == Cell::Null {
                bail!("a document has no `_id`; every row needs a primary key");
            }
            copy_row(&row, &mut parent);
            for (i, ct) in plan.child_tables.iter().enumerate() {
                for r in child_rows(doc, ct, &row[pk_index], &mut issues) {
                    copy_row(&r, &mut children[i]);
                    child_total += 1;
                }
            }
            for issue in issues {
                stats
                    .coercion
                    .entry(issue.column.clone())
                    .or_insert((0, issue))
                    .0 += 1;
            }
            if let Value::Object(o) = doc {
                for k in o.keys().filter(|k| !known.contains(k.as_str())) {
                    *stats.unmapped.entry(k.clone()).or_default() += 1;
                }
            }
        }

        let mut tx = conn.begin().await?;
        pg::copy_in(&mut tx, &plan.table, &parent_cols, &parent)
            .await
            .map_err(|e| {
                // MongoDB guarantees unique _ids; exported files don't.
                if format!("{e:#}").contains(&format!("\"{}_pkey\"", plan.table)) {
                    e.context("the source has duplicate `_id` values; every document needs a unique `_id`")
                } else {
                    e
                }
            })?;
        for (i, ct) in plan.child_tables.iter().enumerate() {
            pg::copy_in(&mut tx, &ct.table_name, &child_cols[i], &children[i]).await?;
        }
        rows_loaded += b.docs.len() as i64;
        cursor = b.cursor;
        pg::save_checkpoint(&mut tx, plan, &cursor, rows_loaded, false).await?;
        tx.commit().await?;
        progress(plan, rows_loaded, total, started, started_rows);
    }

    pg::save_checkpoint(conn, plan, &cursor, rows_loaded, true).await?;
    for stmt in plan.index_statements(Dialect::Postgresql) {
        pg::execute(conn, &stmt).await?;
    }
    pg::execute(conn, &format!("ANALYZE {}", pg::q(&plan.table))).await?;
    progress(plan, rows_loaded, total, started, started_rows);
    eprintln!();

    let extra = if child_total > 0 {
        format!(" + {child_total} child rows")
    } else {
        String::new()
    };
    println!(
        "  {}: done — {rows_loaded} rows{extra} in {:.1}s",
        plan.collection,
        started.elapsed().as_secs_f64()
    );
    report(&stats);
    Ok(())
}

fn progress(plan: &CollectionPlan, rows: i64, total: Option<u64>, started: Instant, from: i64) {
    let secs = started.elapsed().as_secs_f64().max(0.001);
    let rate = (rows - from) as f64 / secs;
    let of = total.map(|t| format!(" / ~{t}")).unwrap_or_default();
    eprint!(
        "\r  {}: {rows}{of} rows ({rate:.0} rows/s)   ",
        plan.collection
    );
    let _ = std::io::stderr().flush();
}

fn report(stats: &LoadStats) {
    for (col, (n, example)) in &stats.coercion {
        println!(
            "    [warn] {n} value(s) in `{col}` were not valid {} and were loaded as NULL (e.g. {})",
            example.expected.as_str(),
            example.value
        );
    }
    if !stats.unmapped.is_empty() {
        let list = stats
            .unmapped
            .iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("    [warn] fields not in the plan were not loaded: {list}. Re-run `nosqlsync plan` with a larger --sample to include them.");
    }
}
