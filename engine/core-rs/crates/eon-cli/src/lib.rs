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

fn draw(frame: &mut ratatui::Frame, status: &JournalStatus) {
    let body = render_text(status);
    let widget = Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(" EON journal "));
    frame.render_widget(widget, Rect::new(0, 0, frame.area().width, frame.area().height));
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
}
