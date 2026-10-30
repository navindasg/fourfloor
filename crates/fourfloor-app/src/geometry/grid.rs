//! A regular grid of cells (the step sequencer, the clip launcher): cell
//! rectangles and hit-testing.

use egui::{pos2, vec2, Pos2, Rect, Vec2};

/// `rows` x `columns` cells of `cell` size starting at `origin`, with `gap`
/// pixels between cells and an extra `group_gap` every `group` columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellGrid {
    /// Top-left corner of cell (0, 0).
    pub origin: Pos2,
    /// Size of one cell.
    pub cell: Vec2,
    /// Space between neighbouring cells.
    pub gap: f32,
    /// Columns per group (4 for a beat of sixteenths); 0 for no groups.
    pub group: usize,
    /// Extra space between groups.
    pub group_gap: f32,
    /// Number of rows.
    pub rows: usize,
    /// Number of columns.
    pub columns: usize,
}

impl CellGrid {
    fn column_x(&self, column: usize) -> f32 {
        let groups = column.checked_div(self.group).unwrap_or(0);
        self.origin.x + column as f32 * (self.cell.x + self.gap) + groups as f32 * self.group_gap
    }

    /// The rectangle of one cell.
    pub fn rect(&self, row: usize, column: usize) -> Rect {
        let y = self.origin.y + row as f32 * (self.cell.y + self.gap);
        Rect::from_min_size(pos2(self.column_x(column), y), self.cell)
    }

    /// Total size of the grid.
    pub fn size(&self) -> Vec2 {
        if self.rows == 0 || self.columns == 0 {
            return Vec2::ZERO;
        }
        let last = self.rect(self.rows - 1, self.columns - 1);
        last.max - self.origin
    }

    /// The cell under `pos`, if any. Gaps between cells hit nothing.
    pub fn hit(&self, pos: Pos2) -> Option<(usize, usize)> {
        let row_pitch = self.cell.y + self.gap;
        if pos.y < self.origin.y || row_pitch <= 0.0 {
            return None;
        }
        let row = ((pos.y - self.origin.y) / row_pitch).floor() as usize;
        if row >= self.rows {
            return None;
        }
        let column = (0..self.columns).find(|column| {
            let x = self.column_x(*column);
            pos.x >= x && pos.x < x + self.cell.x
        })?;
        self.rect(row, column)
            .contains(pos)
            .then_some((row, column))
    }

    /// The centre of a cell, handy for driving tests.
    pub fn center(&self, row: usize, column: usize) -> Pos2 {
        self.rect(row, column).center()
    }

    /// A grid sized so `columns` cells (with gaps) fill `width`.
    pub fn fitted(
        origin: Pos2,
        width: f32,
        row_height: f32,
        rows: usize,
        columns: usize,
        group: usize,
    ) -> Self {
        let gap = 2.0;
        let group_gap = if group > 0 { 4.0 } else { 0.0 };
        let groups = columns.saturating_sub(1).checked_div(group).unwrap_or(0);
        let spare = width - gap * columns.saturating_sub(1) as f32 - group_gap * groups as f32;
        let cell_w = (spare / columns.max(1) as f32).max(6.0);
        Self {
            origin,
            cell: vec2(cell_w, row_height),
            gap,
            group,
            group_gap,
            rows,
            columns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> CellGrid {
        CellGrid {
            origin: pos2(10.0, 20.0),
            cell: vec2(20.0, 16.0),
            gap: 2.0,
            group: 4,
            group_gap: 6.0,
            rows: 3,
            columns: 16,
        }
    }

    #[test]
    fn every_cell_centre_hits_its_own_cell() {
        let grid = grid();
        for row in 0..grid.rows {
            for column in 0..grid.columns {
                assert_eq!(grid.hit(grid.center(row, column)), Some((row, column)));
            }
        }
    }

    #[test]
    fn groups_add_space_and_gaps_hit_nothing() {
        let grid = grid();
        assert_eq!(grid.rect(0, 4).min.x, 10.0 + 4.0 * 22.0 + 6.0);
        let gap = pos2(grid.rect(0, 0).max.x + 1.0, 25.0);
        assert_eq!(grid.hit(gap), None);
        assert_eq!(grid.hit(pos2(5.0, 25.0)), None);
        assert_eq!(grid.hit(pos2(15.0, 200.0)), None);
        assert_eq!(grid.size().y, 3.0 * 16.0 + 2.0 * 2.0);
    }

    #[test]
    fn a_fitted_grid_fills_the_width() {
        let grid = CellGrid::fitted(pos2(0.0, 0.0), 400.0, 20.0, 2, 16, 4);
        let right = grid.rect(0, 15).max.x;
        assert!((right - 400.0).abs() < 0.5, "{right}");
    }
}
