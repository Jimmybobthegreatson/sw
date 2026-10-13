//! A small JSON-Schema subset validator for data patches.
//!
//! Supported keywords: `type` (string or array; `integer`, `number`, `string`, `boolean`, `array`,
//! `object`, `null`), `properties`, `required`, `additionalProperties` (bool or schema), `items`,
//! `enum`, `const`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `minLength`,
//! `maxLength`, `minItems`, `maxItems`, `anyOf`, `oneOf`, `allOf`. Unknown keywords are ignored.
//! `$schema`, `title`, `description`, `default` are annotations.

use serde_json::Value;

/// Validate `value` against `schema`; returns human-readable errors (empty = valid).
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    check(schema, value, "", &mut errs);
    errs
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "number" => v.is_number(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => true,
    }
}

fn at(path: &str) -> &str {
    if path.is_empty() { "/" } else { path }
}

fn check(schema: &Value, v: &Value, path: &str, errs: &mut Vec<String>) {
    let Some(s) = schema.as_object() else {
        if schema == &Value::Bool(false) {
            errs.push(format!("{}: not allowed", at(path)));
        }
        return;
    };
    if let Some(t) = s.get("type") {
        let ok = match t {
            Value::String(t) => type_ok(t, v),
            Value::Array(ts) => ts.iter().filter_map(|t| t.as_str()).any(|t| type_ok(t, v)),
            _ => true,
        };
        if !ok {
            errs.push(format!("{}: expected type {}, got {}", at(path), t, kind(v)));
            return;
        }
    }
    if let Some(Value::Array(e)) = s.get("enum") {
        if !e.contains(v) {
            errs.push(format!("{}: value {} not in enum", at(path), v));
        }
    }
    if let Some(c) = s.get("const") {
        if c != v {
            errs.push(format!("{}: must equal {}", at(path), c));
        }
    }
    if let Some(n) = v.as_f64() {
        let lim = |k: &str| s.get(k).and_then(|x| x.as_f64());
        if let Some(m) = lim("minimum") {
            if n < m {
                errs.push(format!("{}: {n} < minimum {m}", at(path)));
            }
        }
        if let Some(m) = lim("maximum") {
            if n > m {
                errs.push(format!("{}: {n} > maximum {m}", at(path)));
            }
        }
        if let Some(m) = lim("exclusiveMinimum") {
            if n <= m {
                errs.push(format!("{}: {n} <= exclusiveMinimum {m}", at(path)));
            }
        }
        if let Some(m) = lim("exclusiveMaximum") {
            if n >= m {
                errs.push(format!("{}: {n} >= exclusiveMaximum {m}", at(path)));
            }
        }
    }
    if let Some(st) = v.as_str() {
        let n = st.chars().count() as u64;
        if let Some(m) = s.get("minLength").and_then(|x| x.as_u64()) {
            if n < m {
                errs.push(format!("{}: string shorter than {m}", at(path)));
            }
        }
        if let Some(m) = s.get("maxLength").and_then(|x| x.as_u64()) {
            if n > m {
                errs.push(format!("{}: string longer than {m}", at(path)));
            }
        }
    }
    if let Some(arr) = v.as_array() {
        let n = arr.len() as u64;
        if let Some(m) = s.get("minItems").and_then(|x| x.as_u64()) {
            if n < m {
                errs.push(format!("{}: fewer than {m} items", at(path)));
            }
        }
        if let Some(m) = s.get("maxItems").and_then(|x| x.as_u64()) {
            if n > m {
                errs.push(format!("{}: more than {m} items", at(path)));
            }
        }
        if let Some(items) = s.get("items") {
            for (i, it) in arr.iter().enumerate() {
                check(items, it, &format!("{path}/{i}"), errs);
            }
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(Value::Array(req)) = s.get("required") {
            for r in req.iter().filter_map(|r| r.as_str()) {
                if !obj.contains_key(r) {
                    errs.push(format!("{}: missing required property {r:?}", at(path)));
                }
            }
        }
        let props = s.get("properties").and_then(|p| p.as_object());
        for (k, val) in obj {
            let sub = format!("{path}/{k}");
            if let Some(ps) = props.and_then(|p| p.get(k)) {
                check(ps, val, &sub, errs);
            } else {
                match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => errs.push(format!("{}: unknown property {k:?}", at(path))),
                    Some(ap @ Value::Object(_)) => check(ap, val, &sub, errs),
                    _ => {}
                }
            }
        }
    }
    if let Some(Value::Array(subs)) = s.get("allOf") {
        for sub in subs {
            check(sub, v, path, errs);
        }
    }
    if let Some(Value::Array(subs)) = s.get("anyOf") {
        if !subs.iter().any(|sub| validate(sub, v).is_empty()) {
            errs.push(format!("{}: matches none of anyOf", at(path)));
        }
    }
    if let Some(Value::Array(subs)) = s.get("oneOf") {
        let n = subs.iter().filter(|sub| validate(sub, v).is_empty()).count();
        if n != 1 {
            errs.push(format!("{}: matches {n} of oneOf (need exactly 1)", at(path)));
        }
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn basic() {
        let s = json!({"type":"object","required":["dmg"],"additionalProperties":false,
            "properties":{"dmg":{"type":"number","minimum":0,"maximum":100},"name":{"type":"string"}}});
        assert!(validate(&s, &json!({"dmg": 5})).is_empty());
        assert_eq!(validate(&s, &json!({"dmg": 500})).len(), 1);
        assert_eq!(validate(&s, &json!({})).len(), 1);
        assert_eq!(validate(&s, &json!({"dmg":1,"x":1})).len(), 1);
        assert_eq!(validate(&s, &json!({"dmg":"a"})).len(), 1);
    }
}
