//! Input protocol negotiated by the active foreground PTY job.
//!
//! The VT screen records the *last* DECSET value, not which application
//! requested it. These two facts must not be conflated: a crashed TUI can
//! leave mouse tracking / bracketed paste / application cursor enabled after
//! the shell regains foreground control. Treat each foreground process group
//! as a separate protocol generation and reset input modes on handoff.
//!
//! The same owner transition is observed when consuming PTY output and when
//! handling user input. That makes it safe if a job exits without producing
//! any more output (or its prompt has not yet been rendered).
use vt100::{MouseProtocolEncoding, MouseProtocolMode, Parser};

// DEC input modes are scoped to the foreground job, not to the lifetime of
// the PTY. Clear them without erasing scrollback or ordinary screen content.
// This is a local VT decoder reset; bytes are NEVER written to the child.
const RESET_INPUT_MODES: &[u8] =
    b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?2004l\x1b[?1l";

#[derive(Default, Debug)]
pub(super) struct InputProtocol {
    foreground: Option<i32>,
    mouse_owner: Option<i32>,
}

impl InputProtocol {
    /// Advances the protocol epoch and releases every input capability
    /// negotiated by the previous foreground job.
    pub(super) fn sync(&mut self, parser: &mut Parser, foreground: Option<i32>) {
        if self.foreground == foreground {
            return;
        }
        self.foreground = foreground;
        self.mouse_owner = None;
        parser.process(RESET_INPUT_MODES);
        // Alternate-screen ownership is tied to the foreground application.
        // A crash cannot leave the command prompt hidden on a stale screen.
        if parser.screen().alternate_screen() {
            parser.process(b"\x1b[?1049l");
        }
    }

    pub(super) fn output(&mut self, parser: &mut Parser, foreground: Option<i32>, bytes: &[u8]) {
        self.sync(parser, foreground);
        let previous = parser.screen().mouse_protocol_mode();
        parser.process(bytes);
        let mode = parser.screen().mouse_protocol_mode();
        if mode == MouseProtocolMode::None {
            self.mouse_owner = None;
        } else if mode != previous {
            // A new explicit mouse negotiation grants the foreground job the
            // lease. The same mode cannot leak across a process-group change.
            self.mouse_owner = foreground;
        }
    }

    pub(super) fn mouse(
        &mut self,
        parser: &mut Parser,
        foreground: Option<i32>,
    ) -> Option<(MouseProtocolMode, MouseProtocolEncoding)> {
        self.sync(parser, foreground);
        if foreground.is_none() || self.mouse_owner != foreground {
            return None;
        }
        let screen = parser.screen();
        let mode = screen.mouse_protocol_mode();
        (mode != MouseProtocolMode::None).then_some((mode, screen.mouse_protocol_encoding()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    fn parser() -> Parser {
        Parser::new(
            NonZeroU16::new(20).unwrap(),
            NonZeroU16::new(80).unwrap(),
            100,
        )
    }

    #[test]
    fn foreground_change_releases_all_input_modes_without_erasing_text() {
        let mut p = parser();
        let mut state = InputProtocol::default();
        state.output(
            &mut p,
            Some(101),
            b"HELLO\x1b[?1049h\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[?1h",
        );
        assert!(p.screen().alternate_screen());
        assert!(p.screen().bracketed_paste());
        assert!(p.screen().application_cursor());
        assert!(state.mouse(&mut p, Some(101)).is_some());
        state.sync(&mut p, Some(202));
        assert_eq!(p.screen().mouse_protocol_mode(), MouseProtocolMode::None);
        assert!(!p.screen().alternate_screen());
        assert!(!p.screen().bracketed_paste());
        assert!(!p.screen().application_cursor());
        assert_eq!(
            p.screen().cell(0, 0).unwrap().contents(),
            "H",
            "foreground cleanup must preserve the primary screen"
        );
        assert!(state.mouse(&mut p, Some(202)).is_none());

        // A new job explicitly requesting the same protocol can own it again.
        state.output(&mut p, Some(202), b"\x1b[?1003h\x1b[?1006h");
        assert!(state.mouse(&mut p, Some(202)).is_some());
        // The job can also explicitly release its own lease.
        state.output(&mut p, Some(202), b"\x1b[?1003l");
        assert!(state.mouse(&mut p, Some(202)).is_none());
    }

    #[test]
    fn shell_return_without_any_pty_output_revokes_mouse_immediately() {
        let mut p = parser();
        let mut state = InputProtocol::default();
        state.output(&mut p, Some(401), b"\x1b[?1003h\x1b[?1006h");
        assert!(state.mouse(&mut p, Some(401)).is_some());
        assert!(state.mouse(&mut p, Some(10)).is_none());
        assert_eq!(p.screen().mouse_protocol_mode(), MouseProtocolMode::None);
        // Another job must explicitly renegotiate; it cannot inherit stale
        // DECSET from the process that was previously in the foreground.
        assert!(state.mouse(&mut p, Some(502)).is_none());
    }

    #[test]
    fn output_owner_switch_is_detected_before_next_job_negotiates() {
        let mut p = parser();
        let mut state = InputProtocol::default();
        state.output(&mut p, Some(400), b"\x1b[?1003h");
        state.output(&mut p, Some(500), b"\x1b[?1003h\x1b[?1006h");
        assert!(state.mouse(&mut p, Some(500)).is_some());
        assert!(state.mouse(&mut p, Some(400)).is_none());
    }
}
