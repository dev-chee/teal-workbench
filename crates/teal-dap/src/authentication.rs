//! DAP 私有传输：双方证明持有共享秘密，握手不发送秘密正文。

use std::{
    io::{self, BufReader, Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const VERSION: u8 = 2;
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PACKET: usize = 4096;

/// 按 TCP 角色握手；launch 和 attach 的 adapter 角色相反。
#[derive(Clone, Copy)]
pub(crate) enum Role {
    Connector,
    Listener,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Message {
    Challenge {
        version: u8,
        nonce: [u8; 32],
    },
    ClientProof {
        version: u8,
        nonce: [u8; 32],
        proof: [u8; 32],
    },
    ServerProof {
        version: u8,
        proof: [u8; 32],
    },
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "DAP v2 authentication failed",
    )
}

fn nonce() -> io::Result<[u8; 32]> {
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(nonce)
}

/// 固定长度 nonce 和不同角色标签避免反射、重放及可变字段拼接歧义。
fn mac(token: &str, role: &[u8], server: &[u8; 32], client: &[u8; 32]) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("HMAC accepts any key size");
    mac.update(b"teal-dap/authentication/v2\0");
    mac.update(role);
    mac.update(server);
    mac.update(client);
    mac
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "DAP authentication timed out"))
}

fn send(stream: &mut TcpStream, deadline: Instant, message: &Message) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_PACKET {
        return Err(invalid());
    }
    let mut written = 0;
    while written < bytes.len() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        let count = stream.write(&bytes[written..])?;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        written += count;
    }
    Ok(())
}

fn receive(stream: &mut TcpStream, deadline: Instant) -> io::Result<Message> {
    let mut bytes = Vec::new();
    // 不预读握手后的命令，确保合并到同一 TCP 写入的业务包仍由返回的 reader 消费。
    for _ in 0..MAX_PACKET {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            return serde_json::from_slice(&bytes).map_err(|_| invalid());
        }
        bytes.push(byte[0]);
    }
    Err(invalid())
}

/// 在总期限内完成互验；旧版包、错误证明、空秘密和不完整输入均失败关闭。
pub(crate) fn authenticate(
    stream: &mut TcpStream,
    token: &str,
    role: Role,
) -> io::Result<BufReader<TcpStream>> {
    if token.is_empty() {
        return Err(invalid());
    }
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    let deadline = Instant::now() + TIMEOUT;
    let result = (|| {
        match role {
            Role::Listener => {
                let server = nonce()?;
                send(
                    stream,
                    deadline,
                    &Message::Challenge {
                        version: VERSION,
                        nonce: server,
                    },
                )?;
                let Message::ClientProof {
                    version: VERSION,
                    nonce: client,
                    proof,
                } = receive(stream, deadline)?
                else {
                    return Err(invalid());
                };
                mac(token, b"connector\0", &server, &client)
                    .verify_slice(&proof)
                    .map_err(|_| invalid())?;
                let proof = mac(token, b"listener\0", &server, &client)
                    .finalize()
                    .into_bytes()
                    .into();
                send(
                    stream,
                    deadline,
                    &Message::ServerProof {
                        version: VERSION,
                        proof,
                    },
                )?;
            }
            Role::Connector => {
                let Message::Challenge {
                    version: VERSION,
                    nonce: server,
                } = receive(stream, deadline)?
                else {
                    return Err(invalid());
                };
                let client = nonce()?;
                let proof = mac(token, b"connector\0", &server, &client)
                    .finalize()
                    .into_bytes()
                    .into();
                send(
                    stream,
                    deadline,
                    &Message::ClientProof {
                        version: VERSION,
                        nonce: client,
                        proof,
                    },
                )?;
                let Message::ServerProof {
                    version: VERSION,
                    proof,
                } = receive(stream, deadline)?
                else {
                    return Err(invalid());
                };
                mac(token, b"listener\0", &server, &client)
                    .verify_slice(&proof)
                    .map_err(|_| invalid())?;
            }
        }
        remaining(deadline)?;
        Ok(())
    })();
    // 期限只约束认证，业务读写继续使用原有生命周期规则。
    let reset_read = stream.set_read_timeout(None);
    let reset_write = stream.set_write_timeout(None);
    result?;
    reset_read?;
    reset_write?;
    Ok(BufReader::new(stream.try_clone()?))
}

#[cfg(test)]
mod tests;
