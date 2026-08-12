//! Remembers where a window was before Tile first moved it, so that
//! [`WindowAction::Restore`](crate::action::WindowAction::Restore) can put it
//! back.

use std::collections::HashMap;

use crate::geometry::Rect;

/// Maximum number of windows tracked before the least recently touched entries
/// are evicted. Prevents unbounded growth over a long session.
const MAX_ENTRIES: usize = 256;

/// Opaque, backend-supplied window identifier (an `HWND` on Windows, an
/// `AXUIElement` hash on macOS).
pub type WindowId = u64;

#[derive(Debug, Clone)]
struct Entry {
    original: Rect,
    /// Where Tile last put the window, used to detect user-initiated moves.
    last_applied: Rect,
    touched: u64,
}

/// Per-window record of the rectangle to restore to.
#[derive(Debug, Default)]
pub struct WindowHistory {
    entries: HashMap<WindowId, Entry>,
    clock: u64,
}

impl WindowHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that Tile is about to move `id` from `before` to `after`.
    ///
    /// The original rectangle is only captured the first time, or after the
    /// user has moved the window themselves; otherwise repeatedly pressing
    /// "left half" then "restore" would restore to a Tile-produced rectangle
    /// instead of the window's true original position.
    pub fn record(&mut self, id: WindowId, before: Rect, after: Rect) {
        self.clock += 1;
        let clock = self.clock;

        match self.entries.get_mut(&id) {
            Some(entry) if entry.last_applied.approx_eq(&before, 2.0) => {
                entry.last_applied = after;
                entry.touched = clock;
            }
            _ => {
                self.entries.insert(
                    id,
                    Entry {
                        original: before,
                        last_applied: after,
                        touched: clock,
                    },
                );
            }
        }

        self.evict_if_needed();
    }

    /// Returns the rectangle to restore `id` to, consuming the entry.
    pub fn take(&mut self, id: WindowId) -> Option<Rect> {
        self.entries.remove(&id).map(|e| e.original)
    }

    pub fn peek(&self, id: WindowId) -> Option<Rect> {
        self.entries.get(&id).map(|e| e.original)
    }

    /// Drops the entry for a window that no longer exists.
    pub fn forget(&mut self, id: WindowId) {
        self.entries.remove(&id);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > MAX_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(id, _)| *id)
            {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: Rect = Rect::new(100.0, 100.0, 800.0, 600.0);
    const LEFT: Rect = Rect::new(0.0, 0.0, 960.0, 1040.0);
    const RIGHT: Rect = Rect::new(960.0, 0.0, 960.0, 1040.0);

    #[test]
    fn restores_the_first_recorded_rect_across_repeated_actions() {
        let mut history = WindowHistory::new();
        history.record(1, ORIGINAL, LEFT);
        history.record(1, LEFT, RIGHT);
        history.record(1, RIGHT, LEFT);
        assert_eq!(history.take(1), Some(ORIGINAL));
    }

    #[test]
    fn a_user_move_resets_the_baseline() {
        let mut history = WindowHistory::new();
        history.record(1, ORIGINAL, LEFT);
        // The user dragged the window somewhere Tile did not put it.
        let dragged = Rect::new(500.0, 500.0, 400.0, 300.0);
        history.record(1, dragged, RIGHT);
        assert_eq!(history.take(1), Some(dragged));
    }

    #[test]
    fn take_consumes_and_windows_are_independent() {
        let mut history = WindowHistory::new();
        history.record(1, ORIGINAL, LEFT);
        history.record(2, RIGHT, LEFT);
        assert_eq!(history.take(1), Some(ORIGINAL));
        assert_eq!(history.take(1), None);
        assert_eq!(history.take(2), Some(RIGHT));
    }

    #[test]
    fn unknown_window_restores_to_nothing() {
        let mut history = WindowHistory::new();
        assert_eq!(history.take(42), None);
    }

    #[test]
    fn history_is_bounded() {
        let mut history = WindowHistory::new();
        for id in 0..(MAX_ENTRIES as u64 + 50) {
            history.record(id, ORIGINAL, LEFT);
        }
        assert!(history.len() <= MAX_ENTRIES);
        // The most recent window must survive eviction.
        assert!(history.peek(MAX_ENTRIES as u64 + 49).is_some());
    }
}
