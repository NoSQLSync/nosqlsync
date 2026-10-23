//! Where documents come from: a MongoDB deployment, or JSON / NDJSON files.
//!
//! Both sources hand documents to the engine as Extended JSON values. MongoDB
//! documents are converted to *canonical* Extended JSON, so every BSON type is
//! explicit; files are taken as they are and sniffed ([`Detect::Json`]).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use futures::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use nosqlsync_core::ejson::{parse_documents, unmarshal_dynamodb, Detect};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub enum Source {
    Mongo { db: mongodb::Database },
    Files { files: Vec<PathBuf> },
}

/// A run of documents plus the position to resume after them.
pub struct Batch {
    pub docs: Vec<Value>,
    pub cursor: Value,
}

impl Source {
    /// `mongodb://…` / `mongodb+srv://…`, or a path to a `.json`/`.ndjson`/`.jsonl`
    /// file or a directory of them.
    pub async fn open(spec: &str, database: Option<&str>) -> Result<Source> {
        if spec.starts_with("mongodb://") || spec.starts_with("mongodb+srv://") {
            let client = mongodb::Client::with_uri_str(spec)
                .await
                .context("could not parse the MongoDB connection string")?;
            let db = match database {
                Some(name) => client.database(name),
                None => client
                    .default_database()
                    .context("no database in the connection string; pass --db <name>")?,
            };
            db.run_command(doc! { "ping": 1 })
                .await
                .context("could not reach MongoDB")?;
            return Ok(Source::Mongo { db });
        }
        let path = Path::new(spec);
        let files = if path.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(path)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("json" | "ndjson" | "jsonl")
                    )
                })
                .collect();
            v.sort();
            v
        } else if path.is_file() {
            vec![path.to_path_buf()]
        } else {
            bail!("source `{spec}` is neither a mongodb:// URI nor an existing file or directory");
        };
        if files.is_empty() {
            bail!("no .json, .ndjson or .jsonl files in {spec}");
        }
        Ok(Source::Files { files })
    }

    pub fn detect_mode(&self) -> Detect {
        match self {
            Source::Mongo { .. } => Detect::Bson,
            Source::Files { .. } => Detect::Json,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Mongo { db } => format!("MongoDB database `{}`", db.name()),
            Source::Files { files } => format!("{} file(s)", files.len()),
        }
    }

    pub async fn collections(&self, only: &[String]) -> Result<Vec<String>> {
        let mut all = match self {
            Source::Mongo { db } => {
                let mut names = db
                    .list_collection_names()
                    .filter(doc! { "type": "collection" })
                    .await?;
                names.retain(|n| !n.starts_with("system."));
                names.sort();
                names
            }
            Source::Files { files } => files.iter().map(|f| collection_name(f)).collect(),
        };
        if !only.is_empty() {
            for name in only {
                if !all.contains(name) {
                    bail!("collection `{name}` not found (have: {})", all.join(", "));
                }
            }
            all.retain(|n| only.contains(n));
        }
        Ok(all)
    }

    fn file(&self, collection: &str) -> Result<&Path> {
        match self {
            Source::Files { files } => files
                .iter()
                .find(|f| collection_name(f) == collection)
                .map(PathBuf::as_path)
                .with_context(|| format!("no file for collection `{collection}`")),
            Source::Mongo { .. } => unreachable!(),
        }
    }

    /// Up to `n` documents (0 = all). Random for MongoDB (`$sample`), the first
    /// `n` for files.
    pub async fn sample(&self, collection: &str, n: usize) -> Result<Vec<Value>> {
        match self {
            Source::Mongo { db } => {
                let coll = db.collection::<Document>(collection);
                let cursor = if n == 0 {
                    coll.find(doc! {}).await?
                } else {
                    coll.aggregate(vec![doc! { "$sample": { "size": n as i64 } }])
                        .await?
                };
                let docs: Vec<Document> = cursor.try_collect().await?;
                Ok(docs.into_iter().map(to_ejson).collect())
            }
            Source::Files { .. } => {
                let path = self.file(collection)?.to_path_buf();
                tokio::task::spawn_blocking(move || {
                    let mut out = Vec::new();
                    for_each_file_doc(&path, |doc| {
                        out.push(doc);
                        n == 0 || out.len() < n
                    })?;
                    Ok(out)
                })
                .await?
            }
        }
    }

    /// A spread-out sample for validation: random for MongoDB, evenly spaced through the file for files.
    pub async fn spread_sample(
        &self,
        collection: &str,
        n: usize,
        total: u64,
    ) -> Result<Vec<Value>> {
        match self {
            Source::Mongo { .. } => self.sample(collection, n).await,
            Source::Files { .. } => {
                let path = self.file(collection)?.to_path_buf();
                // A fractional step, so 200 of 300 documents spans the whole
                // file rather than stopping at document 200.
                let step = (total as f64 / n.max(1) as f64).max(1.0);
                tokio::task::spawn_blocking(move || {
                    let mut out = Vec::new();
                    let mut i = 0usize;
                    let mut next = 0.0f64;
                    for_each_file_doc(&path, |doc| {
                        if i as f64 >= next {
                            out.push(doc);
                            next += step;
                        }
                        i += 1;
                        out.len() < n
                    })?;
                    Ok(out)
                })
                .await?
            }
        }
    }

    /// Exact document count.
    pub async fn count(&self, collection: &str) -> Result<u64> {
        match self {
            Source::Mongo { db } => Ok(db
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await?),
            Source::Files { .. } => {
                let path = self.file(collection)?.to_path_buf();
                tokio::task::spawn_blocking(move || {
                    let mut n = 0u64;
                    for_each_file_doc(&path, |_| {
                        n += 1;
                        true
                    })?;
                    Ok(n)
                })
                .await?
            }
        }
    }

    /// A cheap count for progress display; may be approximate.
    pub async fn estimated_count(&self, collection: &str) -> Option<u64> {
        match self {
            Source::Mongo { db } => db
                .collection::<Document>(collection)
                .estimated_document_count()
                .await
                .ok(),
            Source::Files { .. } => None,
        }
    }

    /// Stream every document in a stable order, in batches, starting after `from`.
    ///
    /// MongoDB is read in `_id` order, so the cursor is the last `_id` loaded
    /// and a restart continues with `{_id: {$gt: …}}` — nothing replayed,
    /// nothing skipped. Files resume by document offset.
    pub async fn stream(
        &self,
        collection: &str,
        from: Option<Value>,
        batch: usize,
    ) -> Result<mpsc::Receiver<Result<Batch>>> {
        let (tx, rx) = mpsc::channel::<Result<Batch>>(2);
        match self {
            Source::Mongo { db } => {
                let coll = db.collection::<Document>(collection);
                let filter = match from {
                    Some(last) => {
                        let id = Bson::try_from(last["_id"].clone())
                            .context("bad resume cursor in checkpoint")?;
                        doc! { "_id": { "$gt": id } }
                    }
                    None => doc! {},
                };
                let mut cursor = coll
                    .find(filter)
                    .sort(doc! { "_id": 1 })
                    .batch_size(batch.min(10_000) as u32)
                    .await?;
                tokio::spawn(async move {
                    let mut docs = Vec::with_capacity(batch);
                    let mut last_id = Value::Null;
                    loop {
                        match cursor.try_next().await {
                            Ok(Some(d)) => {
                                last_id = d
                                    .get("_id")
                                    .cloned()
                                    .map(Bson::into_canonical_extjson)
                                    .unwrap_or(Value::Null);
                                docs.push(to_ejson(d));
                                if docs.len() >= batch {
                                    let b = Batch {
                                        docs: std::mem::take(&mut docs),
                                        cursor: json!({ "_id": last_id }),
                                    };
                                    if tx.send(Ok(b)).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                let _ = tx.send(Err(e.into())).await;
                                return;
                            }
                        }
                    }
                    if !docs.is_empty() {
                        let _ = tx
                            .send(Ok(Batch {
                                docs,
                                cursor: json!({ "_id": last_id }),
                            }))
                            .await;
                    }
                });
            }
            Source::Files { .. } => {
                let path = self.file(collection)?.to_path_buf();
                // An offset only means something in the file it was taken from.
                // Resuming into an edited file would skip or repeat documents,
                // so the cursor carries the file's fingerprint and must match.
                let fingerprint = file_fingerprint(&path)?;
                let skip = match &from {
                    Some(c) if c["file"] != fingerprint => bail!(
                        "{} changed since the interrupted load, so its saved position no longer applies. Re-run with --fresh.",
                        path.display()
                    ),
                    Some(c) => c["offset"].as_u64().unwrap_or(0),
                    None => 0,
                };
                let cursor_at = move |offset: u64| json!({ "offset": offset, "file": fingerprint });
                tokio::task::spawn_blocking(move || {
                    let mut docs = Vec::with_capacity(batch);
                    let mut offset = 0u64;
                    let result = for_each_file_doc(&path, |doc| {
                        offset += 1;
                        if offset <= skip {
                            return true;
                        }
                        docs.push(doc);
                        if docs.len() >= batch {
                            let b = Batch {
                                docs: std::mem::take(&mut docs),
                                cursor: cursor_at(offset),
                            };
                            return tx.blocking_send(Ok(b)).is_ok();
                        }
                        true
                    });
                    match result {
                        Err(e) => {
                            let _ = tx.blocking_send(Err(e));
                        }
                        Ok(()) if !docs.is_empty() => {
                            let _ = tx.blocking_send(Ok(Batch {
                                docs,
                                cursor: cursor_at(offset),
                            }));
                        }
                        Ok(()) => {}
                    }
                });
            }
        }
        Ok(rx)
    }
}

fn to_ejson(d: Document) -> Value {
    Bson::Document(d).into_canonical_extjson()
}

/// Size and modification time: enough to notice an edited or replaced file.
fn file_fingerprint(path: &Path) -> Result<Value> {
    let meta = std::fs::metadata(path)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string());
    Ok(json!({ "size": meta.len(), "modified": modified }))
}

pub fn collection_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("collection")
        .to_string()
}

/// Call `f` for each document in a file until it returns false. A JSON array
/// file is parsed whole; anything else is read line by line, so a multi-GB
/// NDJSON export never has to fit in memory.
fn for_each_file_doc(path: &Path, mut f: impl FnMut(Value) -> bool) -> Result<()> {
    let file =
        std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let first = loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(());
        }
        match buf.iter().position(|b| !b.is_ascii_whitespace()) {
            Some(i) => {
                let c = buf[i];
                reader.consume(i);
                break c;
            }
            None => {
                let n = buf.len();
                reader.consume(n);
            }
        }
    };
    // A pretty-printed single object spans lines, so it is only NDJSON if the
    // first line is a complete document on its own.
    let mut head = String::new();
    if first == b'{' {
        reader.read_line(&mut head)?;
    }
    if first == b'[' || (first == b'{' && serde_json::from_str::<Value>(head.trim()).is_err()) {
        let mut text = head;
        reader.read_to_string(&mut text)?;
        let docs =
            parse_documents(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        for d in docs {
            if !f(d) {
                break;
            }
        }
        return Ok(());
    }
    let lines = std::iter::once(Ok(head)).chain(reader.lines());
    for (i, line) in lines.enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).with_context(|| {
            format!(
                "{} line {}: not valid JSON (expected one object per line)",
                path.display(),
                i + 1
            )
        })?;
        let v = unmarshal_dynamodb(v);
        if !v.is_object() {
            bail!(
                "{} line {}: each document must be a JSON object",
                path.display(),
                i + 1
            );
        }
        if !f(v) {
            break;
        }
    }
    Ok(())
}
