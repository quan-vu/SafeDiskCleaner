//! Overview card layout: which cards, in what order and size, and how they
//! pack into a 4-column grid (like CSS grid `dense` auto-placement).

use serde::{Deserialize, Serialize};

pub const CARD_IDS: [&str; 4] = ["disk", "temp", "cpu", "ram"];
pub const GRID_COLUMNS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardSize {
    S,
    M,
    L,
}

impl CardSize {
    /// (columns, rows) the card spans.
    pub fn span(self) -> (usize, usize) {
        match self {
            CardSize::S => (1, 1),
            CardSize::M => (2, 1),
            CardSize::L => (2, 2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPref {
    pub id: String,
    pub size: CardSize,
}

pub fn default_cards() -> Vec<CardPref> {
    [("disk", CardSize::L), ("temp", CardSize::M), ("cpu", CardSize::S), ("ram", CardSize::S)]
        .into_iter()
        .map(|(id, size)| CardPref { id: id.into(), size })
        .collect()
}

/// Known cards once each, in saved order; missing ones are appended with defaults.
pub fn normalize(saved: &[CardPref]) -> Vec<CardPref> {
    let mut out: Vec<CardPref> = vec![];
    for c in saved {
        if CARD_IDS.contains(&c.id.as_str()) && !out.iter().any(|o| o.id == c.id) {
            out.push(c.clone());
        }
    }
    for d in default_cards() {
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d);
        }
    }
    out
}

/// A card's cell in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
}

/// Place each card at the first free spot (top to bottom, left to right) it fits.
/// Returns the placements (same order as `sizes`) and the number of rows used.
pub fn pack(sizes: &[CardSize], columns: usize) -> (Vec<Placement>, usize) {
    let mut grid: Vec<Vec<bool>> = vec![];
    let mut out = vec![];
    for size in sizes {
        let (w, h) = size.span();
        let w = w.min(columns);
        let mut row = 0;
        let placed = loop {
            while grid.len() < row + h {
                grid.push(vec![false; columns]);
            }
            if let Some(col) = (0..=columns - w).find(|&c| (row..row + h).all(|r| (c..c + w).all(|x| !grid[r][x]))) {
                break Placement { col, row, cols: w, rows: h };
            }
            row += 1;
        };
        for cells in &mut grid[placed.row..placed.row + h] {
            cells[placed.col..placed.col + w].fill(true);
        }
        out.push(placed);
    }
    let rows = out.iter().map(|p| p.row + p.rows).max().unwrap_or(0);
    (out, rows)
}

pub const MENU_BAR_ITEMS: [&str; 4] = ["disk", "temp", "cpu", "ram"];
pub const MENU_BAR_STYLES: [&str; 3] = ["compact", "labels", "icons"];
