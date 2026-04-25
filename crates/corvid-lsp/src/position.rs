use lsp_types::{Position, Range};

pub fn byte_span_to_lsp_range(source: &str, start: usize, end: usize) -> Range {
    let start = byte_to_lsp_position(source, start);
    let mut end = byte_to_lsp_position(source, end);
    if end < start {
        end = start;
    }
    Range { start, end }
}

pub fn byte_to_lsp_position(source: &str, offset: usize) -> Position {
    let bounded = offset.min(source.len());
    let mut line = 0u32;
    let mut character = 0u32;

    for (idx, ch) in source.char_indices() {
        if idx >= bounded {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }

    Position { line, character }
}

pub fn lsp_position_to_byte(source: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut character = 0u32;
    for (idx, ch) in source.char_indices() {
        if line == position.line && character >= position.character {
            return idx;
        }
        if ch == '\n' {
            if line == position.line {
                return idx;
            }
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }
    source.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_bytes_to_zero_based_lsp_positions() {
        let src = "agent x() -> Int:\n    return 1\n";
        let offset = src.find("return").unwrap();
        let pos = byte_to_lsp_position(src, offset);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn columns_count_utf16_code_units() {
        let src = "é🙂x";
        let offset = src.find('x').unwrap();
        let pos = byte_to_lsp_position(src, offset);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn maps_lsp_positions_back_to_byte_offsets() {
        let src = "é🙂x\nnext";
        let offset = lsp_position_to_byte(src, Position { line: 0, character: 3 });
        assert_eq!(&src[offset..offset + 1], "x");
    }
}
