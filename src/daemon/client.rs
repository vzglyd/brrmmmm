use std::path::Path;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::protocol::{Command, Response};

pub(crate) struct DaemonClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl DaemonClient {
    pub(crate) async fn connect(sock_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(sock_path).await.map_err(|_| {
            anyhow::anyhow!(
                "cannot connect to brrmmmm daemon at {}\nhint: run `brrmmmm daemon start`",
                sock_path.display()
            )
        })?;
        let (r, w) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
        })
    }

    pub(crate) async fn send(&mut self, cmd: &Command) -> Result<Response> {
        let json = serde_json::to_string(cmd)?;
        self.writer
            .write_all(format!("{json}\n").as_bytes())
            .await?;

        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("daemon closed connection without response");
        }
        Ok(serde_json::from_str(line.trim_end())?)
    }
}
