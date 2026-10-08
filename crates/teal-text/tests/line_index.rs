//! 共享文本坐标在 LSP 与工作台之间的边界回归。

use teal_text::{LineIndex, Utf16Position};

#[test]
fn converts_lf_crlf_empty_lines_and_eof_without_counting_line_endings() {
    let index = LineIndex::new("a\u{1f600}b\r\n\nc\n");

    assert_eq!(index.offset(0, 0), Some(0));
    assert_eq!(index.offset(0, 1), Some(1));
    assert_eq!(index.offset(0, 3), Some(5));
    assert_eq!(index.offset(0, 4), Some(6));
    assert_eq!(index.offset(0, 5), None);
    assert_eq!(index.offset(1, 0), Some(8));
    assert_eq!(index.offset(1, 1), None);
    assert_eq!(index.offset(2, 0), Some(9));
    assert_eq!(index.offset(2, 1), Some(10));
    assert_eq!(index.offset(3, 0), Some(11));
    assert_eq!(index.offset(3, 1), None);
    assert_eq!(index.offset(4, 0), None);

    assert_eq!(
        index.position(6),
        Some(Utf16Position {
            line: 0,
            character: 4
        }),
        "the carriage return belongs to the preceding line end"
    );
    assert_eq!(
        index.position(7),
        Some(Utf16Position {
            line: 0,
            character: 4
        }),
        "the line feed belongs to the preceding line end"
    );
    assert_eq!(
        index.position(8),
        Some(Utf16Position {
            line: 1,
            character: 0
        }),
        "the empty line starts after the CRLF pair"
    );
    assert_eq!(
        index.position(10),
        Some(Utf16Position {
            line: 2,
            character: 1
        }),
        "the LF after a content line maps to that line end"
    );
    assert_eq!(
        index.position(11),
        Some(Utf16Position {
            line: 3,
            character: 0
        }),
        "EOF after a trailing newline is the empty final line"
    );
}

#[test]
fn rejects_utf16_surrogate_and_utf8_byte_interior_positions() {
    let index = LineIndex::new("a\u{1f600}b");

    assert_eq!(index.offset(0, 2), None);
    assert_eq!(index.position(2), None);
    assert_eq!(index.position(3), None);
    assert_eq!(index.position(4), None);
    assert_eq!(index.char_offset(2), None);
    assert_eq!(index.char_offset(5), Some(2));
    assert_eq!(
        index.position(6),
        Some(Utf16Position {
            line: 0,
            character: 4
        })
    );
    assert_eq!(index.position(7), None);
}
