use anyhow::{bail, Error};
use serde_json::Value;

// Generate canonical json
pub fn to_canonical_json(value: &Value) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    write_canonical_json(value, &mut data)?;
    Ok(data)
}

pub fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), Error> {
    match value {
        Value::Null => bail!("got unexpected null value"),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => {
            serde_json::to_writer(output, &value)?;
        }
        Value::Array(list) => {
            output.push(b'[');
            let mut iter = list.iter();
            if let Some(item) = iter.next() {
                write_canonical_json(item, output)?;
                for item in iter {
                    output.push(b',');
                    write_canonical_json(item, output)?;
                }
            }
            output.push(b']');
        }
        Value::Object(map) => {
            output.push(b'{');
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort();
            let mut iter = keys.into_iter();
            if let Some(key) = iter.next() {
                serde_json::to_writer(&mut *output, &key)?;
                output.push(b':');
                write_canonical_json(&map[key], output)?;
                for key in iter {
                    output.push(b',');
                    serde_json::to_writer(&mut *output, &key)?;
                    output.push(b':');
                    write_canonical_json(&map[key], output)?;
                }
            }
            output.push(b'}');
        }
    }
    Ok(())
}
