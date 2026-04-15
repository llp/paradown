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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceState {
    pub piece_index: u32,
    pub completed: bool,
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

pub fn initialize_piece_states(pieces: &[PieceLayout]) -> Vec<PieceState> {
    pieces
        .iter()
        .map(|piece| PieceState {
            piece_index: piece.piece_index,
            completed: false,
        })
        .collect()
}

pub fn mark_completed_pieces(
    states: &mut [PieceState],
    pieces: &[PieceLayout],
    covered_start: u64,
    covered_end: u64,
) {
    for (state, piece) in states.iter_mut().zip(pieces.iter()) {
        let piece_end = piece
            .offset
            .saturating_add(piece.length as u64)
            .saturating_sub(1);
        if piece.offset >= covered_start && piece_end <= covered_end {
            state.completed = true;
        }
    }
}

pub fn completed_piece_count(states: &[PieceState]) -> u32 {
    states.iter().filter(|piece| piece.completed).count() as u32
}

#[cfg(test)]
mod tests {
    use super::{
        completed_piece_count, initialize_piece_states, mark_completed_pieces, plan_piece_layouts,
    };

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

    #[test]
    fn marks_only_fully_covered_pieces_as_completed() {
        let pieces = plan_piece_layouts(10, 4);
        let mut states = initialize_piece_states(&pieces);

        mark_completed_pieces(&mut states, &pieces, 0, 5);

        assert_eq!(completed_piece_count(&states), 1);
        assert!(states[0].completed);
        assert!(!states[1].completed);
        assert!(!states[2].completed);
    }
}
