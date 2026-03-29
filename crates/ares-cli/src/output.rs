use anyhow::Result;
use clap::ValueEnum;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Json,
    Jq,
    Jsonl,
    Csv,
    Table,
}

pub struct OutputFormatter;

impl OutputFormatter {
    pub fn format(format: OutputFormat, data: &Value) -> Result<()> {
        match format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(data)?);
            }
            OutputFormat::Jq => {
                println!("{}", serde_json::to_string(data)?);
            }
            OutputFormat::Jsonl => {
                if let Some(arr) = data.as_array() {
                    for item in arr {
                        println!("{}", serde_json::to_string(item)?);
                    }
                } else {
                    println!("{}", serde_json::to_string(data)?);
                }
            }
            OutputFormat::Csv => {
                let mut wtr = csv::Writer::from_writer(std::io::stdout());

                if let Some(arr) = data.as_array() {
                    for (i, item) in arr.iter().enumerate() {
                        if let Some(obj) = item.as_object() {
                            if i == 0 {
                                let headers: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
                                wtr.write_record(&headers)?;
                            }
                            let row: Vec<String> = obj
                                .values()
                                .map(|v| match v {
                                    Value::String(s) => s.clone(),
                                    _ => v.to_string(),
                                })
                                .collect();
                            wtr.write_record(&row)?;
                        }
                    }
                } else if let Some(obj) = data.as_object() {
                    let headers: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
                    wtr.write_record(&headers)?;
                    let row: Vec<String> = obj
                        .values()
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        })
                        .collect();
                    wtr.write_record(&row)?;
                } else {
                    anyhow::bail!("Cannot format non-object/array as CSV");
                }
                wtr.flush()?;
            }
            OutputFormat::Table => {
                let items = match data {
                    Value::Array(arr) => arr.clone(),
                    Value::Object(_) => vec![data.clone()],
                    _ => anyhow::bail!("Cannot format scalar value as Table"),
                };

                if items.is_empty() {
                    return Ok(());
                }

                if let Some(first_obj) = items[0].as_object() {
                    let keys: Vec<&str> = first_obj.keys().map(|s| s.as_str()).collect();
                    let mut widths = vec![0; keys.len()];

                    for (i, key) in keys.iter().enumerate() {
                        widths[i] = key.len();
                    }

                    let mut rows = vec![];
                    for item in &items {
                        if let Some(obj) = item.as_object() {
                            let mut row = vec![];
                            for (i, key) in keys.iter().enumerate() {
                                let val = obj
                                    .get(*key)
                                    .map(|v| match v {
                                        Value::String(s) => s.clone(),
                                        Value::Null => String::new(),
                                        _ => v.to_string(),
                                    })
                                    .unwrap_or_default();

                                let val_display = if val.len() > 60 {
                                    format!("{}...", &val[..57])
                                } else {
                                    val
                                };

                                widths[i] = widths[i].max(val_display.len());
                                row.push(val_display);
                            }
                            rows.push(row);
                        }
                    }

                    // Print Headers
                    let header_row = keys
                        .iter()
                        .enumerate()
                        .map(|(i, k)| format!("{k:<w$}", k = k.to_uppercase(), w = widths[i]))
                        .collect::<Vec<_>>()
                        .join("  ");
                    println!("{}", header_row);
                    println!("{}", "-".repeat(header_row.len()));

                    // Print Rows
                    for row in rows {
                        let formatted_row = row
                            .iter()
                            .enumerate()
                            .map(|(i, col)| format!("{col:<w$}", col = col, w = widths[i]))
                            .collect::<Vec<_>>()
                            .join("  ");
                        println!("{}", formatted_row);
                    }
                } else {
                    anyhow::bail!("Array elements must be objects to render as a table");
                }
            }
        }
        Ok(())
    }
}
