//! `nosqlsync` — migrate MongoDB to PostgreSQL with a real relational schema.

mod analyze;
mod migrate;
mod pg;
mod source;
mod validate;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use nosqlsync_core::plan::Plan;

pub const VERSION: &str = concat!("nosqlsync ", env!("CARGO_PKG_VERSION"));
pub const CLOUD_HINT: &str =
    "Need the target kept in sync until cutover? NoSQLSync Cloud runs live CDC from this same plan: https://nosqlsync.com/?ref=cli";

#[derive(Parser)]
#[command(name = "nosqlsync", version, about = "Migrate MongoDB to PostgreSQL with a real relational schema.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sample collections and report the inferred schema and what will bite you.
    Analyze {
        #[command(flatten)]
        source: SourceArgs,
        /// Output format: `text` for the terminal, `markdown` for docs and PRs, `json` for tooling.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Design the target tables and write an editable plan plus its SQL.
    Plan {
        #[command(flatten)]
        source: SourceArgs,
        /// Where to write the plan.
        #[arg(long, short, default_value = "nosqlsync.plan.json")]
        out: PathBuf,
        /// Where to write the DDL script.
        #[arg(long, default_value = "nosqlsync.schema.sql")]
        sql: PathBuf,
        /// An existing plan whose edits (decisions, renames, retypes) to keep.
        #[arg(long)]
        plan: Option<PathBuf>,
    },
    /// Bulk-load collections into PostgreSQL with COPY. Resumable.
    Migrate {
        #[command(flatten)]
        source: SourceArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Plan to load with. Without one, a plan is inferred with the defaults.
        #[arg(long)]
        plan: Option<PathBuf>,
        /// Rows per COPY batch (and per checkpoint).
        #[arg(long, default_value_t = 5000)]
        batch: usize,
        /// Truncate the target tables and load from scratch.
        #[arg(long)]
        fresh: bool,
    },
    /// Check counts and compare sampled rows field by field.
    Validate {
        #[command(flatten)]
        source: SourceArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Plan to validate against. Without one, a plan is inferred with the defaults.
        #[arg(long)]
        plan: Option<PathBuf>,
        /// Documents to compare per collection.
        #[arg(long, default_value_t = 200)]
        compare: usize,
        /// Write the report here (`.md` or `.json`).
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

#[derive(Args, Clone)]
pub struct SourceArgs {
    /// `mongodb://…` / `mongodb+srv://…`, or a .json/.ndjson/.jsonl file or a directory of them.
    #[arg(long, short, env = "NOSQLSYNC_SOURCE", hide_env_values = true)]
    pub source: String,
    /// Database name, when the connection string doesn't include one.
    #[arg(long)]
    pub db: Option<String>,
    /// Only these collections (comma-separated). Default: all.
    #[arg(long, short, value_delimiter = ',')]
    pub collections: Vec<String>,
    /// Documents to sample per collection for inference (0 = all).
    #[arg(long, default_value_t = 1000)]
    pub sample: usize,
}

#[derive(Args, Clone)]
pub struct TargetArgs {
    /// `postgres://user:pass@host/db`. Honors `sslmode`.
    #[arg(long, short, env = "NOSQLSYNC_TARGET", hide_env_values = true)]
    pub target: String,
    /// Schema to create the tables in.
    #[arg(long, default_value = "public")]
    pub schema: String,
}

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Markdown,
}

pub fn read_plan(path: &PathBuf) -> Result<Plan> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read plan {}", path.display()))?;
    let plan: Plan = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a valid nosqlsync plan", path.display()))?;
    anyhow::ensure!(
        plan.dialect == nosqlsync_core::Dialect::Postgresql,
        "this version of the CLI loads PostgreSQL plans only"
    );
    Ok(plan)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Analyze { source, format } => analyze::analyze(&source, format).await,
        Command::Plan {
            source,
            out,
            sql,
            plan,
        } => analyze::plan(&source, &out, &sql, plan.as_ref()).await,
        Command::Migrate {
            source,
            target,
            plan,
            batch,
            fresh,
        } => migrate::run(&source, &target, plan.as_ref(), batch.max(1), fresh).await,
        Command::Validate {
            source,
            target,
            plan,
            compare,
            report,
        } => validate::run(&source, &target, plan.as_ref(), compare, report.as_ref()).await,
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
