use std::{
    fmt,
    io::{self, Write},
};

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

pub trait TerminalOps: Send {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn enter_alt(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn enable_bracketed_paste(&mut self) -> io::Result<()>;
    fn disable_bracketed_paste(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
    fn leave_alt(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;

    fn report_restore_errors(&mut self, errors: &[String]) -> io::Result<()> {
        let mut stderr = io::stderr().lock();
        writeln!(
            stderr,
            "pinvou: terminal restoration encountered errors: {}",
            errors.join("; ")
        )
    }
}

pub struct CrosstermOps<W: Write + Send> {
    writer: W,
}

impl<W: Write + Send> CrosstermOps<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: Write + Send> TerminalOps for CrosstermOps<W> {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }
    fn enter_alt(&mut self) -> io::Result<()> {
        execute!(self.writer, EnterAlternateScreen)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Hide)
    }
    fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        execute!(self.writer, EnableBracketedPaste)
    }
    fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        execute!(self.writer, DisableBracketedPaste)
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Show)
    }
    fn leave_alt(&mut self) -> io::Result<()> {
        execute!(self.writer, LeaveAlternateScreen)
    }
    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

#[derive(Debug)]
pub struct TerminalRestoreError {
    failures: Vec<String>,
}

impl TerminalRestoreError {
    pub fn failures(&self) -> &[String] {
        &self.failures
    }
}

impl fmt::Display for TerminalRestoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.failures.join("; "))
    }
}

impl std::error::Error for TerminalRestoreError {}

pub struct TerminalGuard<O: TerminalOps> {
    ops: O,
    raw: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
    bracketed_paste: bool,
}

impl<O: TerminalOps> fmt::Debug for TerminalGuard<O> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalGuard")
            .field("raw", &self.raw)
            .field("alternate_screen", &self.alternate_screen)
            .field("cursor_hidden", &self.cursor_hidden)
            .field("bracketed_paste", &self.bracketed_paste)
            .finish_non_exhaustive()
    }
}

impl<O: TerminalOps> TerminalGuard<O> {
    pub fn enter(ops: O) -> io::Result<Self> {
        let mut guard = Self {
            ops,
            raw: false,
            alternate_screen: false,
            cursor_hidden: false,
            bracketed_paste: false,
        };

        if let Err(primary) = guard.enable_all() {
            let _ = guard.restore();
            return Err(primary);
        }
        Ok(guard)
    }

    fn enable_all(&mut self) -> io::Result<()> {
        self.ops.enable_raw()?;
        self.raw = true;
        self.ops.enter_alt()?;
        self.alternate_screen = true;
        self.ops.hide_cursor()?;
        self.cursor_hidden = true;
        self.ops.enable_bracketed_paste()?;
        self.bracketed_paste = true;
        Ok(())
    }

    pub fn restore(&mut self) -> Result<(), TerminalRestoreError> {
        let mut failures = Vec::new();
        if self.bracketed_paste {
            self.bracketed_paste = false;
            record_failure(
                &mut failures,
                "disable bracketed paste",
                self.ops.disable_bracketed_paste(),
            );
        }
        if self.cursor_hidden {
            self.cursor_hidden = false;
            record_failure(&mut failures, "show cursor", self.ops.show_cursor());
        }
        if self.alternate_screen {
            self.alternate_screen = false;
            record_failure(
                &mut failures,
                "leave alternate screen",
                self.ops.leave_alt(),
            );
        }
        if self.raw {
            self.raw = false;
            record_failure(&mut failures, "disable raw mode", self.ops.disable_raw());
        }

        if failures.is_empty() {
            Ok(())
        } else {
            // Reporting deliberately happens after the alternate-screen restoration attempt.
            let _ = self.ops.report_restore_errors(&failures);
            Err(TerminalRestoreError { failures })
        }
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn record_failure(failures: &mut Vec<String>, operation: &str, result: io::Result<()>) {
    if let Err(error) = result {
        failures.push(format!("{operation}: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        io,
        sync::{Arc, Mutex},
    };

    use super::{TerminalGuard, TerminalOps};

    #[derive(Clone, Default)]
    struct RecordingOps {
        calls: Arc<Mutex<Vec<String>>>,
        failures: Arc<Mutex<HashSet<&'static str>>>,
    }

    impl RecordingOps {
        fn failing(names: &[&'static str]) -> Self {
            Self {
                failures: Arc::new(Mutex::new(names.iter().copied().collect())),
                ..Self::default()
            }
        }

        fn record(&self, name: &'static str) -> io::Result<()> {
            self.calls.lock().unwrap().push(name.into());
            if self.failures.lock().unwrap().contains(name) {
                Err(io::Error::other(name))
            } else {
                Ok(())
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl TerminalOps for RecordingOps {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.record("enable_raw")
        }
        fn enter_alt(&mut self) -> io::Result<()> {
            self.record("enter_alt")
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.record("hide_cursor")
        }
        fn enable_bracketed_paste(&mut self) -> io::Result<()> {
            self.record("enable_paste")
        }
        fn disable_bracketed_paste(&mut self) -> io::Result<()> {
            self.record("disable_paste")
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.record("show_cursor")
        }
        fn leave_alt(&mut self) -> io::Result<()> {
            self.record("leave_alt")
        }
        fn disable_raw(&mut self) -> io::Result<()> {
            self.record("disable_raw")
        }
        fn report_restore_errors(&mut self, errors: &[String]) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("report:{}", errors.join(",")));
            if self.failures.lock().unwrap().contains("report") {
                Err(io::Error::other("report"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn normal_drop_restores_terminal_in_strict_reverse_order() {
        let ops = RecordingOps::default();
        {
            let _guard = TerminalGuard::enter(ops.clone()).unwrap();
        }
        assert_eq!(
            ops.calls(),
            [
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "enable_paste",
                "disable_paste",
                "show_cursor",
                "leave_alt",
                "disable_raw",
            ]
        );
    }

    #[test]
    fn every_partial_initialization_failure_restores_only_successful_steps() {
        let cases = [
            ("enable_raw", vec!["enable_raw"]),
            ("enter_alt", vec!["enable_raw", "enter_alt", "disable_raw"]),
            (
                "hide_cursor",
                vec![
                    "enable_raw",
                    "enter_alt",
                    "hide_cursor",
                    "leave_alt",
                    "disable_raw",
                ],
            ),
            (
                "enable_paste",
                vec![
                    "enable_raw",
                    "enter_alt",
                    "hide_cursor",
                    "enable_paste",
                    "show_cursor",
                    "leave_alt",
                    "disable_raw",
                ],
            ),
        ];
        for (failure, expected) in cases {
            let ops = RecordingOps::failing(&[failure]);
            let error = TerminalGuard::enter(ops.clone()).unwrap_err();
            assert_eq!(error.to_string(), failure);
            assert_eq!(ops.calls(), expected, "failure at {failure}");
        }
    }

    #[test]
    fn restore_continues_after_errors_and_reports_only_after_leaving_alt_screen() {
        let ops =
            RecordingOps::failing(&["disable_paste", "show_cursor", "leave_alt", "disable_raw"]);
        let mut guard = TerminalGuard::enter(ops.clone()).unwrap();
        let error = guard.restore().unwrap_err();
        assert_eq!(error.failures().len(), 4);
        let calls = ops.calls();
        assert_eq!(
            &calls[..8],
            [
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "enable_paste",
                "disable_paste",
                "show_cursor",
                "leave_alt",
                "disable_raw",
            ]
        );
        assert!(calls[8].starts_with("report:"));
        drop(guard);
        assert_eq!(ops.calls(), calls);
    }

    #[test]
    fn explicit_restore_then_drop_is_idempotent() {
        let ops = RecordingOps::default();
        let mut guard = TerminalGuard::enter(ops.clone()).unwrap();
        guard.restore().unwrap();
        let calls = ops.calls();
        drop(guard);
        assert_eq!(ops.calls(), calls);
    }

    #[test]
    fn enter_returns_primary_error_even_when_rollback_also_fails() {
        let ops = RecordingOps::failing(&["hide_cursor", "leave_alt", "disable_raw"]);
        let error = TerminalGuard::enter(ops.clone()).unwrap_err();
        assert_eq!(error.to_string(), "hide_cursor");
        let calls = ops.calls();
        assert_eq!(
            &calls[..5],
            [
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "leave_alt",
                "disable_raw"
            ]
        );
        assert!(calls[5].starts_with("report:"));
    }

    #[test]
    fn reporter_failure_never_interrupts_restoration_or_panics_in_drop() {
        let ops = RecordingOps::failing(&["show_cursor", "report"]);
        {
            let _guard = TerminalGuard::enter(ops.clone()).unwrap();
        }
        let calls = ops.calls();
        assert_eq!(
            &calls[..8],
            [
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "enable_paste",
                "disable_paste",
                "show_cursor",
                "leave_alt",
                "disable_raw",
            ]
        );
        assert!(calls[8].starts_with("report:"));
    }
}
