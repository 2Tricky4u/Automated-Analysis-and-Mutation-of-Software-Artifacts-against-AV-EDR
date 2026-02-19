//! JSON Value field extraction helpers for proto mapping.
//!
//! Safe accessors that return sensible defaults for missing/wrong-typed fields.
//! Used by API handlers to convert ES `_source` documents into proto responses.

use serde_json::Value;

pub fn str_field(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}

pub fn u32_field(v: &Value, key: &str) -> u32 {
    v[key].as_u64().unwrap_or(0) as u32
}

pub fn bool_field(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}

pub fn f64_field(v: &Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or(0.0)
}

pub fn i32_field(v: &Value, key: &str) -> i32 {
    v[key].as_i64().unwrap_or(0) as i32
}

pub fn u64_field(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

pub fn string_array_field(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a date value that may be RFC3339 string (new format) or unix seconds (old format).
pub fn parse_date_to_unix_secs(v: &Value) -> i64 {
    if let Some(s) = v.as_str()
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s)
    {
        return dt.timestamp();
    }
    v.as_i64().unwrap_or(0)
}
