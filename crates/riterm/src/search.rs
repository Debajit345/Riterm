//! Scrollback search and match highlighting.
//!
//! The terminal grid is scanned line by line. Matches are returned as absolute
//! grid points (including negative scrollback lines), which the UI converts to
//! viewport coordinates and uses to drive the current selection.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;

/// A search hit, inclusive of both end points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: Point,
    pub end: Point,
}

impl SearchMatch {
    pub fn range(&self) -> (Point, Point) {
        (self.start, self.end)
    }
}

/// Find case-insensitive matches across the whole scrollback buffer.
///
/// Unicode lowercase expansions are handled one character at a time: a cell
/// whose uppercase form lowers to multiple characters is matched on its first
/// folded character.
pub fn find_matches<T: EventListener>(term: &Term<T>, query: &str) -> Vec<SearchMatch> {
    let needle: Vec<char> = query.to_lowercase().chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }

    let grid = term.grid();
    let columns = grid.columns();
    let top = grid.topmost_line().0;
    let bottom = grid.bottommost_line().0;
    let mut matches = Vec::new();

    for line in (top..=bottom).map(Line::from) {
        let mut folded = Vec::new();
        for column in 0..columns {
            let cell = &grid[Point::new(line, Column(column))];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            for lower in cell.c.to_lowercase() {
                folded.push((lower, column, width));
            }
        }

        matches.extend(find_in_folded(&folded, &needle, line, columns));
        if matches.len() >= 20_000 {
            break;
        }
    }

    matches
}

fn find_in_folded(
    folded: &[(char, usize, usize)],
    needle: &[char],
    line: Line,
    columns: usize,
) -> Vec<SearchMatch> {
    if needle.is_empty() || folded.len() < needle.len() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for start in 0..=folded.len() - needle.len() {
        if !folded[start..start + needle.len()]
            .iter()
            .map(|(ch, _, _)| *ch)
            .eq(needle.iter().copied())
        {
            continue;
        }
        let first = folded[start];
        let last = folded[start + needle.len() - 1];
        let end_col = (last.1 + last.2 - 1).min(columns.saturating_sub(1));
        matches.push(SearchMatch {
            start: Point::new(line, Column(first.1)),
            end: Point::new(line, Column(end_col)),
        });
    }
    matches
}

/// Move a copy-mode cursor by a vi-style word motion within the current line.
/// Returns a new point clamped to the grid.
pub fn word_motion<T: EventListener>(
    term: &Term<T>,
    point: Point,
    forward: bool,
    to_end: bool,
) -> Point {
    let grid = term.grid();
    let last_column = grid.last_column().0;
    let line = point.line;
    let mut column = point.column.0.min(last_column);
    let is_space = |column: usize| {
        let cell = &grid[Point::new(line, Column(column))];
        cell.c.is_whitespace()
    };

    if forward {
        if to_end {
            if column < last_column {
                column += 1;
            }
            while column < last_column && is_space(column) {
                column += 1;
            }
            while column < last_column && !is_space(column + 1) {
                column += 1;
            }
        } else {
            while column < last_column && !is_space(column) {
                column += 1;
            }
            while column < last_column && is_space(column) {
                column += 1;
            }
        }
    } else {
        if column > 0 {
            column -= 1;
        }
        while column > 0 && is_space(column) {
            column -= 1;
        }
        while column > 0 && !is_space(column - 1) {
            column -= 1;
        }
    }

    Point::new(line, Column(column))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_insensitive_matches() {
        let folded: Vec<(char, usize, usize)> = "hello hello"
            .chars()
            .enumerate()
            .map(|(column, ch)| (ch, column, 1))
            .collect();
        let needle: Vec<char> = "HELLO".to_lowercase().chars().collect();
        let matches = find_in_folded(&folded, &needle, Line(0), 11);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start.column, Column(0));
        assert_eq!(matches[1].start.column, Column(6));
        assert_eq!(matches[1].end.column, Column(10));
    }

    #[test]
    fn wide_cells_span_both_columns() {
        let folded = vec![('日', 4, 2), ('本', 6, 2)];
        let needle: Vec<char> = "日本".chars().collect();
        let matches = find_in_folded(&folded, &needle, Line(-2), 20);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start, Point::new(Line(-2), Column(4)));
        assert_eq!(matches[0].end, Point::new(Line(-2), Column(7)));
    }

    #[test]
    fn empty_query_matches_nothing() {
        let folded = vec![('a', 0, 1)];
        assert!(find_in_folded(&folded, &[], Line(0), 1).is_empty());
    }
}
