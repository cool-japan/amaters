//! Output formatting for CLI results

use amaters_core::{CipherBlob, Key};
use anyhow::Result;
use comfy_table::{Cell, CellAlignment, Color, Table};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;

/// Output format type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Yaml,
    Table,
    /// NUL-delimited format: `<key>\0<value>\0` per record.
    /// Binary values are base64-wrapped with `BASE64:` sentinel.
    Nul,
    /// Newline-delimited JSON: one JSON object per line.
    /// Binary values are base64-wrapped with `BASE64:` sentinel in the value field.
    Ndjson,
}

impl OutputFormat {
    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "table" => Some(Self::Table),
            "nul" => Some(Self::Nul),
            "ndjson" => Some(Self::Ndjson),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Pipe-friendly helpers
// ---------------------------------------------------------------------------

/// Returns `true` if every byte in `bytes` is printable ASCII, tab, or newline.
pub fn is_text_safe(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|&b| b == b'\t' || b == b'\n' || (32..=126).contains(&b))
}

/// Encode bytes as standard (padded) base64.
///
/// Implemented inline to avoid a new crate dependency.
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        out.push(TABLE[b2 & 0x3f] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let b0 = bytes[i] as usize;
            out.push(TABLE[b0 >> 2] as char);
            out.push(TABLE[(b0 & 3) << 4] as char);
            out.push_str("==");
        }
        2 => {
            let b0 = bytes[i] as usize;
            let b1 = bytes[i + 1] as usize;
            out.push(TABLE[b0 >> 2] as char);
            out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
            out.push(TABLE[(b1 & 0xf) << 2] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Encode bytes as a displayable string, using base64 sentinel for binary data.
fn encode_field(bytes: &[u8]) -> String {
    if is_text_safe(bytes) {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        format!("BASE64:{}", base64_encode(bytes))
    }
}

/// Format a single record in NUL-delimited format.
///
/// Output is: `<key_field>\0<value_field>\0`
/// Binary key or value bytes are base64-wrapped.
pub fn format_record_nul(key: &[u8], value: &[u8]) -> Vec<u8> {
    let key_str = encode_field(key);
    let val_str = encode_field(value);
    let mut out = Vec::with_capacity(key_str.len() + 1 + val_str.len() + 1);
    out.extend_from_slice(key_str.as_bytes());
    out.push(0);
    out.extend_from_slice(val_str.as_bytes());
    out.push(0);
    out
}

/// Format a single record as an NDJSON line.
///
/// Output is: `{"key":"...","value":"..."}\n`
/// Binary key or value bytes are base64-wrapped.
pub fn format_record_ndjson(key: &[u8], value: &[u8]) -> String {
    let key_str = encode_field(key);
    let val_str = encode_field(value);
    // Use serde_json for correct JSON escaping.
    let obj = json!({"key": key_str, "value": val_str});
    format!("{}\n", obj)
}

/// Print a success message for Set operation
pub fn print_set_result(key: &Key, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let output = json!({
                "status": "success",
                "operation": "set",
                "key": key.to_string_lossy(),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let output = json!({
                "status": "success",
                "operation": "set",
                "key": key.to_string_lossy(),
            });
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            println!("✓ Successfully set key: {}", key.to_string_lossy());
        }
        OutputFormat::Nul | OutputFormat::Ndjson => {
            // Single-record operations: emit key only (no value to stream).
            println!("ok: {}", key.to_string_lossy());
        }
    }
    Ok(())
}

/// Print a Get operation result
pub fn print_get_result(key: &Key, value: Option<&CipherBlob>, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let output = if let Some(blob) = value {
                json!({
                    "status": "success",
                    "operation": "get",
                    "key": key.to_string_lossy(),
                    "value": {
                        "size": blob.len(),
                        "checksum": blob.metadata().checksum,
                        "created_at": blob.metadata().created_at.to_rfc3339(),
                    }
                })
            } else {
                json!({
                    "status": "not_found",
                    "operation": "get",
                    "key": key.to_string_lossy(),
                })
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let output = if let Some(blob) = value {
                json!({
                    "status": "success",
                    "operation": "get",
                    "key": key.to_string_lossy(),
                    "value": {
                        "size": blob.len(),
                        "checksum": blob.metadata().checksum,
                        "created_at": blob.metadata().created_at.to_rfc3339(),
                    }
                })
            } else {
                json!({
                    "status": "not_found",
                    "operation": "get",
                    "key": key.to_string_lossy(),
                })
            };
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            if let Some(blob) = value {
                let mut table = Table::new();
                table.set_header(vec!["Property", "Value"]);

                table.add_row(vec![
                    Cell::new("Key").fg(Color::Cyan),
                    Cell::new(key.to_string_lossy()),
                ]);
                table.add_row(vec![
                    Cell::new("Size").fg(Color::Cyan),
                    Cell::new(format!("{} bytes", blob.len())),
                ]);
                table.add_row(vec![
                    Cell::new("Checksum").fg(Color::Cyan),
                    Cell::new(format!("{:#x}", blob.metadata().checksum)),
                ]);
                table.add_row(vec![
                    Cell::new("Created At").fg(Color::Cyan),
                    Cell::new(blob.metadata().created_at.to_rfc3339()),
                ]);

                println!("{table}");
            } else {
                println!("✗ Key not found: {}", key.to_string_lossy());
            }
        }
        OutputFormat::Nul | OutputFormat::Ndjson => {
            // Single-record get: emit the raw encrypted bytes.
            if let Some(blob) = value {
                let key_bytes = key.as_bytes();
                let val_bytes = blob.as_bytes();
                if format == OutputFormat::Nul {
                    let record = format_record_nul(key_bytes, val_bytes);
                    std::io::stdout().write_all(&record)?;
                } else {
                    print!("{}", format_record_ndjson(key_bytes, val_bytes));
                }
            } else {
                eprintln!("not_found: {}", key.to_string_lossy());
            }
        }
    }
    Ok(())
}

/// Print a Delete operation result
pub fn print_delete_result(key: &Key, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let output = json!({
                "status": "success",
                "operation": "delete",
                "key": key.to_string_lossy(),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let output = json!({
                "status": "success",
                "operation": "delete",
                "key": key.to_string_lossy(),
            });
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            println!("✓ Successfully deleted key: {}", key.to_string_lossy());
        }
        OutputFormat::Nul | OutputFormat::Ndjson => {
            println!("deleted: {}", key.to_string_lossy());
        }
    }
    Ok(())
}

/// Print a range query result
pub fn print_range_result(results: &[(Key, CipherBlob)], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let items: Vec<_> = results
                .iter()
                .map(|(key, blob)| {
                    json!({
                        "key": key.to_string_lossy(),
                        "size": blob.len(),
                        "checksum": blob.metadata().checksum,
                        "created_at": blob.metadata().created_at.to_rfc3339(),
                    })
                })
                .collect();

            let output = json!({
                "status": "success",
                "operation": "range",
                "count": results.len(),
                "results": items,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let items: Vec<_> = results
                .iter()
                .map(|(key, blob)| {
                    json!({
                        "key": key.to_string_lossy(),
                        "size": blob.len(),
                        "checksum": blob.metadata().checksum,
                        "created_at": blob.metadata().created_at.to_rfc3339(),
                    })
                })
                .collect();

            let output = json!({
                "status": "success",
                "operation": "range",
                "count": results.len(),
                "results": items,
            });
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            if results.is_empty() {
                println!("No results found");
                return Ok(());
            }

            let mut table = Table::new();
            table.set_header(vec![
                Cell::new("Key").fg(Color::Cyan),
                Cell::new("Size").fg(Color::Cyan),
                Cell::new("Checksum").fg(Color::Cyan),
                Cell::new("Created At").fg(Color::Cyan),
            ]);

            for (key, blob) in results {
                table.add_row(vec![
                    Cell::new(key.to_string_lossy()),
                    Cell::new(format!("{} bytes", blob.len())).set_alignment(CellAlignment::Right),
                    Cell::new(format!("{:#x}", blob.metadata().checksum)),
                    Cell::new(
                        blob.metadata()
                            .created_at
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                    ),
                ]);
            }

            println!("{table}");
            println!("\nTotal: {} results", results.len());
        }
        OutputFormat::Nul => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for (key, blob) in results {
                let record = format_record_nul(key.as_bytes(), blob.as_bytes());
                out.write_all(&record)?;
            }
        }
        OutputFormat::Ndjson => {
            for (key, blob) in results {
                print!("{}", format_record_ndjson(key.as_bytes(), blob.as_bytes()));
            }
        }
    }
    Ok(())
}

/// Print a paginated range query result, optionally showing the next cursor.
pub fn print_paginated_result(
    results: &[(Key, CipherBlob)],
    next_cursor: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let items: Vec<_> = results
                .iter()
                .map(|(key, blob)| {
                    serde_json::json!({
                        "key": key.to_string_lossy(),
                        "size": blob.len(),
                        "checksum": blob.metadata().checksum,
                        "created_at": blob.metadata().created_at.to_rfc3339(),
                    })
                })
                .collect();

            let mut output = serde_json::json!({
                "status": "success",
                "operation": "range",
                "count": results.len(),
                "results": items,
                "has_more": next_cursor.is_some(),
            });
            if let Some(cursor) = next_cursor {
                output["next_cursor"] = serde_json::Value::String(cursor.to_string());
            }
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let items: Vec<_> = results
                .iter()
                .map(|(key, blob)| {
                    serde_json::json!({
                        "key": key.to_string_lossy(),
                        "size": blob.len(),
                        "checksum": blob.metadata().checksum,
                        "created_at": blob.metadata().created_at.to_rfc3339(),
                    })
                })
                .collect();

            let mut output = serde_json::json!({
                "status": "success",
                "operation": "range",
                "count": results.len(),
                "results": items,
                "has_more": next_cursor.is_some(),
            });
            if let Some(cursor) = next_cursor {
                output["next_cursor"] = serde_json::Value::String(cursor.to_string());
            }
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            if results.is_empty() {
                println!("No results found");
            } else {
                let mut table = Table::new();
                table.set_header(vec![
                    Cell::new("Key").fg(Color::Cyan),
                    Cell::new("Size").fg(Color::Cyan),
                    Cell::new("Checksum").fg(Color::Cyan),
                    Cell::new("Created At").fg(Color::Cyan),
                ]);

                for (key, blob) in results {
                    table.add_row(vec![
                        Cell::new(key.to_string_lossy()),
                        Cell::new(format!("{} bytes", blob.len()))
                            .set_alignment(CellAlignment::Right),
                        Cell::new(format!("{:#x}", blob.metadata().checksum)),
                        Cell::new(
                            blob.metadata()
                                .created_at
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string(),
                        ),
                    ]);
                }

                println!("{table}");
                println!("\nTotal: {} results", results.len());
            }

            if let Some(cursor) = next_cursor {
                println!("Next cursor: {}", cursor);
            }
        }
        OutputFormat::Nul => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for (key, blob) in results {
                let record = format_record_nul(key.as_bytes(), blob.as_bytes());
                out.write_all(&record)?;
            }
            // Cursor is not meaningful in streaming NUL-delimited output.
        }
        OutputFormat::Ndjson => {
            for (key, blob) in results {
                print!("{}", format_record_ndjson(key.as_bytes(), blob.as_bytes()));
            }
            // Cursor is not meaningful in streaming NDJSON output.
        }
    }
    Ok(())
}

/// Print an error message
pub fn print_error(error: &anyhow::Error, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let output = json!({
                "status": "error",
                "message": error.to_string(),
            });
            if let Ok(json_str) = serde_json::to_string_pretty(&output) {
                eprintln!("{}", json_str);
            } else {
                eprintln!("Error: {}", error);
            }
        }
        OutputFormat::Yaml => {
            let output = json!({
                "status": "error",
                "message": error.to_string(),
            });
            if let Ok(yaml_str) = serde_yaml::to_string(&output) {
                eprintln!("{}", yaml_str);
            } else {
                eprintln!("Error: {}", error);
            }
        }
        OutputFormat::Table => {
            eprintln!("✗ Error: {}", error);
        }
        OutputFormat::Nul | OutputFormat::Ndjson => {
            eprintln!("error: {}", error);
        }
    }
}

/// Print any serializable value in the specified format
pub fn print_value<T: Serialize>(value: &T, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let json_str = serde_json::to_string_pretty(value)?;
            println!("{}", json_str);
        }
        OutputFormat::Yaml => {
            let yaml_str = serde_yaml::to_string(value)?;
            println!("{}", yaml_str);
        }
        OutputFormat::Table => {
            // For table format, fall back to JSON pretty print
            let json_str = serde_json::to_string_pretty(value)?;
            println!("{}", json_str);
        }
        OutputFormat::Nul | OutputFormat::Ndjson => {
            // For non-tabular structured values, fall back to compact JSON.
            let json_str = serde_json::to_string_pretty(value)?;
            println!("{}", json_str);
        }
    }
    Ok(())
}

/// Print a success message
pub fn print_success(message: &str, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let output = json!({
                "status": "success",
                "message": message,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let output = json!({
                "status": "success",
                "message": message,
            });
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            println!("✓ {}", message);
        }
        OutputFormat::Nul | OutputFormat::Ndjson => {
            println!("ok: {}", message);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parse() {
        assert_eq!(OutputFormat::from_str("json"), Some(OutputFormat::Json));
        assert_eq!(OutputFormat::from_str("JSON"), Some(OutputFormat::Json));
        assert_eq!(OutputFormat::from_str("yaml"), Some(OutputFormat::Yaml));
        assert_eq!(OutputFormat::from_str("yml"), Some(OutputFormat::Yaml));
        assert_eq!(OutputFormat::from_str("table"), Some(OutputFormat::Table));
        assert_eq!(OutputFormat::from_str("TABLE"), Some(OutputFormat::Table));
        assert_eq!(OutputFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_print_set_result() -> Result<()> {
        let key = Key::from_str("test_key");
        print_set_result(&key, OutputFormat::Json)?;
        print_set_result(&key, OutputFormat::Table)?;
        Ok(())
    }

    #[test]
    fn test_print_get_result() -> Result<()> {
        let key = Key::from_str("test_key");
        let blob = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        print_get_result(&key, Some(&blob), OutputFormat::Json)?;
        print_get_result(&key, Some(&blob), OutputFormat::Table)?;
        print_get_result(&key, None, OutputFormat::Json)?;
        print_get_result(&key, None, OutputFormat::Table)?;

        Ok(())
    }

    #[test]
    fn test_print_delete_result() -> Result<()> {
        let key = Key::from_str("test_key");
        print_delete_result(&key, OutputFormat::Json)?;
        print_delete_result(&key, OutputFormat::Table)?;
        Ok(())
    }

    #[test]
    fn test_print_range_result() -> Result<()> {
        let key1 = Key::from_str("key1");
        let key2 = Key::from_str("key2");
        let blob1 = CipherBlob::new(vec![1, 2, 3]);
        let blob2 = CipherBlob::new(vec![4, 5, 6]);

        let results = vec![(key1, blob1), (key2, blob2)];

        print_range_result(&results, OutputFormat::Json)?;
        print_range_result(&results, OutputFormat::Table)?;
        print_range_result(&[], OutputFormat::Table)?;

        Ok(())
    }

    #[test]
    fn test_print_error() {
        let error = anyhow::anyhow!("Test error");
        print_error(&error, OutputFormat::Json);
        print_error(&error, OutputFormat::Table);
    }

    // -----------------------------------------------------------------------
    // Item 5: Pipe-friendly output tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_output_nul_text_record() {
        let key = b"hello";
        let value = b"world";
        let record = format_record_nul(key, value);
        // Expected: "hello\0world\0"
        assert_eq!(record, b"hello\x00world\x00");
    }

    #[test]
    fn test_output_nul_binary_value_base64_wrapped() {
        let key = b"mykey";
        // value contains byte 0x01 — not text-safe
        let value = b"\x01\x02\x03";
        let record = format_record_nul(key, value);
        let record_str = String::from_utf8_lossy(&record);
        // key is text-safe, value should be BASE64-wrapped
        assert!(record_str.starts_with("mykey\x00BASE64:"));
        assert!(record_str.ends_with('\x00'));
    }

    #[test]
    fn test_output_ndjson_record_per_line() {
        let key = b"k1";
        let value = b"v1";
        let line = format_record_ndjson(key, value);
        // Should be a single JSON object followed by exactly one newline.
        assert!(line.ends_with('\n'));
        // Should parse as valid JSON.
        let parsed: serde_json::Value =
            serde_json::from_str(line.trim_end()).expect("should be valid JSON");
        assert_eq!(parsed["key"], "k1");
        assert_eq!(parsed["value"], "v1");
    }

    #[test]
    fn test_output_ndjson_escapes_quotes() {
        let key = b"k";
        let value = b"say \"hello\"";
        let line = format_record_ndjson(key, value);
        // Must parse as valid JSON — serde_json handles escaping.
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end())
            .expect("should be valid JSON with escaped quotes");
        assert_eq!(parsed["value"], "say \"hello\"");
    }

    #[test]
    fn test_output_ndjson_binary_value_base64_wrapped() {
        let key = b"bkey";
        let value = b"\xff\xfe\xfd";
        let line = format_record_ndjson(key, value);
        let parsed: serde_json::Value =
            serde_json::from_str(line.trim_end()).expect("should be valid JSON");
        let val_str = parsed["value"].as_str().expect("value should be a string");
        assert!(
            val_str.starts_with("BASE64:"),
            "Binary value should be base64-wrapped, got: {val_str}"
        );
    }

    #[test]
    fn test_format_flag_accepts_nul_and_ndjson() {
        assert_eq!(OutputFormat::from_str("nul"), Some(OutputFormat::Nul));
        assert_eq!(OutputFormat::from_str("NUL"), Some(OutputFormat::Nul));
        assert_eq!(OutputFormat::from_str("ndjson"), Some(OutputFormat::Ndjson));
        assert_eq!(OutputFormat::from_str("NDJSON"), Some(OutputFormat::Ndjson));
        assert_eq!(OutputFormat::from_str("unknown"), None);
    }

    #[test]
    fn test_base64_encode_basic() {
        // Known base64 encodings
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn test_is_text_safe() {
        assert!(is_text_safe(b"hello world"));
        assert!(is_text_safe(b"tab\there"));
        assert!(is_text_safe(b"newline\nhere"));
        assert!(!is_text_safe(b"\x00null"));
        assert!(!is_text_safe(b"\x01binary"));
        assert!(!is_text_safe(b"\xff"));
    }

    #[test]
    fn test_nul_format_multiple_records() {
        let results: Vec<(Key, CipherBlob)> = vec![
            (Key::from_str("key1"), CipherBlob::new(b"val1".to_vec())),
            (Key::from_str("key2"), CipherBlob::new(b"val2".to_vec())),
        ];
        // Should not panic; streaming output goes to stdout.
        let result = print_range_result(&results, OutputFormat::Nul);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ndjson_format_multiple_records() {
        let results: Vec<(Key, CipherBlob)> = vec![
            (Key::from_str("alpha"), CipherBlob::new(b"AAA".to_vec())),
            (Key::from_str("beta"), CipherBlob::new(b"BBB".to_vec())),
        ];
        let result = print_range_result(&results, OutputFormat::Ndjson);
        assert!(result.is_ok());
    }
}
