//! Deterministic structured digests of tool results (v1.1).
//!
//! Pure, no LLM, no I/O. Used by `history_compaction` for `tool`-role
//! messages whose rendered content is large AND structured (JSON), so the
//! conversation summary preserves SHAPE (which columns/fields exist) instead
//! of lossy NL prose. The line is intentionally partial — recall_history
//! (lossless, v1) recovers the full result verbatim.

use serde_json::Value;

const MAX_COLUMNS: usize = 12;
const SAMPLE_ROWS: usize = 2;
const SCAN_ROWS_FOR_COLUMNS: usize = 50;
const MAX_INLINE_FIELDS: usize = 6;
const FIELD_VALUE_CHARS: usize = 40;
const DIGEST_CEILING_CHARS: usize = 400;

/// Returns a one-line structured digest of a tool result if `content` is
/// recognizably structured JSON (object, array-of-objects, or scalar array);
/// otherwise `None`, in which case the caller falls back to the NL summary.
///
/// The returned line carries NO turn tag and NO recall hint — the caller in
/// `history_compaction` prepends `[Tn] TOOL:` and appends the recall cue.
pub fn digest_tool_result(content: &str) -> Option<String> {
    let v: Value = serde_json::from_str(content.trim()).ok()?;
    let digest = match &v {
        Value::Array(arr) => digest_array(arr)?,
        Value::Object(_) => digest_object(&v)?,
        // Bare scalars (number / string / bool) are not "structured".
        _ => return None,
    };
    Some(cap(&digest, DIGEST_CEILING_CHARS))
}

fn digest_array(arr: &[Value]) -> Option<String> {
    if arr.is_empty() {
        return None;
    }
    // Array of objects → tabular digest.
    if arr.iter().take(SCAN_ROWS_FOR_COLUMNS).all(Value::is_object) {
        let cols = collect_columns(arr);
        let mut s = format!("{} filas · cols: {}", arr.len(), join_capped(&cols));
        let sample = sample_rows(arr, &cols);
        if !sample.is_empty() {
            s.push_str(&format!(" · muestra: {}", sample.join("; ")));
        }
        for agg in numeric_aggregates(arr, &cols) {
            s.push_str(&format!(" · {agg}"));
        }
        return Some(s);
    }
    // Array of scalars (or mixed) → count + small sample.
    let sample: Vec<String> = arr.iter().take(5).map(scalar_str).collect();
    let more = if arr.len() > 5 { ", …" } else { "" };
    Some(format!(
        "{} elementos · muestra: [{}{}]",
        arr.len(),
        sample.join(", "),
        more
    ))
}

fn digest_object(v: &Value) -> Option<String> {
    let map = v.as_object()?;
    if map.is_empty() {
        return None;
    }
    let mut fields: Vec<String> = Vec::new();
    let mut inline: Vec<String> = Vec::new();
    let mut drill: Option<(String, &Vec<Value>)> = None;
    for (k, val) in map.iter() {
        match val {
            Value::Array(a) => {
                fields.push(format!("{k}[{}]", a.len()));
                let is_obj_array =
                    !a.is_empty() && a.iter().take(SCAN_ROWS_FOR_COLUMNS).all(Value::is_object);
                if is_obj_array {
                    let better = drill
                        .as_ref()
                        .map(|(_, prev)| a.len() > prev.len())
                        .unwrap_or(true);
                    if better {
                        drill = Some((k.clone(), a));
                    }
                }
            }
            Value::Object(o) => fields.push(format!("{k}{{{}}}", o.len())),
            scalar => {
                fields.push(k.clone());
                if inline.len() < MAX_INLINE_FIELDS {
                    inline.push(format!("{k}={}", scalar_str(scalar)));
                }
            }
        }
    }
    let mut s = format!("objeto · campos: {}", join_capped(&fields));
    if !inline.is_empty() {
        s.push_str(&format!(" · {}", inline.join(", ")));
    }
    if let Some((k, a)) = drill {
        let cols = collect_columns(a);
        s.push_str(&format!(" · {k}[{}] cols: {}", a.len(), join_capped(&cols)));
        for agg in numeric_aggregates(a, &cols) {
            s.push_str(&format!(" · {agg}"));
        }
    }
    Some(s)
}

/// Union of object keys across the first `SCAN_ROWS_FOR_COLUMNS` rows, in
/// first-seen order (deterministic).
fn collect_columns(arr: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for row in arr.iter().take(SCAN_ROWS_FOR_COLUMNS) {
        if let Value::Object(map) = row {
            for k in map.keys() {
                if !seen.iter().any(|s| s == k) {
                    seen.push(k.clone());
                }
            }
        }
    }
    seen
}

fn join_capped(cols: &[String]) -> String {
    if cols.len() <= MAX_COLUMNS {
        cols.join(", ")
    } else {
        let shown = cols[..MAX_COLUMNS].join(", ");
        format!("{shown}, +{} más", cols.len() - MAX_COLUMNS)
    }
}

fn sample_rows(arr: &[Value], cols: &[String]) -> Vec<String> {
    arr.iter()
        .take(SAMPLE_ROWS)
        .filter_map(|row| {
            row.as_object().map(|map| {
                let fields: Vec<String> = cols
                    .iter()
                    .take(MAX_INLINE_FIELDS)
                    .filter_map(|c| {
                        map.get(c.as_str())
                            .map(|v| format!("{c}:{}", scalar_str(v)))
                    })
                    .collect();
                format!("{{{}}}", fields.join(", "))
            })
        })
        .collect()
}

const MAX_AGG_COLS: usize = 3;

/// For up to `MAX_AGG_COLS` numeric columns, compute min/max across all rows.
fn numeric_aggregates(arr: &[Value], cols: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in cols {
        if out.len() >= MAX_AGG_COLS {
            break;
        }
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut any = false;
        for row in arr {
            if let Some(n) = row.get(c.as_str()).and_then(Value::as_f64) {
                any = true;
                if n < min {
                    min = n;
                }
                if n > max {
                    max = n;
                }
            }
        }
        if any {
            out.push(format!("{c}: min {} max {}", fmt_num(min), fmt_num(max)));
        }
    }
    out
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n:.2}")
    }
}

fn scalar_str(v: &Value) -> String {
    let raw = match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) => return format!("[{}]", a.len()),
        Value::Object(o) => return format!("{{{}}}", o.len()),
    };
    cap(&raw, FIELD_VALUE_CHARS)
}

/// Char-safe truncation with an ellipsis. Safe to truncate a digest because
/// the full result is recoverable verbatim via recall_history (lossless, v1).
fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_of_objects_becomes_tabular_digest() {
        let rows: Vec<String> = (0..8)
            .map(|i| {
                format!(
                    r#"{{"region":"R{i}","revenue":{},"units":{}}}"#,
                    100_000 + i * 1000,
                    500 + i * 10
                )
            })
            .collect();
        let content = format!("[{}]", rows.join(","));
        let d = digest_tool_result(&content).expect("structured");
        assert!(d.contains("8 filas"), "got: {d}");
        assert!(d.contains("cols: region, revenue, units"), "got: {d}");
        assert!(d.contains("region:R0"), "sample row missing: {d}");
    }

    #[test]
    fn array_of_scalars_becomes_count_and_sample() {
        let content = format!(
            "[{}]",
            (0..40).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        );
        let d = digest_tool_result(&content).expect("structured");
        assert!(d.contains("40 elementos"), "got: {d}");
        assert!(d.contains("muestra: [0, 1, 2"), "got: {d}");
    }

    #[test]
    fn object_lists_fields_inline_scalars_and_nested_markers() {
        let content = r#"{
            "order_id":"5512","status":"en transito","total":340,
            "items":[{"sku":"AB-1","qty":2},{"sku":"AB-2","qty":1}],
            "shipping_address":"Av Corrientes 1234","customer":{"email":"a@x.com","tier":"pro"}
        }"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(d.contains("objeto"), "got: {d}");
        assert!(d.contains("order_id"), "got: {d}");
        assert!(d.contains("items[2]"), "nested array marker missing: {d}");
        assert!(
            d.contains("customer{2}"),
            "nested object marker missing: {d}"
        );
        assert!(
            d.contains("status=en transito"),
            "inline scalar missing: {d}"
        );
        // Drill-down surfaces the columns of the dominant array-of-objects field.
        assert!(
            d.contains("items[2] cols: sku, units") || d.contains("items[2] cols: sku, qty"),
            "drill-down missing: {d}"
        );
    }

    #[test]
    fn non_json_returns_none() {
        assert!(
            digest_tool_result("the search returned three relevant articles about ...").is_none()
        );
    }

    #[test]
    fn bare_scalar_returns_none() {
        assert!(digest_tool_result("96").is_none());
        assert!(digest_tool_result(r#""just a quoted string""#).is_none());
    }

    #[test]
    fn empty_collections_return_none() {
        assert!(digest_tool_result("[]").is_none());
        assert!(digest_tool_result("{}").is_none());
    }

    #[test]
    fn many_columns_are_capped_with_overflow_marker() {
        let pairs: Vec<String> = (0..20).map(|i| format!(r#""c{i}":{i}"#)).collect();
        let content = format!("[{{{}}}]", pairs.join(","));
        let d = digest_tool_result(&content).expect("structured");
        assert!(
            d.contains("+8 más"),
            "expected column overflow marker, got: {d}"
        );
    }

    #[test]
    fn tabular_digest_includes_numeric_min_max() {
        let rows = r#"[
            {"region":"Norte","revenue":420000,"margin":18},
            {"region":"Sur","revenue":310000,"margin":22},
            {"region":"Este","revenue":120000,"margin":9}
        ]"#;
        let d = digest_tool_result(rows).expect("structured");
        assert!(d.contains("revenue: min 120000 max 420000"), "got: {d}");
        assert!(d.contains("margin: min 9 max 22"), "got: {d}");
    }

    #[test]
    fn drilled_array_in_object_includes_aggregates() {
        let content = r#"{"row_count":2,"rows":[{"sku":"A","qty":5},{"sku":"B","qty":12}]}"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(d.contains("rows[2] cols: sku, qty"), "got: {d}");
        assert!(d.contains("qty: min 5 max 12"), "got: {d}");
    }
}
