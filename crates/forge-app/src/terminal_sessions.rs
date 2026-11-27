//! Owns tab lifecycle; inactive PTYs stay alive but cannot receive focus.
use super::embedded_terminal::EmbeddedTerminal;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub(super) struct TerminalSessions {
    // The workbench may route input and render the active PTY; all tab
    // lifecycle operations and inactive-session mutation stay here.
    pub(super) active: Option<EmbeddedTerminal>,
    inactive: Vec<EmbeddedTerminal>,
}

impl TerminalSessions {
    pub(super) fn inactive(&self) -> &[EmbeddedTerminal] {
        &self.inactive
    }

    pub(super) fn can_open(&self) -> bool {
        self.inactive.len() < 7
            || self
                .active
                .as_ref()
                .is_none_or(|t| t.ended.load(Ordering::Acquire))
    }

    pub(super) fn install(&mut self, mut next: EmbeddedTerminal) {
        next.maximized = self.active.as_ref().is_some_and(|t| t.maximized);
        if let Some(mut previous) = self.active.take() {
            if !previous.ended.load(Ordering::Acquire) {
                previous.set_visible(false);
                self.inactive.push(previous);
            }
        }
        next.activate();
        self.active = Some(next);
    }

    pub(super) fn activate_number(&mut self, position: usize) {
        let mut ids: Vec<_> = self
            .inactive
            .iter()
            .chain(self.active.iter())
            .map(|t| t.id)
            .collect();
        ids.sort_unstable();
        if let Some(id) = ids.get(position).copied() {
            self.activate(id);
        }
    }

    pub(super) fn switch(&mut self, backwards: bool) {
        if self.inactive.is_empty() || self.active.is_none() {
            return;
        }
        let index = if backwards {
            self.inactive.len() - 1
        } else {
            0
        };
        self.activate(self.inactive[index].id);
    }

    pub(super) fn activate(&mut self, id: u64) {
        if let Some(term) = self.active.as_mut().filter(|t| t.id == id) {
            term.set_focused(true);
            return;
        }
        let Some(index) = self.inactive.iter().position(|term| term.id == id) else {
            return;
        };
        let Some(mut previous) = self.active.take() else {
            return;
        };
        let mut next = self.inactive.remove(index);
        next.activate();
        // Maximization belongs to the displayed panel, not a background PTY.
        next.maximized = previous.maximized;
        previous.set_visible(false);
        self.inactive.push(previous);
        self.active = Some(next);
    }

    pub(super) fn close_active(&mut self) -> bool {
        let Some(previous) = self.active.take() else {
            return false;
        };
        let maximized = previous.maximized;
        drop(previous);
        if let Some(mut next) = self.inactive.pop() {
            next.maximized = maximized;
            next.activate();
            self.active = Some(next);
        }
        true
    }

    pub(super) fn close_by_id(&mut self, id: u64) -> bool {
        if self.active.as_ref().is_some_and(|t| t.id == id) {
            return self.close_active();
        }
        if let Some(index) = self.inactive.iter().position(|t| t.id == id) {
            self.inactive.remove(index);
            return true;
        }
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn tabs_keep_panel_maximization_and_background_sessions_unfocused() {
        let dir = tempfile::tempdir().unwrap();
        let notify = Arc::new(tokio::sync::Notify::new());
        let mut sessions = TerminalSessions::default();
        sessions.install(EmbeddedTerminal::start(dir.path(), 80, 12, notify.clone()).unwrap());
        let first_id = sessions.active.as_ref().unwrap().id;
        sessions.active.as_mut().unwrap().maximized = true;
        sessions.install(EmbeddedTerminal::start(dir.path(), 80, 12, notify).unwrap());
        let second_id = sessions.active.as_ref().unwrap().id;
        assert!(sessions.active.as_ref().unwrap().maximized);
        assert!(!sessions.inactive()[0].is_focused());
        assert!(!sessions.inactive()[0].is_visible());
        sessions.active.as_mut().unwrap().maximized = false;
        sessions.activate(first_id);
        assert_eq!(sessions.active.as_ref().unwrap().id, first_id);
        assert!(!sessions.active.as_ref().unwrap().maximized);
        sessions.active.as_mut().unwrap().maximized = true;
        assert!(sessions.close_active());
        assert_eq!(sessions.active.as_ref().unwrap().id, second_id);
        assert!(sessions.active.as_ref().unwrap().maximized);
        assert!(sessions.active.as_ref().unwrap().is_focused());
        assert!(sessions.inactive().is_empty());
    }
}
