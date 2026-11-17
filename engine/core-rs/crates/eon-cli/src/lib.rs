//! Read-only journal status and its terminal rendering (docs/implementation/05, section 5).
//!
//! The status is computed from the journal on disk, so the CLI shows what is durable rather than what a process
//! believes. Rendering goes through ratatui, and the same widget is used by the binary and by tests.

use std::fs;
use std::path::Path;

use eon_foundation::journal::replay;
use eon_primitives::{EonError, EonResult};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalStatus {
    pub segments: usize,
    pub frames: usize,
    pub last_seq: Option<u64>,
}

pub fn read_status(dir: &Path) -> EonResult<JournalStatus> {
    let segments = fs::read_dir(dir)
        .map_err(|e| EonError::OperationFailed { operation: "status.read_dir".into(), reason: e.to_string() })?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "eonj"))
        .count();
    let frames = replay(dir)?;
    Ok(JournalStatus {
        segments,
        frames: frames.len(),
        last_seq: frames.last().map(|f| f.seq_no),
    })
}

pub fn render_text(status: &JournalStatus) -> String {
    let last = status.last_seq.map_or("none".to_string(), |s| s.to_string());
    format!(
        "segments: {}\nframes: {}\nlast durable seq: {}\n",
        status.segments, status.frames, last
    )
}

fn draw_body(frame: &mut ratatui::Frame, title: &str, body: &str) {
    let widget = Paragraph::new(body.to_owned()).block(Block::default().borders(Borders::ALL).title(title.to_owned()));
    frame.render_widget(widget, Rect::new(0, 0, frame.area().width, frame.area().height));
}

fn draw(frame: &mut ratatui::Frame, status: &JournalStatus) {
    draw_body(frame, " EON journal ", &render_text(status));
}

/// Live view state for `eon watch`. Each refresh reads the journal from disk, so the view shows durable state only.
#[derive(Debug, Default)]
pub struct WatchState {
    pub status: Option<JournalStatus>,
    pub error: Option<String>,
    pub quit: bool,
}

impl WatchState {
    pub fn refresh(&mut self, dir: &Path) {
        match read_status(dir) {
            Ok(status) => {
                self.status = Some(status);
                self.error = None;
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    /// `q` or `Q` requests exit. Other keys are ignored.
    pub fn on_key(&mut self, key: char) {
        if matches!(key, 'q' | 'Q') {
            self.quit = true;
        }
    }

    fn body(&self) -> String {
        match (&self.error, &self.status) {
            (Some(err), _) => format!("error: {err}\n\npress q to quit"),
            (None, Some(status)) => format!("{}\npress q to quit\n", render_text(status)),
            (None, None) => "waiting for the journal\npress q to quit".to_owned(),
        }
    }
}

/// Renders a watch state into an in-memory terminal and returns the visible text.
pub fn render_watch_snapshot(state: &WatchState, width: u16, height: u16) -> EonResult<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|e| EonError::InternalError(format!("terminal: {e}")))?;
    terminal
        .draw(|frame| draw_body(frame, " eon watch ", &state.body()))
        .map_err(|e| EonError::InternalError(format!("draw: {e}")))?;
    let buffer = terminal.backend().buffer();
    let lines: Vec<String> = (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
        .collect();
    Ok(lines.join("\n"))
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

/// Runs the interactive view until `q` is pressed. Requires a terminal. The terminal is restored on every exit path.
pub fn run_watch(dir: &Path, poll: std::time::Duration) -> EonResult<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};

    let io_err = |context: &str, e: std::io::Error| EonError::OperationFailed {
        operation: format!("watch.{context}"),
        reason: e.to_string(),
    };
    enable_raw_mode().map_err(|e| io_err("raw_mode", e))?;
    let _guard = TerminalGuard;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen).map_err(|e| io_err("alternate_screen", e))?;
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))
        .map_err(|e| io_err("terminal", e))?;

    let mut state = WatchState::default();
    loop {
        state.refresh(dir);
        terminal
            .draw(|frame| draw_body(frame, " eon watch ", &state.body()))
            .map_err(|e| io_err("draw", e))?;
        if event::poll(poll).map_err(|e| io_err("poll", e))?
            && let Event::Key(key) = event::read().map_err(|e| io_err("read", e))?
            && key.kind == KeyEventKind::Press
            && let KeyCode::Char(c) = key.code
        {
            state.on_key(c);
        }
        if state.quit {
            return Ok(());
        }
    }
}

/// Renders the status into an in-memory terminal and returns the visible text, one line per row.
pub fn render_snapshot(status: &JournalStatus, width: u16, height: u16) -> EonResult<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|e| EonError::InternalError(format!("terminal: {e}")))?;
    terminal
        .draw(|frame| draw(frame, status))
        .map_err(|e| EonError::InternalError(format!("draw: {e}")))?;
    let buffer = terminal.backend().buffer();
    let lines: Vec<String> = (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
        .collect();
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use eon_foundation::journal::JournalWriter;

    use super::*;

    #[test]
    fn status_reflects_what_is_durable_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open_with_segment_size(dir.path(), 128).unwrap();
        for i in 0..20 {
            writer.append_batch(&[format!("event {i}").as_bytes()]).unwrap();
        }
        drop(writer);
        let status = read_status(dir.path()).unwrap();
        assert_eq!(status.frames, 20);
        assert_eq!(status.last_seq, Some(20));
        assert!(status.segments > 1);
    }

    #[test]
    fn empty_journal_reports_no_durable_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let status = read_status(dir.path()).unwrap();
        assert_eq!(status, JournalStatus { segments: 0, frames: 0, last_seq: None });
        assert!(render_text(&status).contains("last durable seq: none"));
    }

    #[test]
    fn terminal_snapshot_shows_the_counts() {
        let status = JournalStatus { segments: 2, frames: 41, last_seq: Some(41) };
        let screen = render_snapshot(&status, 40, 6).unwrap();
        assert!(screen.contains("EON journal"));
        assert!(screen.contains("frames: 41"));
        assert!(screen.contains("last durable seq: 41"));
    }

    #[test]
    fn only_q_requests_exit_from_the_watch_view() {
        let mut state = WatchState::default();
        state.on_key('x');
        assert!(!state.quit);
        state.on_key('Q');
        assert!(state.quit);
    }

    #[test]
    fn watch_refresh_reads_the_durable_journal_and_reports_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        writer.append_batch(&[b"a".as_slice(), b"b".as_slice()]).unwrap();
        drop(writer);

        let mut state = WatchState::default();
        state.refresh(dir.path());
        assert_eq!(state.status.as_ref().map(|s| s.frames), Some(2));
        assert!(state.error.is_none());

        let mut missing = WatchState::default();
        missing.refresh(&dir.path().join("does-not-exist"));
        assert!(missing.status.is_none());
        assert!(missing.error.is_some());
    }

    #[test]
    fn watch_snapshot_shows_status_or_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        writer.append_batch(&[b"a".as_slice()]).unwrap();
        drop(writer);

        let mut state = WatchState::default();
        state.refresh(dir.path());
        let text = render_watch_snapshot(&state, 60, 8).unwrap();
        assert!(text.contains("frames: 1"), "{text}");
        assert!(text.contains("press q to quit"), "{text}");

        let mut failed = WatchState::default();
        failed.refresh(&dir.path().join("missing"));
        let text = render_watch_snapshot(&failed, 80, 8).unwrap();
        assert!(text.contains("error:"), "{text}");
    }
}
