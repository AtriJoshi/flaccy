//! A gesture-grained undo history: one step per gesture, however many edits
//! the gesture made. A drag edits the profile on every pointer motion, so the
//! stack remembers the state the gesture started from and only records a step
//! when the gesture finishes having changed something.

const DEPTH: usize = 100;

pub struct UndoStack<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    gesture_base: Option<T>,
}

impl<T: Clone + PartialEq> UndoStack<T> {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            gesture_base: None,
        }
    }

    /// Marks the start of a gesture. A second begin before the first finishes
    /// keeps the original base, so nested handlers cannot split one gesture.
    pub fn begin(&mut self, base: T) {
        if self.gesture_base.is_none() {
            self.gesture_base = Some(base);
        }
    }

    pub fn is_in_gesture(&self) -> bool {
        self.gesture_base.is_some()
    }

    /// Ends the gesture. It becomes one undo step when `current` differs from
    /// where it began; a gesture that changed nothing leaves no trace.
    pub fn finish(&mut self, current: &T) -> bool {
        let Some(base) = self.gesture_base.take() else {
            return false;
        };
        if &base == current {
            return false;
        }
        self.record(base);
        true
    }

    fn record(&mut self, previous: T) {
        self.undo.push(previous);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[cfg(test)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The state to restore, with `current` kept for redo.
    pub fn undo(&mut self, current: T) -> Option<T> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    /// The state to restore, with `current` kept for undo.
    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
}

impl<T: Clone + PartialEq> Default for UndoStack<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_drag_is_one_step() {
        let mut stack = UndoStack::new();
        stack.begin(0);
        stack.begin(1);
        assert!(stack.finish(&5));
        assert_eq!(stack.undo(5), Some(0));
        assert!(!stack.can_undo());
        assert!(stack.can_redo());
    }

    #[test]
    fn a_gesture_that_changed_nothing_leaves_no_step() {
        let mut stack = UndoStack::new();
        stack.begin(3);
        assert!(!stack.finish(&3));
        assert!(!stack.can_undo());
        assert!(!stack.finish(&9));
    }

    #[test]
    fn undo_and_redo_walk_the_same_line() {
        let mut stack = UndoStack::new();
        for (from, to) in [(0, 1), (1, 2), (2, 3)] {
            stack.begin(from);
            stack.finish(&to);
        }
        assert_eq!(stack.undo(3), Some(2));
        assert_eq!(stack.undo(2), Some(1));
        assert_eq!(stack.redo(1), Some(2));
        assert_eq!(stack.redo(2), Some(3));
        assert_eq!(stack.redo(3), None);
        assert_eq!(stack.undo(3), Some(2));
    }

    #[test]
    fn a_new_gesture_forgets_the_redo_line() {
        let mut stack = UndoStack::new();
        stack.begin(0);
        stack.finish(&1);
        stack.undo(1);
        assert!(stack.can_redo());
        stack.begin(0);
        stack.finish(&7);
        assert!(!stack.can_redo());
    }

    #[test]
    fn the_history_is_bounded() {
        let mut stack = UndoStack::new();
        for step in 0..150 {
            stack.begin(step);
            stack.finish(&(step + 1));
        }
        let mut count = 0;
        let mut current = 150;
        while let Some(previous) = stack.undo(current) {
            current = previous;
            count += 1;
        }
        assert_eq!(count, 100);
        assert_eq!(current, 50);
    }
}
