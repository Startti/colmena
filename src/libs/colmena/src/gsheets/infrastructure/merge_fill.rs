//! Pure forward-fill of merged cells for sheet reads.
//!
//! Google's `spreadsheets.get` returns a merged region's value only in its
//! top-left (anchor) cell; the rest of the rectangle is empty. This module
//! propagates the anchor value across the whole rectangle so a read reflects
//! what the merge visually represents.
//!
//! Spec: `docs/superpowers/specs/2026-06-14-gsheets-expand-merges-design.md`.

use serde_json::Value;

/// A merge rectangle in absolute sheet coordinates (half-open: `start..end`,
/// `end` exclusive — the convention Google's API uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeRect {
    pub start_row: usize,
    pub end_row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// Forward-fill merged cells in `grid` (row-major; `grid[r][c]` is the cell at
/// grid-relative `(r, c)`).
///
/// `row_offset`/`col_offset` are the absolute sheet coordinates of `grid[0][0]`
/// (the `startRow`/`startColumn` of the returned `GridData`). For each merge
/// whose anchor `(start_row, start_col)` — after subtracting the offsets —
/// falls inside the grid, the anchor's value is cloned into every cell of the
/// rectangle, clamped to the grid bounds.
///
/// B1 (per spec): merges whose anchor falls OUTSIDE the grid (e.g. a sub-range
/// that begins partway down a vertical merge) are skipped — those cells keep
/// whatever they had, matching the pre-feature behavior.
pub fn forward_fill_merges(
    grid: &mut [Vec<Value>],
    merges: &[MergeRect],
    row_offset: usize,
    col_offset: usize,
) {
    for m in merges {
        // Anchor before the offset ⇒ outside the returned grid (B1): skip.
        if m.start_row < row_offset || m.start_col < col_offset {
            continue;
        }
        let ar = m.start_row - row_offset; // anchor row, grid-relative
        let ac = m.start_col - col_offset; // anchor col, grid-relative
        let Some(anchor_row) = grid.get(ar) else {
            continue;
        };
        let Some(anchor_val) = anchor_row.get(ac) else {
            continue;
        };
        let anchor_val = anchor_val.clone();
        let r_end = m.end_row.saturating_sub(row_offset).min(grid.len());
        for row in grid.iter_mut().take(r_end).skip(ar) {
            let c_end = m.end_col.saturating_sub(col_offset).min(row.len());
            for cell in row.iter_mut().take(c_end).skip(ac) {
                *cell = anchor_val.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grid(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
        rows
    }

    #[test]
    fn horizontal_merge_fills_to_the_right() {
        // A1:C1 merged, anchor "H". Row width 3.
        let mut g = grid(vec![
            vec![json!("H"), json!(null), json!(null)],
            vec![json!(1), json!(2), json!(3)],
        ]);
        let merges = [MergeRect {
            start_row: 0,
            end_row: 1,
            start_col: 0,
            end_col: 3,
        }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g[0], vec![json!("H"), json!("H"), json!("H")]);
        assert_eq!(g[1], vec![json!(1), json!(2), json!(3)]); // row 2 untouched
    }

    #[test]
    fn vertical_merge_fills_downward() {
        // A1:A3 merged, anchor "Cat".
        let mut g = grid(vec![
            vec![json!("Cat"), json!(10)],
            vec![json!(null), json!(20)],
            vec![json!(null), json!(30)],
        ]);
        let merges = [MergeRect {
            start_row: 0,
            end_row: 3,
            start_col: 0,
            end_col: 1,
        }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g[0][0], json!("Cat"));
        assert_eq!(g[1][0], json!("Cat"));
        assert_eq!(g[2][0], json!("Cat"));
        // Column B untouched.
        assert_eq!(
            (g[0][1].clone(), g[1][1].clone(), g[2][1].clone()),
            (json!(10), json!(20), json!(30))
        );
    }

    #[test]
    fn block_merge_fills_both_directions() {
        // A1:B2 merged, anchor "X".
        let mut g = grid(vec![
            vec![json!("X"), json!(null)],
            vec![json!(null), json!(null)],
        ]);
        let merges = [MergeRect {
            start_row: 0,
            end_row: 2,
            start_col: 0,
            end_col: 2,
        }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(
            g,
            vec![vec![json!("X"), json!("X")], vec![json!("X"), json!("X")],]
        );
    }

    #[test]
    fn anchor_outside_grid_is_skipped_b1() {
        // Merge A1:A5 in absolute coords, but the grid starts at row 2 (offset 2):
        // the anchor (row 0) is above the returned grid, so nothing is filled.
        let mut g = grid(vec![
            vec![json!(null)], // abs row 2
            vec![json!(null)], // abs row 3
        ]);
        let merges = [MergeRect {
            start_row: 0,
            end_row: 5,
            start_col: 0,
            end_col: 1,
        }];
        forward_fill_merges(&mut g, &merges, 2, 0);
        assert_eq!(g, vec![vec![json!(null)], vec![json!(null)]]); // unchanged
    }

    #[test]
    fn offset_applied_when_anchor_inside_grid() {
        // Grid starts at abs (row 2, col 1). Merge anchor at abs (2,1) → grid (0,0).
        let mut g = grid(vec![
            vec![json!("A"), json!(null)],
            vec![json!(null), json!(null)],
        ]);
        let merges = [MergeRect {
            start_row: 2,
            end_row: 4,
            start_col: 1,
            end_col: 3,
        }];
        forward_fill_merges(&mut g, &merges, 2, 1);
        assert_eq!(
            g,
            vec![vec![json!("A"), json!("A")], vec![json!("A"), json!("A")]]
        );
    }

    #[test]
    fn no_merges_is_noop() {
        let original = grid(vec![vec![json!(1), json!(2)], vec![json!(3), json!(4)]]);
        let mut g = original.clone();
        forward_fill_merges(&mut g, &[], 0, 0);
        assert_eq!(g, original);
    }

    #[test]
    fn merge_extending_past_grid_is_clamped() {
        // Merge claims A1:A10 but grid only has 2 rows — no panic, fill what exists.
        let mut g = grid(vec![vec![json!("V")], vec![json!(null)]]);
        let merges = [MergeRect {
            start_row: 0,
            end_row: 10,
            start_col: 0,
            end_col: 1,
        }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g, vec![vec![json!("V")], vec![json!("V")]]);
    }

    #[test]
    fn empty_anchor_fills_with_null_noop() {
        let mut g = grid(vec![vec![json!(null), json!(null)]]);
        let merges = [MergeRect {
            start_row: 0,
            end_row: 1,
            start_col: 0,
            end_col: 2,
        }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g, vec![vec![json!(null), json!(null)]]);
    }
}
