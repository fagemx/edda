use serde_json::Value;

/// Produce canonical JSON bytes: object keys sorted lexicographically (recursive),
/// arrays preserve order, no extra whitespace.
pub fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    let sorted = sort_value(value);
    serde_json::to_vec(&sorted).expect("canonical JSON serialization should not fail")
}

fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut pairs: Vec<(&String, Value)> =
                map.iter().map(|(k, v)| (k, sort_value(v))).collect();
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            let sorted_map: serde_json::Map<String, Value> =
                pairs.into_iter().map(|(k, v)| (k.clone(), v)).collect();
            Value::Object(sorted_map)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sorted_lexicographically() {
        let input: Value = serde_json::from_str(r#"{"z":1,"a":2,"m":3}"#).unwrap();
        let bytes = canonical_json_bytes(&input);
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(output, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn nested_objects_sorted() {
        let input: Value = serde_json::from_str(r#"{"b":{"z":1,"a":2},"a":1}"#).unwrap();
        let bytes = canonical_json_bytes(&input);
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(output, r#"{"a":1,"b":{"a":2,"z":1}}"#);
    }

    #[test]
    fn arrays_preserve_order() {
        let input: Value = serde_json::from_str(r#"{"a":[3,1,2]}"#).unwrap();
        let bytes = canonical_json_bytes(&input);
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(output, r#"{"a":[3,1,2]}"#);
    }

    #[test]
    fn scalars_unchanged() {
        let input: Value = serde_json::from_str(r#""hello""#).unwrap();
        let bytes = canonical_json_bytes(&input);
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(output, r#""hello""#);
    }
}
