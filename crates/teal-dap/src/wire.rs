//! 经认证的适配器到调试器的电线包。

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum Packet {
    Command {
        id: u64,
        command: String,
        arguments: Value,
    },
    Response {
        id: u64,
        success: bool,
        body: Value,
        message: Option<String>,
    },
    Event {
        event: String,
        body: Value,
    },
}

pub(crate) fn write_packet(writer: &mut impl Write, packet: &Packet) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, packet).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub(crate) fn read_packet(reader: &mut impl BufRead) -> io::Result<Option<Packet>> {
    let mut line = String::new();
    let count = reader.read_line(&mut line)?;
    if count == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
