//! PostgreSQL target: schema setup, checkpoints, and COPY loading.
//!
//! Every batch is one transaction holding the parent COPY, each child table's
//! COPY, and the checkpoint update. A crash therefore leaves the target at a
//! batch boundary that the checkpoint describes exactly, and a re-run resumes
//! from it without duplicates or gaps.

use anyhow::{Context, Result};
use nosqlsync_core::plan::CollectionPlan;
use nosqlsync_core::types::quote_ident;
use nosqlsync_core::Dialect;
use serde_json::Value;
use sqlx::{Connection, PgConnection, Row};

/// `format!` wrapped as audited SQL. Every dynamic statement in this module is
/// built from fixed SQL plus identifiers passed through [`q`] (quoted and
/// escaped); values always go through binds.
macro_rules! safe {
    ($($t:tt)*) => { sqlx::AssertSqlSafe(format!($($t)*)) };
}

pub const CHECKPOINTS: &str = "_nosqlsync_checkpoints";

pub fn q(name: &str) -> String {
    quote_ident(name, Dialect::Postgresql)
}

pub async fn connect(url: &str, schema: &str) -> Result<PgConnection> {
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        anyhow::bail!("target must be a postgres:// or postgresql:// URL");
    }
    let mut conn = PgConnection::connect(url)
        .await
        .context("could not connect to PostgreSQL")?;
    sqlx::raw_sql(safe!(
        "CREATE SCHEMA IF NOT EXISTS {0}; SET search_path TO {0};",
        q(schema)
    ))
    .execute(&mut conn)
    .await
    .with_context(|| format!("could not use schema `{schema}`"))?;
    Ok(conn)
}

pub async fn ensure_checkpoints(conn: &mut PgConnection) -> Result<()> {
    sqlx::raw_sql(safe!(
        "CREATE TABLE IF NOT EXISTS {} (
           table_name TEXT PRIMARY KEY,
           collection TEXT NOT NULL,
           cursor JSONB,
           rows_loaded BIGINT NOT NULL DEFAULT 0,
           completed BOOLEAN NOT NULL DEFAULT FALSE,
           updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
         )",
        q(CHECKPOINTS)
    ))
    .execute(conn)
    .await?;
    Ok(())
}

pub struct Checkpoint {
    pub cursor: Option<Value>,
    pub rows_loaded: i64,
    pub completed: bool,
}

pub async fn checkpoint(conn: &mut PgConnection, table: &str) -> Result<Option<Checkpoint>> {
    let row = sqlx::query(safe!(
        "SELECT cursor::text, rows_loaded, completed FROM {} WHERE table_name = $1",
        q(CHECKPOINTS)
    ))
    .bind(table)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| Checkpoint {
        cursor: r
            .get::<Option<String>, _>(0)
            .and_then(|s| serde_json::from_str(&s).ok()),
        rows_loaded: r.get(1),
        completed: r.get(2),
    }))
}

pub async fn save_checkpoint(
    conn: &mut PgConnection,
    plan: &CollectionPlan,
    cursor: &Value,
    rows: i64,
    completed: bool,
) -> Result<()> {
    sqlx::query(safe!(
        "INSERT INTO {} (table_name, collection, cursor, rows_loaded, completed, updated_at)
         VALUES ($1, $2, $3::jsonb, $4, $5, now())
         ON CONFLICT (table_name) DO UPDATE
         SET cursor = EXCLUDED.cursor, rows_loaded = EXCLUDED.rows_loaded,
             completed = EXCLUDED.completed, updated_at = now()",
        q(CHECKPOINTS)
    ))
    .bind(&plan.table)
    .bind(&plan.collection)
    .bind(cursor.to_string())
    .bind(rows)
    .bind(completed)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn clear_checkpoint(conn: &mut PgConnection, table: &str) -> Result<()> {
    sqlx::query(safe!(
        "DELETE FROM {} WHERE table_name = $1",
        q(CHECKPOINTS)
    ))
    .bind(table)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn table_exists(conn: &mut PgConnection, table: &str) -> Result<bool> {
    let row = sqlx::query("SELECT to_regclass($1) IS NOT NULL")
        .bind(q(table))
        .fetch_one(conn)
        .await?;
    Ok(row.get(0))
}

pub async fn has_rows(conn: &mut PgConnection, table: &str) -> Result<bool> {
    let row = sqlx::query(safe!("SELECT EXISTS (SELECT 1 FROM {})", q(table)))
        .fetch_one(conn)
        .await?;
    Ok(row.get(0))
}

pub async fn count(conn: &mut PgConnection, table: &str) -> Result<u64> {
    let row = sqlx::query(safe!("SELECT count(*) FROM {}", q(table)))
        .fetch_one(conn)
        .await?;
    Ok(row.get::<i64, _>(0) as u64)
}

pub async fn execute(conn: &mut PgConnection, sql: &str) -> Result<()> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
        .execute(conn)
        .await
        .with_context(|| format!("failed: {}", first_line(sql)))?;
    Ok(())
}

fn first_line(sql: &str) -> &str {
    sql.lines().next().unwrap_or(sql)
}

/// Stream a COPY text payload into `table (columns…)`.
pub async fn copy_in(
    conn: &mut PgConnection,
    table: &str,
    columns: &[String],
    payload: &str,
) -> Result<()> {
    if payload.is_empty() {
        return Ok(());
    }
    let cols = columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
    let mut copy = conn
        .copy_in_raw(&format!("COPY {} ({cols}) FROM STDIN", q(table)))
        .await?;
    // Send in modest chunks; one huge message is slower and holds more memory.
    for chunk in payload.as_bytes().chunks(1 << 20) {
        if let Err(e) = copy.send(chunk).await {
            let _ = copy.abort("send failed").await;
            return Err(e.into());
        }
    }
    copy.finish()
        .await
        .with_context(|| format!("COPY into {table} failed"))?;
    Ok(())
}

/// One target row as JSON, looked up by primary key (cast from text, so any key type works).
pub async fn row_json(
    conn: &mut PgConnection,
    table: &str,
    pk_col: &str,
    pk_type: &str,
    pk: &str,
) -> Result<Option<serde_json::Map<String, Value>>> {
    let sql = format!(
        "SELECT row_to_json(t)::text FROM {} t WHERE {} = $1::text::{pk_type}",
        q(table),
        q(pk_col)
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(pk)
        .fetch_optional(conn)
        .await?;
    Ok(row
        .and_then(|r| serde_json::from_str::<Value>(&r.get::<String, _>(0)).ok())
        .and_then(|v| match v {
            Value::Object(o) => Some(o),
            _ => None,
        }))
}

/// A parent's child rows as JSON, in array order.
pub async fn child_rows_json(
    conn: &mut PgConnection,
    table: &str,
    fk_col: &str,
    pk_type: &str,
    pk: &str,
) -> Result<Vec<serde_json::Map<String, Value>>> {
    let sql = format!(
        "SELECT row_to_json(c)::text FROM {} c WHERE {} = $1::text::{pk_type} ORDER BY {}",
        q(table),
        q(fk_col),
        q("array_index")
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(pk)
        .fetch_all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |r| match serde_json::from_str::<Value>(&r.get::<String, _>(0)) {
                Ok(Value::Object(o)) => Some(o),
                _ => None,
            },
        )
        .collect())
}
