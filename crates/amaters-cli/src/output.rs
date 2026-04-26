//! Output formatting for CLI results

use amaters_core::{CipherBlob, Key};
use anyhow::Result;
use comfy_table::{Cell, CellAlignment, Color, Table};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Output format type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Yaml,
    Table,
}

impl OutputFormat {
    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "table" => Some(Self::Table),
            _ => None,
        }
    }
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
}
