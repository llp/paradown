use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceLayout {
    pub piece_index: u32,
    pub offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceBlock {
    pub piece_index: u32,
    pub offset: u64,
    pub length: u32,
}

pub fn plan_piece_layouts(total_size: u64, piece_size: u32) -> Vec<PieceLayout> {
    if total_size == 0 {
        return Vec::new();
    }

    let piece_size = piece_size.max(1);
    let mut pieces = Vec::new();
    let mut piece_index = 0u32;
    let mut offset = 0u64;

    while offset < total_size {
        let remaining = total_size - offset;
        let length = remaining.min(piece_size as u64) as u32;
        pieces.push(PieceLayout {
            piece_index,
            offset,
            length,
        });
        offset += length as u64;
        piece_index += 1;
    }

    pieces
}

#[cfg(test)]
mod tests {
    use super::plan_piece_layouts;

    #[test]
    fn plans_piece_layouts_for_small_payload() {
        let pieces = plan_piece_layouts(10, 4);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].offset, 0);
        assert_eq!(pieces[0].length, 4);
        assert_eq!(pieces[1].offset, 4);
        assert_eq!(pieces[1].length, 4);
        assert_eq!(pieces[2].offset, 8);
        assert_eq!(pieces[2].length, 2);
    }
}
