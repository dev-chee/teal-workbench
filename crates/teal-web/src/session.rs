//! 文本浏览器的有界离线会话数据。
//!
//! 只保存结构化页面与当前索引，不保存网络连接或认证状态；恢复本身不会重新发送请求。

use crate::WebDocument;
use serde::{Deserialize, Serialize};

/// 有界离线浏览历史；恢复时不执行网络 I/O。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSession {
    /// 按导航顺序保存的结构化页面。
    pub history: Vec<WebDocument>,

    /// 当前页面索引。
    pub index: usize,
}

impl BrowserSession {
    /// 最多保留 32 页，每页序列化大小至多 256 KiB。
    pub fn bound(&mut self) {
        if self.history.len() > 32 {
            let remove = self.history.len() - 32;
            self.history.drain(..remove);
            self.index = self.index.saturating_sub(remove);
        }
        self.index = self.index.min(self.history.len().saturating_sub(1));
        for page in &mut self.history {
            page.title = truncate(&page.title, 4096);
            for line in &mut page.lines {
                *line = truncate(line, 256 * 1024);
            }
            let mut truncated = false;
            while serde_json::to_vec(&page).map_or(usize::MAX, |v| v.len()) > 256 * 1024 - 128 {
                if page.lines.pop().is_some() || page.links.pop().is_some() {
                    truncated = true;
                } else {
                    break;
                }
            }
            if truncated {
                page.lines
                    .push("[Session preview truncated; please refresh to reload]".into());
            }
        }
    }

    /// 验证离线 HTTP/HTTPS 历史及保存的大小限制。
    pub fn validate(&self) -> Result<(), String> {
        if self.history.is_empty() || self.history.len() > 32 || self.index >= self.history.len() {
            return Err("Browsing history is invalid".into());
        }
        for page in &self.history {
            if !matches!(page.url.scheme(), "http" | "https") || page.url.as_str().len() > 16384 {
                return Err("Browser URL is invalid".into());
            }
            if serde_json::to_vec(page).map_err(|e| e.to_string())?.len() > 256 * 1024 {
                return Err("Browser page exceeds the session limit".into());
            }
        }
        Ok(())
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_and_unicode_serialized_pages_are_bounded_and_restore_offline() {
        let page = WebDocument {
            url: url::Url::parse("https://example.com").unwrap(),
            title: "\u{4e16}\u{754c}".repeat(3000),
            status: 200,
            lines: vec!["\u{754c}\"".repeat(100_000)],
            links: vec![],
        };
        let mut session = BrowserSession {
            history: vec![page; 40],
            index: 39,
        };
        session.bound();
        session.validate().unwrap();
        assert_eq!(session.history.len(), 32);
        assert_eq!(session.index, 31);
        assert!(
            session
                .history
                .iter()
                .all(|page| serde_json::to_vec(page).unwrap().len() <= 256 * 1024)
        );
        assert!(
            session.history[0]
                .lines
                .last()
                .unwrap()
                .contains("truncated")
        );
        let restored: BrowserSession =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(restored, session);
        session.history[0].url = url::Url::parse("file:///private").unwrap();
        assert!(session.validate().is_err());
    }
}
