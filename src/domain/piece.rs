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
    pub block_index: u32,
    pub offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceState {
    pub piece_index: u32,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    pub piece_index: u32,
    pub block_index: u32,
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

pub fn plan_piece_blocks(pieces: &[PieceLayout], block_size: u32) -> Vec<PieceBlock> {
    if pieces.is_empty() {
        return Vec::new();
    }

    let block_size = block_size.max(1);
    let mut blocks = Vec::new();

    for piece in pieces {
        let mut block_index = 0u32;
        let mut offset = piece.offset;
        let piece_end = piece.offset.saturating_add(piece.length as u64);
        while offset < piece_end {
            let remaining = piece_end.saturating_sub(offset);
            let length = remaining.min(block_size as u64) as u32;
            blocks.push(PieceBlock {
                piece_index: piece.piece_index,
                block_index,
                offset,
                length,
            });
            offset = offset.saturating_add(length as u64);
            block_index = block_index.saturating_add(1);
        }
    }

    blocks
}

pub fn initialize_block_states(blocks: &[PieceBlock]) -> Vec<BlockState> {
    blocks
        .iter()
        .map(|block| BlockState {
            piece_index: block.piece_index,
            block_index: block.block_index,
            completed: false,
        })
        .collect()
}

#[allow(dead_code)]
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

pub fn mark_completed_blocks(
    states: &mut [BlockState],
    blocks: &[PieceBlock],
    covered_start: u64,
    covered_end: u64,
) {
    for (state, block) in states.iter_mut().zip(blocks.iter()) {
        let block_end = block
            .offset
            .saturating_add(block.length as u64)
            .saturating_sub(1);
        if block.offset >= covered_start && block_end <= covered_end {
            state.completed = true;
        }
    }
}

pub fn completed_piece_count(states: &[PieceState]) -> u32 {
    states.iter().filter(|piece| piece.completed).count() as u32
}

pub fn completed_block_count(states: &[BlockState]) -> u32 {
    states.iter().filter(|block| block.completed).count() as u32
}

pub fn derive_piece_states_from_blocks(
    pieces: &[PieceLayout],
    blocks: &[PieceBlock],
    block_states: &[BlockState],
) -> Vec<PieceState> {
    pieces
        .iter()
        .map(|piece| {
            let piece_blocks = blocks
                .iter()
                .filter(|block| block.piece_index == piece.piece_index);
            let completed = piece_blocks.clone().all(|block| {
                block_states
                    .iter()
                    .find(|state| {
                        state.piece_index == block.piece_index
                            && state.block_index == block.block_index
                    })
                    .map(|state| state.completed)
                    .unwrap_or(false)
            });
            PieceState {
                piece_index: piece.piece_index,
                completed,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        completed_block_count, completed_piece_count, derive_piece_states_from_blocks,
        initialize_block_states, initialize_piece_states, mark_completed_blocks,
        mark_completed_pieces, plan_piece_blocks, plan_piece_layouts,
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

    #[test]
    fn plans_blocks_and_derives_piece_completion() {
        let pieces = plan_piece_layouts(10, 4);
        let blocks = plan_piece_blocks(&pieces, 2);
        let mut block_states = initialize_block_states(&blocks);

        mark_completed_blocks(&mut block_states, &blocks, 0, 7);
        let piece_states = derive_piece_states_from_blocks(&pieces, &blocks, &block_states);

        assert_eq!(completed_block_count(&block_states), 4);
        assert_eq!(completed_piece_count(&piece_states), 2);
        assert!(piece_states[0].completed);
        assert!(piece_states[1].completed);
        assert!(!piece_states[2].completed);
    }
}
