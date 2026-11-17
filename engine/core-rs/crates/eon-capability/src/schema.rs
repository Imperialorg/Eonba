//! Input validation against a tool's declared JSON Schema (docs/implementation/01, section 5.3).
//!
//! Tool inputs are attacker-influenced, so they are checked before a tool runs. This supports the subset tools
//! actually declare: `type`, `properties`, `required`, `enum`, `items`, and `additionalProperties: false`.
//! Anything outside that subset is refused rather than silently accepted.

use serde_json::{Map, Value};

/// Serializes JSON with object keys sorted, so equal schemas always produce equal bytes for fingerprinting.
pub fn canonical_json(value: &Value) -> String {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: std::collections::BTreeMap<String, Value> =
                    map.iter().map(|(k, v)| (k.clone(), sort(v))).collect();
                Value::Object(sorted.into_iter().collect::<Map<String, Value>>())
            }
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(value).to_string()
}

const SUPPORTED_KEYS: &[&str] = &[
    "type",
    "properties",
    "required",
    "enum",
    "items",
    "additionalProperties",
    "description",
    "title",
    "$schema",
];

pub fn validate(schema: &Value, input: &Value) -> Result<(), String> {
    check(schema, input, "$")
}

fn check(schema: &Value, input: &Value, at: &str) -> Result<(), String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| format!("{at}: schema must be an object"))?;
    if let Some(unknown) = obj.keys().find(|k| !SUPPORTED_KEYS.contains(&k.as_str())) {
        return Err(format!("{at}: unsupported schema keyword '{unknown}'"));
    }
    let enum_violation = obj
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|allowed| !allowed.contains(input));
    if enum_violation {
        return Err(format!("{at}: value is not one of the allowed enum values"));
    }
    let type_violation = obj
        .get("type")
        .and_then(Value::as_str)
        .filter(|expected| !type_matches(expected, input));
    if let Some(expected) = type_violation {
        return Err(format!("{at}: expected {expected}"));
    }
    match input {
        Value::Object(fields) => check_object(obj, fields, at),
        Value::Array(items) => match obj.get("items") {
            Some(item_schema) => items
                .iter()
                .enumerate()
                .try_for_each(|(i, item)| check(item_schema, item, &format!("{at}[{i}]"))),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

fn check_object(schema: &Map<String, Value>, fields: &Map<String, Value>, at: &str) -> Result<(), String> {
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !fields.contains_key(name) {
                return Err(format!("{at}: missing required field '{name}'"));
            }
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
    let unexpected = properties
        .filter(|_| closed)
        .and_then(|props| fields.keys().find(|k| !props.contains_key(*k)));
    if let Some(extra) = unexpected {
        return Err(format!("{at}: unexpected field '{extra}'"));
    }
    if let Some(props) = properties {
        for (name, value) in fields {
            if let Some(field_schema) = props.get(name) {
                check(field_schema, value, &format!("{at}.{name}"))?;
            }
        }
    }
    Ok(())
}

fn type_matches(expected: &str, input: &Value) -> bool {
    match expected {
        "object" => input.is_object(),
        "array" => input.is_array(),
        "string" => input.is_string(),
        "boolean" => input.is_boolean(),
        "integer" => input.as_i64().is_some() || input.as_u64().is_some(),
        "number" => input.is_number(),
        "null" => input.is_null(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read_file_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "max_bytes": {"type": "integer"},
                "mode": {"enum": ["text", "binary"]}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    #[test]
    fn well_formed_input_passes() {
        assert!(validate(&read_file_schema(), &json!({"path": "a.txt", "max_bytes": 10, "mode": "text"})).is_ok());
    }

    #[test]
    fn missing_required_field_is_refused() {
        assert!(validate(&read_file_schema(), &json!({"max_bytes": 10})).is_err());
    }

    #[test]
    fn wrong_type_is_refused() {
        assert!(validate(&read_file_schema(), &json!({"path": 7})).is_err());
        assert!(validate(&read_file_schema(), &json!({"path": "a", "max_bytes": 1.5})).is_err());
    }

    #[test]
    fn smuggled_extra_field_is_refused_when_additional_properties_is_false() {
        let input = json!({"path": "a", "cmd": "rm -rf /"});
        assert!(validate(&read_file_schema(), &input).is_err());
    }

    #[test]
    fn enum_values_are_closed() {
        assert!(validate(&read_file_schema(), &json!({"path": "a", "mode": "exec"})).is_err());
    }

    #[test]
    fn nested_arrays_are_checked_item_by_item() {
        let schema = json!({"type": "array", "items": {"type": "integer"}});
        assert!(validate(&schema, &json!([1, 2, 3])).is_ok());
        assert!(validate(&schema, &json!([1, "two"])).is_err());
    }

    #[test]
    fn unknown_schema_keywords_are_refused_not_ignored() {
        let schema = json!({"type": "string", "pattern": "^[a-z]+$"});
        assert!(validate(&schema, &json!("abc")).is_err());
    }

    #[test]
    fn canonical_form_ignores_key_order() {
        let a = json!({"b": 1, "a": {"y": 2, "x": 3}});
        let b = json!({"a": {"x": 3, "y": 2}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }
}
