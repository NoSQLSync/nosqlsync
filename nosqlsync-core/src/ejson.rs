//! Documents as MongoDB Extended JSON: type detection, unwrapping, and input parsing.
//!
//! Two kinds of input reach the engine, and they need different detection:
//!
//! - **From a driver**, documents arrive as *canonical* Extended JSON, where
//!   every non-JSON type is wrapped (`{"$oid": …}`, `{"$numberLong": …}`). The
//!   wrapper is the truth, so a plain string is always a string.
//! - **From a file or a paste** (`mongoexport`, an API dump), wrappers may be
//!   missing: `mongoexport` without `--jsonArray` writes dates and ObjectIds as
//!   bare strings. Sniffing those back is what the TypeScript `detectJsonType`
//!   does, and [`Detect::Json`] matches it exactly.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::types::MongoType;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
/// How to detect special types in a document (see the module docs).
pub enum Detect {
    /// Canonical Extended JSON from a driver: only wrappers mark special types.
    Bson,
    /// Loose JSON: also treat ISO-8601 strings as dates and 24-hex strings as ObjectIds.
    Json,
}

static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$").unwrap()
});
static OBJECT_ID_HEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)[0-9a-f]{24}$").unwrap());

/// The type an Extended JSON wrapper such as `{"$oid": "…"}` stands for.
pub fn wrapper_type(obj: &Map<String, Value>) -> Option<MongoType> {
    if obj.len() != 1 {
        return None;
    }
    match obj.keys().next().map(String::as_str) {
        Some("$oid") => Some(MongoType::ObjectId),
        Some("$date") => Some(MongoType::Date),
        Some("$numberInt") | Some("$numberLong") => Some(MongoType::Int),
        Some("$numberDouble") | Some("$numberDecimal") => Some(MongoType::Double),
        Some("$binary") | Some("$uuid") => Some(MongoType::Binary),
        // Not in the TypeScript tool, which only meets these via a driver. A
        // BSON Timestamp is a point in time; the rest are opaque scalars.
        // Either way they must never be mistaken for an embedded object and
        // flattened into `seen_at__timestamp_t` columns.
        Some("$timestamp") => Some(MongoType::Date),
        Some("$regularExpression" | "$symbol" | "$code" | "$dbPointer" | "$minKey" | "$maxKey") => {
            Some(MongoType::String)
        }
        _ => None,
    }
}

/// Is this an Extended JSON Decimal128?
pub fn is_decimal(value: &Value) -> bool {
    matches!(value, Value::Object(o) if o.len() == 1 && o.contains_key("$numberDecimal"))
}

/// Is this an Extended JSON Int64 (`$numberLong`)?
pub fn is_int64(value: &Value) -> bool {
    matches!(value, Value::Object(o) if o.len() == 1 && o.contains_key("$numberLong"))
}

/// The UUID in an Extended JSON value, as `xxxxxxxx-xxxx-…`: a `$uuid`
/// wrapper, or `$binary` with subtype 04 (how drivers write UUIDs).
pub fn as_uuid(value: &Value) -> Option<String> {
    use base64::Engine;
    let Value::Object(o) = value else { return None };
    if let Some(Value::String(s)) = o.get("$uuid") {
        return Some(s.to_lowercase());
    }
    let b = o.get("$binary")?;
    if b.get("subType").and_then(Value::as_str) != Some("04") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b.get("base64")?.as_str()?)
        .ok()?;
    if bytes.len() != 16 {
        return None;
    }
    let h: String = bytes.iter().map(|x| format!("{x:02x}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

/// Is this number an integer the way JavaScript's `Number.isInteger` sees it?
/// `2.0` parsed by `JSON.parse` is just `2`, so it counts.
pub fn is_js_integer(n: &Number) -> bool {
    if n.is_i64() || n.is_u64() {
        return true;
    }
    n.as_f64()
        .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

/// The [`MongoType`] of one value. An object is its wrapper's type if it is an
/// Extended JSON wrapper (see [`wrapper_type`]), otherwise `Object`.
pub fn detect(value: &Value, mode: Detect) -> MongoType {
    match value {
        Value::Null => MongoType::Null,
        Value::Array(_) => MongoType::Array,
        Value::Bool(_) => MongoType::Boolean,
        Value::Number(n) => {
            if is_js_integer(n) {
                MongoType::Int
            } else {
                MongoType::Double
            }
        }
        Value::String(s) => {
            if mode == Detect::Json {
                if ISO_DATE.is_match(s) {
                    return MongoType::Date;
                }
                if OBJECT_ID_HEX.is_match(s) {
                    return MongoType::ObjectId;
                }
            }
            MongoType::String
        }
        Value::Object(obj) => wrapper_type(obj).unwrap_or(MongoType::Object),
    }
}

/// A short, human-readable sample value, as the TypeScript `serializeJsonSample` produces.
pub fn sample_value(value: &Value) -> Value {
    match value {
        Value::Array(a) => Value::String(format!("[array({})]", a.len())),
        Value::Object(obj) => {
            if wrapper_type(obj).is_some() {
                unwrap_value(value)
            } else {
                Value::String("{object}".into())
            }
        }
        other => other.clone(),
    }
}

/// Unwrap one Extended JSON scalar to the plain JSON a person would expect to
/// see in a JSONB column: ObjectId → hex string, date → ISO string, numbers →
/// numbers. Anything unrecognised is returned unchanged.
pub fn unwrap_value(value: &Value) -> Value {
    let Value::Object(obj) = value else {
        return value.clone();
    };
    if obj.len() != 1 {
        return value.clone();
    }
    let (k, v) = obj.iter().next().unwrap();
    match (k.as_str(), v) {
        ("$oid", Value::String(_)) => v.clone(),
        ("$date", Value::String(_)) => v.clone(),
        ("$date", Value::Number(n)) => n
            .as_i64()
            .and_then(millis_to_iso)
            .map(Value::String)
            .unwrap_or_else(|| value.clone()),
        ("$date", Value::Object(inner)) => match inner.get("$numberLong") {
            Some(Value::String(ms)) => ms
                .parse::<i64>()
                .ok()
                .and_then(millis_to_iso)
                .map(Value::String)
                .unwrap_or_else(|| value.clone()),
            _ => value.clone(),
        },
        ("$timestamp", Value::Object(ts)) => ts
            .get("t")
            .and_then(Value::as_i64)
            .and_then(|t| millis_to_iso(t * 1000))
            .map(Value::String)
            .unwrap_or_else(|| value.clone()),
        ("$binary", _) | ("$uuid", _) if as_uuid(value).is_some() => {
            Value::String(as_uuid(value).unwrap())
        }
        ("$numberInt" | "$numberLong", Value::String(s)) => s
            .parse::<i64>()
            .map(|i| Value::Number(i.into()))
            .unwrap_or_else(|_| value.clone()),
        ("$numberDouble", Value::String(s)) => s
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| v.clone()),
        // Decimal128 keeps its exact digits as a string, as the JS driver's toJSON does.
        ("$numberDecimal", Value::String(_)) => v.clone(),
        _ => value.clone(),
    }
}

/// [`unwrap_value`] applied recursively — the representation written to JSON columns.
pub fn plain_json(value: &Value) -> Value {
    match value {
        Value::Array(a) => Value::Array(a.iter().map(plain_json).collect()),
        Value::Object(obj) => {
            if wrapper_type(obj).is_some() {
                return unwrap_value(value);
            }
            Value::Object(
                obj.iter()
                    .map(|(k, v)| (k.clone(), plain_json(v)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Epoch milliseconds → `YYYY-MM-DDTHH:MM:SS.mmmZ`, or `None` if out of chrono's range.
pub fn millis_to_iso(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Accept what people actually have: a JSON array, one object, or
/// newline-delimited objects (what `mongoexport` and DynamoDB's S3 export emit).
pub fn parse_documents(text: &str) -> Result<Vec<Value>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("No documents found.".into());
    }
    // Only a JSON *syntax* error falls through to NDJSON; a well-formed value
    // of the wrong shape is a real error.
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        return match parsed {
            Value::Array(a) if a.is_empty() => Err("That array is empty.".into()),
            Value::Array(a) => a.into_iter().map(assert_object).collect(),
            other => Ok(vec![assert_object(other)?]),
        };
    }
    let mut docs = Vec::new();
    for (i, line) in trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .enumerate()
    {
        let v: Value = serde_json::from_str(line).map_err(|_| {
            format!(
                "Line {} is not valid JSON. Use a JSON array, one object, or one object per line.",
                i + 1
            )
        })?;
        docs.push(assert_object(v)?);
    }
    if docs.is_empty() {
        return Err("No documents found.".into());
    }
    Ok(docs)
}

fn assert_object(v: Value) -> Result<Value, String> {
    let v = unmarshal_dynamodb(v);
    if v.is_object() {
        Ok(v)
    } else {
        Err("Each document must be a JSON object.".into())
    }
}

// --- DynamoDB JSON ------------------------------------------------------------

/// DynamoDB exports (S3 export, `aws dynamodb scan`) describe every attribute
/// with a type tag: `{"Item": {"name": {"S": "Ada"}, "age": {"N": "36"}}}`.
/// Turn that into an ordinary document; anything else passes through untouched.
pub fn unmarshal_dynamodb(doc: Value) -> Value {
    let item = match &doc {
        Value::Object(o) if o.len() == 1 && o.contains_key("Item") => o.get("Item").cloned(),
        _ => None,
    };
    let candidate = item.as_ref().unwrap_or(&doc);
    let Value::Object(attrs) = candidate else {
        return doc;
    };
    if attrs.is_empty() || !attrs.values().all(is_dynamo_attribute) {
        return doc;
    }
    Value::Object(
        attrs
            .iter()
            .map(|(k, v)| (k.clone(), dynamo_value(v)))
            .collect(),
    )
}

fn is_dynamo_attribute(v: &Value) -> bool {
    match v {
        Value::Object(o) if o.len() == 1 => matches!(
            o.keys().next().map(String::as_str),
            Some("S" | "N" | "BOOL" | "NULL" | "M" | "L" | "SS" | "NS" | "BS" | "B")
        ),
        _ => false,
    }
}

fn dynamo_number(s: &str) -> Value {
    if let Ok(i) = s.parse::<i64>() {
        return Value::Number(i.into());
    }
    s.parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map(Value::Number)
        .unwrap_or_else(|| Value::String(s.to_string()))
}

fn dynamo_value(v: &Value) -> Value {
    let Value::Object(o) = v else {
        return v.clone();
    };
    let Some((tag, inner)) = o.iter().next() else {
        return v.clone();
    };
    match (tag.as_str(), inner) {
        ("S", Value::String(s)) => Value::String(s.clone()),
        ("N", Value::String(s)) => dynamo_number(s),
        ("BOOL", b @ Value::Bool(_)) => b.clone(),
        ("NULL", _) => Value::Null,
        ("M", Value::Object(m)) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), dynamo_value(v)))
                .collect(),
        ),
        ("L", Value::Array(l)) => Value::Array(l.iter().map(dynamo_value).collect()),
        ("SS", Value::Array(l)) => Value::Array(l.clone()),
        ("NS", Value::Array(l)) => Value::Array(
            l.iter()
                .map(|n| n.as_str().map(dynamo_number).unwrap_or(Value::Null))
                .collect(),
        ),
        ("B", Value::String(b64)) => {
            serde_json::json!({ "$binary": { "base64": b64, "subType": "00" } })
        }
        ("BS", Value::Array(l)) => Value::Array(
            l.iter()
                .map(|b| serde_json::json!({ "$binary": { "base64": b, "subType": "00" } }))
                .collect(),
        ),
        _ => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_parse_exactly() {
        // The default serde_json parser reads this as 60.90422855224461 (one ULP off);
        // `float_roundtrip` must be on, or JSON sources load wrong values.
        let v: Value = serde_json::from_str("60.904228552244604").unwrap();
        assert_eq!(
            v.as_f64(),
            Some("60.904228552244604".parse::<f64>().unwrap())
        );
    }

    #[test]
    fn detects_like_the_browser_tool() {
        assert_eq!(detect(&json!(2.0), Detect::Json), MongoType::Int);
        assert_eq!(detect(&json!(2.5), Detect::Json), MongoType::Double);
        assert_eq!(
            detect(&json!("2024-01-02T03:04:05Z"), Detect::Json),
            MongoType::Date
        );
        assert_eq!(
            detect(&json!("2024-01-02T03:04:05Z"), Detect::Bson),
            MongoType::String
        );
        assert_eq!(
            detect(&json!("507f1f77bcf86cd799439011"), Detect::Json),
            MongoType::ObjectId
        );
        assert_eq!(
            detect(&json!({"$oid": "507f1f77bcf86cd799439011"}), Detect::Bson),
            MongoType::ObjectId
        );
        assert_eq!(
            detect(&json!({"$numberLong": "9"}), Detect::Bson),
            MongoType::Int
        );
        assert_eq!(
            detect(&json!({"$date": {"$numberLong": "0"}}), Detect::Bson),
            MongoType::Date
        );
        assert_eq!(detect(&json!({"a": 1}), Detect::Bson), MongoType::Object);
        assert_eq!(
            detect(&json!({"$timestamp": {"t": 1, "i": 1}}), Detect::Bson),
            MongoType::Date
        );
        assert_eq!(
            detect(
                &json!({"$regularExpression": {"pattern": "a", "options": ""}}),
                Detect::Bson
            ),
            MongoType::String
        );
    }

    #[test]
    fn uuids() {
        // UUID("00112233-4455-6677-8899-aabbccddeeff") as a driver writes it.
        let v = json!({"$binary": {"base64": "ABEiM0RVZneImaq7zN3u/w==", "subType": "04"}});
        assert_eq!(
            as_uuid(&v).as_deref(),
            Some("00112233-4455-6677-8899-aabbccddeeff")
        );
        assert_eq!(
            plain_json(&json!([v])),
            json!(["00112233-4455-6677-8899-aabbccddeeff"])
        );
        let not_uuid = json!({"$binary": {"base64": "3q2+7w==", "subType": "00"}});
        assert_eq!(as_uuid(&not_uuid), None);
    }

    #[test]
    fn plain_json_unwraps_nested() {
        let v = json!({"tags": [{"$oid": "507f1f77bcf86cd799439011"}], "at": {"$date": {"$numberLong": "0"}}, "n": {"$numberLong": "5"}});
        assert_eq!(
            plain_json(&v),
            json!({"tags": ["507f1f77bcf86cd799439011"], "at": "1970-01-01T00:00:00.000Z", "n": 5})
        );
    }

    #[test]
    fn parses_ndjson_and_arrays() {
        assert_eq!(parse_documents("{\"a\":1}\n{\"a\":2}\n").unwrap().len(), 2);
        assert_eq!(parse_documents("[{\"a\":1},{\"a\":2}]").unwrap().len(), 2);
        assert!(parse_documents("[1]").is_err());
        assert!(parse_documents("{\"a\":1}\nnope").is_err());
    }

    #[test]
    fn unmarshals_dynamodb_export_lines() {
        let doc = json!({"Item": {"id": {"S": "u1"}, "age": {"N": "36"}, "tags": {"SS": ["a"]}, "addr": {"M": {"zip": {"N": "10.5"}}}, "x": {"NULL": true}}});
        assert_eq!(
            unmarshal_dynamodb(doc),
            json!({"id": "u1", "age": 36, "tags": ["a"], "addr": {"zip": 10.5}, "x": null})
        );
        // Ordinary documents are left alone, even when a field is called "S".
        let plain = json!({"S": "x"});
        assert_eq!(unmarshal_dynamodb(plain.clone()), plain);
    }
}
