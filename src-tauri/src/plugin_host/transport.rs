//! Content-Length framed JSON-RPC transport over a child process's stdio.
//! Wire format is frozen in docs/plugins/PROTOCOL.md section 1.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Frames above this size are a protocol violation (4 MiB).
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Writes one framed JSON-RPC envelope.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_FRAME_BYTES {
        bail!("outgoing frame exceeds {MAX_FRAME_BYTES} bytes");
    }
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one framed JSON-RPC envelope. Returns Ok(None) on clean EOF at a
/// frame boundary. Any malformed header, oversized frame, or invalid JSON is
/// an error (the caller treats it as a protocol violation).
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            // EOF. Clean only if nothing of a frame was read yet.
            if content_length.is_none() {
                return Ok(None);
            }
            bail!("EOF in the middle of a frame header");
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                let parsed: usize = value
                    .trim()
                    .parse()
                    .map_err(|_| anyhow!("invalid Content-Length value '{}'", value.trim()))?;
                content_length = Some(parsed);
            }
            // Unknown headers are ignored for forward compatibility.
        } else {
            bail!("malformed frame header line");
        }
    }
    let len = content_length.ok_or_else(|| anyhow!("frame missing Content-Length header"))?;
    if len > MAX_FRAME_BYTES {
        bail!("incoming frame of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit");
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|e| anyhow!("frame body is not valid JSON: {e}"))?;
    Ok(Some(value))
}
