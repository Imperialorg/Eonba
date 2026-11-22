//! Crash and corruption tests for the journal (docs/implementation/01, section 3.1 and Invariant 7).
//!
//! The `crash_writer_child` test is a re-exec helper: when `EON_JOURNAL_CRASH_DIR` is set it becomes a
//! writer that never returns. The parent test SIGKILLs it at arbitrary points and checks that every frame
//! the child acknowledged on stdout is still present after recovery.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use eon_foundation::journal::{Frame, HEADER_LEN, JournalWriter, replay};
use proptest::prelude::*;

const CHILD_ENV: &str = "EON_JOURNAL_CRASH_DIR";

fn only_segment(dir: &Path) -> PathBuf {
    let mut segments: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "eonj"))
        .collect();
    segments.sort();
    assert_eq!(segments.len(), 1, "these fixtures stay within one segment");
    segments.remove(0)
}

fn write_payloads(dir: &Path, payloads: &[Vec<u8>]) {
    let mut writer = JournalWriter::open(dir).unwrap();
    for payload in payloads {
        writer.append_batch(&[payload.as_slice()]).unwrap();
    }
}

fn assert_contiguous_prefix(frames: &[Frame], payloads: &[Vec<u8>]) -> Result<(), TestCaseError> {
    prop_assert!(frames.len() <= payloads.len(), "replayed more frames than were written");
    for (i, frame) in frames.iter().enumerate() {
        prop_assert_eq!(frame.seq_no, i as u64 + 1);
        prop_assert_eq!(&frame.payload, &payloads[i], "frame {} was modified", i + 1);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// A crash can cut the file at any byte. Recovery must keep exactly the frames that ended before the cut.
    #[test]
    fn truncation_at_any_byte_recovers_the_complete_prefix(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..96), 1..12),
        cut_fraction in 0.0f64..=1.0,
    ) {
        let dir = tempfile::tempdir().unwrap();
        write_payloads(dir.path(), &payloads);
        let segment = only_segment(dir.path());
        let full_len = fs::metadata(&segment).unwrap().len();
        let cut = (full_len as f64 * cut_fraction) as u64;
        fs::OpenOptions::new().write(true).open(&segment).unwrap().set_len(cut).unwrap();

        let mut kept_end = 0u64;
        let mut expected_frames = 0usize;
        for payload in &payloads {
            let frame_end = kept_end + (HEADER_LEN + payload.len()) as u64;
            if frame_end > cut {
                break;
            }
            kept_end = frame_end;
            expected_frames += 1;
        }

        let reopened = JournalWriter::open(dir.path()).unwrap();
        let frames = replay(dir.path()).unwrap();
        prop_assert_eq!(frames.len(), expected_frames);
        assert_contiguous_prefix(&frames, &payloads)?;
        prop_assert_eq!(reopened.next_seq(), expected_frames as u64 + 1);
        prop_assert_eq!(fs::metadata(&segment).unwrap().len(), kept_end);
    }

    /// Flipping any single byte must never replay data that differs from what was written.
    /// Corruption before the final frame must be refused outright and leave the file untouched.
    #[test]
    fn single_byte_corruption_never_replays_altered_data(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..64), 2..8),
        position in any::<prop::sample::Index>(),
        mask in 1u8..=255,
    ) {
        let dir = tempfile::tempdir().unwrap();
        write_payloads(dir.path(), &payloads);
        let segment = only_segment(dir.path());
        let mut bytes = fs::read(&segment).unwrap();
        let at = position.index(bytes.len());
        bytes[at] ^= mask;
        fs::write(&segment, &bytes).unwrap();

        let mut frame_start = 0usize;
        let mut hit_frame = payloads.len() - 1;
        for (i, payload) in payloads.iter().enumerate() {
            let frame_end = frame_start + HEADER_LEN + payload.len();
            if at < frame_end {
                hit_frame = i;
                break;
            }
            frame_start = frame_end;
        }
        let must_refuse = hit_frame + 1 < payloads.len();

        match JournalWriter::open(dir.path()) {
            Err(_) => {
                prop_assert_eq!(fs::read(&segment).unwrap(), bytes, "refused journals must not be modified");
            }
            Ok(_) => {
                prop_assert!(!must_refuse, "corruption in non-final frame {} was accepted", hit_frame + 1);
                let frames = replay(dir.path()).unwrap();
                assert_contiguous_prefix(&frames, &payloads)?;
            }
        }
    }
}

#[test]
fn crash_writer_child() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let mut writer = JournalWriter::open(&dir).unwrap();
    loop {
        let base = writer.next_seq();
        let payloads: Vec<Vec<u8>> = (0..3).map(|k| format!("seq-{}", base + k).into_bytes()).collect();
        let refs: Vec<&[u8]> = payloads.iter().map(Vec::as_slice).collect();
        let seqs = writer.append_batch(&refs).unwrap();
        let mut out = std::io::stdout().lock();
        writeln!(out, "ACK {}", seqs.last().unwrap()).unwrap();
        out.flush().unwrap();
    }
}

#[test]
fn acknowledged_frames_survive_sigkill_and_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let mut max_acked = 0u64;

    for round in 0..4u64 {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_writer_child", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = thread::spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| line.strip_prefix("ACK ").and_then(|n| n.parse::<u64>().ok()))
                .last()
                .unwrap_or(0)
        });

        thread::sleep(Duration::from_millis(120 + 40 * round));
        child.kill().unwrap();
        child.wait().unwrap();
        let acked = reader.join().unwrap();
        max_acked = max_acked.max(acked);

        JournalWriter::open(dir.path()).unwrap();
        let frames = replay(dir.path()).unwrap();
        let last_durable = frames.last().map_or(0, |f| f.seq_no);
        assert!(
            last_durable >= max_acked,
            "round {round}: child acknowledged seq {max_acked} but only {last_durable} survived"
        );
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(frame.seq_no, i as u64 + 1, "round {round}: sequence gap");
            assert_eq!(frame.payload, format!("seq-{}", i + 1).into_bytes(), "round {round}: payload mismatch");
        }
    }

    assert!(max_acked >= 3, "the child never made progress; the test proved nothing");
}
