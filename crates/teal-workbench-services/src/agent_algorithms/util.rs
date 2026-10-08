//! 工作台共享的基础摘要和时间工具。

/// 返回字节内容的十六进制 SHA-256 摘要。
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    teal_utils::sha256_hex(bytes)
}

/// 返回当前 Unix 毫秒时间戳；系统时钟早于 epoch 时返回零。
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
