//! Source types, target dialects, SQL type maps, and DDL.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "camelCase")]
pub enum MongoType {
    ObjectId,
    String,
    Int,
    Double,
    Boolean,
    Date,
    Array,
    Object,
    Null,
    Binary,
    Unknown,
}

impl MongoType {
    pub fn as_str(self) -> &'static str {
        match self {
            MongoType::ObjectId => "objectId",
            MongoType::String => "string",
            MongoType::Int => "int",
            MongoType::Double => "double",
            MongoType::Boolean => "boolean",
            MongoType::Date => "date",
            MongoType::Array => "array",
            MongoType::Object => "object",
            MongoType::Null => "null",
            MongoType::Binary => "binary",
            MongoType::Unknown => "unknown",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Postgresql,
    Mysql,
    Mariadb,
    Bigquery,
    Snowflake,
    Redshift,
    Clickhouse,
    Mssql,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FieldMapping {
    /// Dot path in the source document, e.g. `address.city`.
    pub source_field: String,
    pub target_column: String,
    /// Every type observed for this field.
    pub mongo_types: Vec<MongoType>,
    pub resolved_mongo_type: MongoType,
    pub sql_type: String,
    pub nullable: bool,
    pub is_primary_key: bool,
    #[serde(default)]
    pub sample_values: Vec<Value>,
    /// 0–1, how often this field was null or missing in the sample.
    pub null_fraction: f64,
}

/// Priority when a field holds several scalar types (highest wins).
const TYPE_PRIORITY: [MongoType; 11] = [
    MongoType::Array,
    MongoType::Object,
    MongoType::Binary,
    MongoType::String,
    MongoType::Double,
    MongoType::Int,
    MongoType::Boolean,
    MongoType::Date,
    MongoType::ObjectId,
    MongoType::Null,
    MongoType::Unknown,
];

pub fn resolve_conflicting_types(types: &[MongoType]) -> MongoType {
    let mut unique: Vec<MongoType> = Vec::new();
    for t in types {
        if *t != MongoType::Null && !unique.contains(t) {
            unique.push(*t);
        }
    }
    match unique.len() {
        0 => return MongoType::Null,
        1 => return unique[0],
        _ => {}
    }
    if unique
        .iter()
        .all(|t| matches!(t, MongoType::Int | MongoType::Double))
    {
        return MongoType::Double;
    }
    if unique.contains(&MongoType::Array) || unique.contains(&MongoType::Object) {
        return if unique.iter().all(|t| *t == MongoType::Array) {
            MongoType::Array
        } else {
            MongoType::Object
        };
    }
    for p in TYPE_PRIORITY {
        if p != MongoType::Null && unique.contains(&p) {
            return p;
        }
    }
    MongoType::String
}

pub fn to_sql_type(t: MongoType, dialect: Dialect) -> &'static str {
    use MongoType as M;
    match dialect {
        Dialect::Postgresql => match t {
            M::ObjectId => "VARCHAR(24)",
            M::String => "TEXT",
            M::Int => "INTEGER",
            M::Double => "DOUBLE PRECISION",
            M::Boolean => "BOOLEAN",
            M::Date => "TIMESTAMPTZ",
            M::Array | M::Object => "JSONB",
            M::Binary => "BYTEA",
            M::Null | M::Unknown => "TEXT",
        },
        Dialect::Mysql | Dialect::Mariadb => match t {
            M::ObjectId => "VARCHAR(24)",
            M::String => "TEXT",
            M::Int => "INT",
            M::Double => "DOUBLE",
            M::Boolean => "TINYINT(1)",
            M::Date => "DATETIME",
            M::Array | M::Object => "JSON",
            M::Binary => "LONGBLOB",
            M::Null | M::Unknown => "TEXT",
        },
        Dialect::Bigquery => match t {
            M::ObjectId | M::String | M::Null | M::Unknown => "STRING",
            M::Int => "INT64",
            M::Double => "FLOAT64",
            M::Boolean => "BOOL",
            M::Date => "TIMESTAMP",
            M::Array | M::Object => "JSON",
            M::Binary => "BYTES",
        },
        Dialect::Snowflake => match t {
            M::ObjectId => "VARCHAR(24)",
            M::String | M::Null | M::Unknown => "TEXT",
            M::Int => "NUMBER",
            M::Double => "FLOAT",
            M::Boolean => "BOOLEAN",
            M::Date => "TIMESTAMP_TZ",
            M::Array | M::Object => "VARIANT",
            M::Binary => "BINARY",
        },
        Dialect::Redshift => match t {
            M::ObjectId => "VARCHAR(24)",
            M::String | M::Null | M::Unknown => "VARCHAR(MAX)",
            M::Int => "INTEGER",
            M::Double => "DOUBLE PRECISION",
            M::Boolean => "BOOLEAN",
            M::Date => "TIMESTAMPTZ",
            M::Array | M::Object => "SUPER",
            M::Binary => "VARBYTE",
        },
        Dialect::Clickhouse => match t {
            M::Int => "Int64",
            M::Double => "Float64",
            M::Boolean => "Bool",
            M::Date => "DateTime64(3, 'UTC')",
            _ => "String",
        },
        Dialect::Mssql => match t {
            M::ObjectId => "NVARCHAR(24)",
            M::String | M::Array | M::Object | M::Null | M::Unknown => "NVARCHAR(MAX)",
            M::Int => "INT",
            M::Double => "FLOAT",
            M::Boolean => "BIT",
            M::Date => "DATETIME2",
            M::Binary => "VARBINARY(MAX)",
        },
    }
}

/// The 64-bit integer type, for dialects whose default integer is 32-bit.
///
/// A MongoDB `Int64` (`$numberLong`) above 2³¹ does not fit `INTEGER`; the
/// whole COPY batch would fail on the first such value.
pub fn wide_int_type(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Postgresql
        | Dialect::Mysql
        | Dialect::Mariadb
        | Dialect::Redshift
        | Dialect::Mssql => "BIGINT",
        other => to_sql_type(MongoType::Int, other),
    }
}

static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());
static ACRONYM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").unwrap());
static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9_]").unwrap());
static LEADING_DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9])").unwrap());

/// MongoDB field name → safe snake_case SQL column name.
/// `createdAt` → `created_at`, `userID` → `user_id`, `2fa` → `_2fa`.
pub fn to_column_name(field: &str) -> String {
    let s = CAMEL.replace_all(field, "${1}_${2}");
    let s = ACRONYM.replace_all(&s, "${1}_${2}");
    let s = UNSAFE.replace_all(&s, "_");
    let s = LEADING_DIGIT.replace(&s, "_${1}");
    s.to_lowercase()
}

/// Quote an identifier the way each target's writer does.
pub fn quote_ident(name: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::Mysql | Dialect::Mariadb | Dialect::Clickhouse | Dialect::Bigquery => {
            format!("`{}`", name.replace('`', "``"))
        }
        Dialect::Mssql => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Column definitions for a `CREATE TABLE` body.
pub fn column_definitions(mappings: &[FieldMapping], dialect: Dialect) -> Vec<String> {
    mappings
        .iter()
        .map(|m| {
            let nn = if m.nullable && !m.is_primary_key {
                ""
            } else {
                " NOT NULL"
            };
            let pk = if m.is_primary_key { " PRIMARY KEY" } else { "" };
            format!(
                "  {} {}{}{}",
                quote_ident(&m.target_column, dialect),
                m.sql_type,
                nn,
                pk
            )
        })
        .collect()
}

#[derive(Default, Clone, Copy)]
pub struct DdlOptions {
    pub if_not_exists: bool,
}

/// `CREATE TABLE` from field mappings.
pub fn generate_ddl(table: &str, mappings: &[FieldMapping], dialect: Dialect) -> String {
    generate_ddl_with(table, mappings, dialect, DdlOptions::default())
}

pub fn generate_ddl_with(
    table: &str,
    mappings: &[FieldMapping],
    dialect: Dialect,
    opts: DdlOptions,
) -> String {
    let ine = if opts.if_not_exists {
        "IF NOT EXISTS "
    } else {
        ""
    };
    format!(
        "CREATE TABLE {}{} (\n{}\n);",
        ine,
        quote_ident(table, dialect),
        column_definitions(mappings, dialect).join(",\n")
    )
}

/// Re-detecting a schema must not undo a user's edits: fields already in a
/// saved plan keep their saved column name and SQL type.
pub fn keep_saved_columns(fields: Vec<FieldMapping>, saved: &[FieldMapping]) -> Vec<FieldMapping> {
    fields
        .into_iter()
        .map(|mut f| {
            if let Some(s) = saved.iter().find(|s| s.source_field == f.source_field) {
                f.target_column = s.target_column.clone();
                f.sql_type = s.sql_type.clone();
            }
            f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use MongoType as M;

    #[test]
    fn column_names() {
        assert_eq!(to_column_name("createdAt"), "created_at");
        assert_eq!(to_column_name("userID"), "user_id");
        assert_eq!(to_column_name("HTTPServer"), "http_server");
        assert_eq!(to_column_name("address.city"), "address_city");
        assert_eq!(to_column_name("2fa"), "_2fa");
        assert_eq!(to_column_name("first name"), "first_name");
    }

    #[test]
    fn conflicts() {
        assert_eq!(resolve_conflicting_types(&[M::Int, M::Double]), M::Double);
        assert_eq!(resolve_conflicting_types(&[M::Int, M::String]), M::String);
        assert_eq!(resolve_conflicting_types(&[M::Array, M::Array]), M::Array);
        assert_eq!(resolve_conflicting_types(&[M::Array, M::Object]), M::Object);
        assert_eq!(resolve_conflicting_types(&[M::Array, M::String]), M::Object);
        assert_eq!(resolve_conflicting_types(&[M::Null, M::Null]), M::Null);
        assert_eq!(
            resolve_conflicting_types(&[M::Boolean, M::Date]),
            M::Boolean
        );
        assert_eq!(
            resolve_conflicting_types(&[M::ObjectId, M::String]),
            M::String
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_ident("order", Dialect::Postgresql), "\"order\"");
        assert_eq!(quote_ident("a\"b", Dialect::Postgresql), "\"a\"\"b\"");
        assert_eq!(quote_ident("order", Dialect::Mysql), "`order`");
        assert_eq!(quote_ident("order", Dialect::Mssql), "[order]");
    }
}
