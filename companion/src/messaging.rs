//! Firefox Native Messaging protocol.
//!
//! Each message is framed as: `[u32 length little-endian] [UTF-8 JSON payload]`.
//! See <https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging>.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum message size accepted from the browser (per Mozilla spec: 1 MB).
const MAX_INBOUND_BYTES: u32 = 1024 * 1024;

/// Message sent by the extension to the companion.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum InboundMessage {
    Start,
    Stop,
    Status,
    Ping,
}

/// Message sent by the companion to the extension.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum OutboundMessage {
    Starting,
    Ready { port: u16 },
    Stopped,
    Error { message: String },
    Pong,
}

/// Read one framed message from stdin. Returns `Ok(None)` on clean EOF.
pub async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<InboundMessage>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).context("reading length prefix"),
    }

    let len = u32::from_le_bytes(len_buf);
    if len == 0 {
        bail!("zero-length message");
    }
    if len > MAX_INBOUND_BYTES {
        bail!("message too large: {len} bytes (max {MAX_INBOUND_BYTES})");
    }

    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .context("reading message body")?;

    let msg: InboundMessage =
        serde_json::from_slice(&buf).context("parsing inbound JSON message")?;
    Ok(Some(msg))
}

/// Write one framed message to stdout.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    message: &OutboundMessage,
) -> Result<()> {
    let payload = serde_json::to_vec(message).context("serializing outbound message")?;
    let len: u32 = payload
        .len()
        .try_into()
        .context("outbound message exceeds u32::MAX")?;

    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn roundtrip_start_message() {
        let payload = br#"{"action":"start"}"#;
        let mut framed = Vec::new();
        framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        framed.extend_from_slice(payload);

        let mut reader = BufReader::new(Cursor::new(framed));
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(matches!(msg, InboundMessage::Start));
    }

    #[tokio::test]
    async fn serializes_ready_with_port() {
        let mut out: Vec<u8> = Vec::new();
        write_message(&mut out, &OutboundMessage::Ready { port: 9050 })
            .await
            .unwrap();
        let len = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
        let body = std::str::from_utf8(&out[4..4 + len]).unwrap();
        assert!(body.contains("\"status\":\"ready\""));
        assert!(body.contains("\"port\":9050"));
    }
}
