use std::collections::VecDeque;

const CAPACITY: usize = 100;

pub(crate) struct History<T> {
    past: VecDeque<T>,
    future: Vec<T>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            past: VecDeque::new(),
            future: Vec::new(),
        }
    }
}

impl<T> History<T> {
    pub(crate) fn push(&mut self, previous: T) {
        if self.past.len() == CAPACITY {
            self.past.pop_front();
        }
        self.past.push_back(previous);
        self.future.clear();
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub(crate) fn undo(&mut self, current: T) -> Option<T> {
        let previous = self.past.pop_back()?;
        self.future.push(current);
        Some(previous)
    }

    pub(crate) fn redo(&mut self, current: T) -> Option<T> {
        let next = self.future.pop()?;
        self.past.push_back(current);
        Some(next)
    }
}

#[cfg(target_arch = "wasm32")]
mod extract {
    use nvsf_core::{ExtractConfig, Rect};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) struct RegionId {
        pub(crate) excluded: bool,
        pub(crate) index: usize,
    }

    impl RegionId {
        pub(crate) fn list(self, config: &ExtractConfig) -> &[Rect] {
            if self.excluded {
                &config.exclusions
            } else {
                &config.regions
            }
        }

        pub(crate) fn list_mut(self, config: &mut ExtractConfig) -> &mut Vec<Rect> {
            if self.excluded {
                &mut config.exclusions
            } else {
                &mut config.regions
            }
        }
    }

    #[derive(PartialEq)]
    pub(crate) struct ExtractEdit {
        regions: Vec<Rect>,
        exclusions: Vec<Rect>,
        start: u64,
        end: Option<u64>,
    }

    impl ExtractEdit {
        pub(crate) fn snapshot(config: &ExtractConfig) -> Self {
            Self {
                regions: config.regions.clone(),
                exclusions: config.exclusions.clone(),
                start: config.start_ms,
                end: config.end_ms,
            }
        }

        pub(crate) fn restore(self, config: &mut ExtractConfig) {
            config.regions = self.regions;
            config.exclusions = self.exclusions;
            config.start_ms = self.start;
            config.end_ms = self.end;
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use extract::{ExtractEdit, RegionId};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_restore_the_previous_and_next_state() {
        let mut history = History::default();
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        history.push(10);
        assert_eq!(history.undo(20), Some(10));
        assert!(!history.can_undo());
        assert!(history.can_redo());
        assert_eq!(history.redo(10), Some(20));
        assert!(history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn a_new_edit_after_undo_discards_the_redo_branch() {
        let mut history = History::default();
        history.push(10);
        assert_eq!(history.undo(20), Some(10));
        history.push(10);
        assert_eq!(history.redo(30), None);
        assert_eq!(history.undo(30), Some(10));
    }

    #[test]
    fn only_the_last_hundred_edits_remain() {
        let mut history = History::default();
        for value in 0..103 {
            history.push(value);
        }
        let mut current = 103;
        for expected in (3..103).rev() {
            assert_eq!(history.undo(current), Some(expected));
            current = expected;
        }
        assert_eq!(history.undo(current), None);
    }
}
