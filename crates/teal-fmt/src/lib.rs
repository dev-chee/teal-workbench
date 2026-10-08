//! Teal 源码的确定性格式化库。
#![deny(missing_docs)]

mod formatter;

pub use formatter::{FormatError, format, format_named};
