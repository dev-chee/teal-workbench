//! LSP 字节流的消息边界编解码。
//!
//! Content-Length 表示正文的字节数，与 Unicode 字符数无关；连续消息按各自边界读取。
//! 截断头部、缺失或非法长度均返回读取错误，不能尝试把残余字节当作下一条消息。

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// 有状态的 LSP `Content-Length` 帧读取器。
pub struct LspReader<R> {
    reader: BufReader<R>,
}

impl<R: AsyncRead + Unpin> LspReader<R> {
    /// 包装一个字节流。
    #[must_use]
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
        }
    }

    /// 读取一条 JSON-RPC 消息；正常到达 EOF 时返回 `None`。
    pub async fn read_message(&mut self) -> Result<Option<serde_json::Value>, std::io::Error> {
        let mut header = Vec::new();
        let mut byte = [0_u8; 1];
        while !header.ends_with(b"\r\n\r\n") && !header.ends_with(b"\n\n") {
            let read = self.reader.read(&mut byte).await?;
            if read == 0 {
                return if header.is_empty() {
                    Ok(None)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "LSP response header is truncated",
                    ))
                };
            }
            header.push(byte[0]);
            if header.len() > MAX_HEADER_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LSP response header exceeds 16 KiB",
                ));
            }
        }
        let header = std::str::from_utf8(&header).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LSP response header is not UTF-8",
            )
        })?;
        let mut content_length = None;
        for line in header.lines() {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.eq_ignore_ascii_case("content-length") {
                if content_length.is_some() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Duplicate LSP Content-Length",
                    ));
                }
                content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "LSP Content-Length is invalid",
                    )
                })?);
            }
        }
        let length = content_length.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Missing LSP Content-Length",
            )
        })?;
        if length > MAX_BODY_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LSP response body exceeds 32 MiB",
            ));
        }
        let mut body = vec![0; length];
        tokio::io::AsyncReadExt::read_exact(&mut self.reader, &mut body).await?;
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

/// 使用 LSP 帧格式写入一个 JSON-RPC 值。
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &serde_json::Value,
) -> Result<(), std::io::Error> {
    let body = serde_json::to_vec(message)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(&body).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::duplex;

    use super::{LspReader, write_message};

    #[tokio::test]
    async fn round_trips_multiple_utf8_frames() {
        let (mut writer, reader) = duplex(1024);
        let send = tokio::spawn(async move {
            write_message(
                &mut writer,
                &json!({"jsonrpc":"2.0","id":1,"result":"world"}),
            )
            .await
            .expect("first frame");
            write_message(&mut writer, &json!({"jsonrpc":"2.0","method":"ready"}))
                .await
                .expect("second frame");
        });
        let mut reader = LspReader::new(reader);
        assert_eq!(
            reader.read_message().await.expect("first read").unwrap()["result"],
            "world"
        );
        assert_eq!(
            reader.read_message().await.expect("second read").unwrap()["method"],
            "ready"
        );
        send.await.expect("write task");
    }

    #[tokio::test]
    async fn rejects_headers_larger_than_16_kib_before_reading_a_body() {
        let (mut writer, reader) = duplex(32 * 1024);
        let send = tokio::spawn(async move {
            let header = format!(
                "X-Fill: {}\r\nContent-Length: 2\r\n\r\n{{}}",
                "x".repeat(16 * 1024)
            );
            tokio::io::AsyncWriteExt::write_all(&mut writer, header.as_bytes())
                .await
                .unwrap();
        });
        let mut reader = LspReader::new(reader);
        let error = reader.read_message().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        send.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_bodies_larger_than_32_mib_before_allocation() {
        let (mut writer, reader) = duplex(128);
        let send = tokio::spawn(async move {
            tokio::io::AsyncWriteExt::write_all(&mut writer, b"Content-Length: 33554433\r\n\r\n")
                .await
                .unwrap();
        });
        let mut reader = LspReader::new(reader);
        let error = reader.read_message().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        send.await.unwrap();
    }
}
