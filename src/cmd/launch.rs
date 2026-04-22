use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

use crate::daemon::{DaemonClient, DaemonCommand, DaemonResponse, socket_path};

pub fn cmd_launch(
    wasm: String,
    name: Option<String>,
    env: &[String],
    params: Option<String>,
) -> Result<()> {
    let wasm = canonicalize_wasm_path(&wasm)?;
    let sock = socket_path();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut client = DaemonClient::connect(&sock).await?;
        let env_map: HashMap<String, String> = env
            .iter()
            .filter_map(|s| {
                let (k, v) = s.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect();
        let resp = client
            .send(&DaemonCommand::Launch {
                wasm,
                name,
                env: env_map,
                params,
            })
            .await?;
        match resp {
            DaemonResponse::Launched { mission } => {
                println!("{mission} launched");
                Ok(())
            }
            DaemonResponse::Full { message } | DaemonResponse::Error { message } => {
                anyhow::bail!("{message}")
            }
            _ => anyhow::bail!("unexpected response from daemon"),
        }
    })
}

fn canonicalize_wasm_path(wasm: &str) -> Result<String> {
    let path = std::fs::canonicalize(Path::new(wasm))
        .with_context(|| format!("resolve mission module path: {wasm}"))?;
    Ok(path.to_string_lossy().into_owned())
}
