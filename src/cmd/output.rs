use std::io::Write;

use anyhow::Result;

use crate::cli::OutputFormat;

pub(super) fn write_payload(data: &[u8], output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                eprintln!("[brrmmmm] payload is not valid JSON, emitting raw");
                write_raw(data)?;
            }
        }
        OutputFormat::Table => {
            if let Ok(serde_json::Value::Object(map)) =
                serde_json::from_slice::<serde_json::Value>(data)
            {
                let rows: Vec<(&str, String)> = map
                    .iter()
                    .map(|(key, value)| {
                        let rendered = match value {
                            serde_json::Value::String(value) => value.clone(),
                            other => other.to_string(),
                        };
                        (key.as_str(), rendered)
                    })
                    .collect();
                print_table(&rows);
            } else {
                eprintln!("[brrmmmm] payload is not a JSON object, emitting raw");
                write_raw(data)?;
            }
        }
        OutputFormat::Text => write_raw(data)?,
    }

    Ok(())
}

pub(super) fn print_table(rows: &[(&str, String)]) {
    let key_w = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let val_w = rows
        .iter()
        .map(|(_, value)| value.len())
        .max()
        .unwrap_or(0)
        .min(60);
    let sep = "─".repeat(key_w + 2 + val_w);
    println!("{:<key_w$}  Value", "Field");
    println!("{sep}");
    for (key, value) in rows {
        println!("{key:<key_w$}  {value}");
    }
}

fn write_raw(data: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(data)?;
    stdout.write_all(b"\n")?;
    Ok(())
}
