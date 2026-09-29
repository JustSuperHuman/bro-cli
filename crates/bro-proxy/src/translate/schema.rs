//! JSON-schema cleanup for tool parameters.

use serde_json::{Map, Value, json};

/// Keywords no OpenAI-style upstream needs.
const ALWAYS_STRIP: &[&str] = &["$schema", "$id", "cache_control"];

/// Extra keywords strict upstreams (Gemini's OpenAI endpoint, some local servers) reject.
const STRICT_STRIP: &[&str] = &[
    "additionalProperties",
    "propertyNames",
    "patternProperties",
    "unevaluatedProperties",
    "dependentRequired",
    "dependentSchemas",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "examples",
    "default",
    "const",
    "$comment",
    "$ref",
    "$defs",
    "definitions",
    "contentEncoding",
    "contentMediaType",
    "if",
    "then",
    "else",
    "not",
];

const STRICT_FORMATS: &[&str] = &["enum", "date-time"];

/// Clean a tool's parameter schema. Always returns an object schema.
pub fn clean(schema: Option<&Value>, strict: bool) -> Value {
    let mut v = schema
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    clean_in_place(&mut v, strict);
    if let Some(obj) = v.as_object_mut() {
        if !obj.contains_key("type") {
            obj.insert("type".into(), json!("object"));
        }
        if obj.get("type") == Some(&json!("object")) && !obj.contains_key("properties") {
            obj.insert("properties".into(), json!({}));
        }
    } else {
        v = json!({"type": "object", "properties": {}});
    }
    v
}

fn clean_in_place(v: &mut Value, strict: bool) {
    match v {
        Value::Object(obj) => {
            for k in ALWAYS_STRIP {
                obj.remove(*k);
            }
            if strict {
                for k in STRICT_STRIP {
                    obj.remove(*k);
                }
                if obj
                    .get("format")
                    .and_then(Value::as_str)
                    .is_some_and(|f| !STRICT_FORMATS.contains(&f))
                {
                    obj.remove("format");
                }
            }
            // `properties` is a map of name -> schema: recurse into values only, so a
            // property literally named e.g. "default" survives.
            if let Some(Value::Object(props)) = obj.get_mut("properties") {
                for p in props.values_mut() {
                    clean_in_place(p, strict);
                }
            }
            for (k, child) in obj.iter_mut() {
                if k != "properties" {
                    clean_in_place(child, strict);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|c| clean_in_place(c, strict)),
        _ => {}
    }
}

/// Wrap a Responses `custom` (freeform) tool as a JSON tool with one string field.
pub fn custom_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "input": { "type": "string", "description": "The raw tool input" } },
        "required": ["input"]
    })
}

pub fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_keywords() {
        let s = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "url": {"type": "string", "format": "uri"},
                "default": {"type": "integer", "exclusiveMinimum": 0, "default": 3}
            },
            "required": ["url"]
        });
        let lenient = clean(Some(&s), false);
        assert!(lenient.get("$schema").is_none());
        assert_eq!(lenient["additionalProperties"], json!(false));
        let strict = clean(Some(&s), true);
        assert!(strict.get("additionalProperties").is_none());
        assert!(strict["properties"]["url"].get("format").is_none());
        assert!(strict["properties"]["default"].get("default").is_none());
        assert!(
            strict["properties"]["default"]
                .get("exclusiveMinimum")
                .is_none()
        );
        assert_eq!(strict["required"], json!(["url"]));
        assert_eq!(
            clean(None, true),
            json!({"type": "object", "properties": {}})
        );
    }
}
