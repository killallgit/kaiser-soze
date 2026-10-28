use serde_json::{json, Map, Value};

const MAX_DEPTH: usize = 12;
const MAX_VARIANTS: usize = 8;
/// Fields whose string value is a schema tag, not content, so the value is kept.
const TAGS: [&str; 3] = ["type", "role", "object"];
/// Fields that hold a user-defined schema or free-form data, so only a marker is kept.
const OPAQUE: [&str; 3] = ["input_schema", "parameters", "metadata"];

/// Reduces a JSON value to its shape: keys and types, never content.
pub fn shape(value: &Value) -> Value {
    walk(value, None, 0)
}

fn walk(value: &Value, key: Option<&str>, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return json!("deep");
    }
    match value {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("boolean"),
        Value::Number(_) => json!("number"),
        Value::String(text) if key.is_some_and(|k| TAGS.contains(&k)) && text.len() <= 64 => {
            json!(format!("string={text}"))
        }
        Value::String(_) => json!("string"),
        Value::Array(items) => {
            let mut variants: Vec<Value> = Vec::new();
            for item in items {
                let item_shape = walk(item, None, depth + 1);
                if variants.len() < MAX_VARIANTS && !variants.contains(&item_shape) {
                    variants.push(item_shape);
                }
            }
            Value::Array(variants)
        }
        Value::Object(_) if key.is_some_and(|k| OPAQUE.contains(&k) || k == "input") => json!("opaque"),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(name, child)| (name.clone(), walk(child, Some(name), depth + 1)))
                .collect::<Map<_, _>>(),
        ),
    }
}
