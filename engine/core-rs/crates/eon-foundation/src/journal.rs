//! Segmented append-only event journal (see docs/implementation/01, section 3.1).
//!
//! Frames are written as `[32-byte header][payload]`. Each frame carries a
//! CRC32 over header bytes `4..24` and the payload. Segments roll over at a
//! byte threshold, and `append_batch` issues a single write and a single
//! `sync_data` for the whole batch (group commit).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use eon_primitives::{EonError, EonResult};

pub const FRAME_MAGIC: u32 = 0x454F_4E4A;
pub const FRAME_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;
pub const DEFAULT_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
/// Header flag: the stored payload is a zstd frame. Replay decompresses it and never returns this bit.
pub const FLAG_ZSTD: u16 = 0x0001;
/// Payloads below this size are never compressed, since the frame overhead would dominate.
pub const COMPRESS_MIN_BYTES: usize = 256;
const ZSTD_LEVEL: i32 = 3;

const SEGMENT_PREFIX: &str = "seg_";
const SEGMENT_SUFFIX: &str = ".eonj";
const INDEX_SUFFIX: &str = ".idx";
/// One index entry per this many frames. Replay seeks to the nearest entry at or before the wanted sequence.
pub const INDEX_STRIDE: u64 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq_no: u64,
    pub mono_ts_ns: u64,
    pub flags: u16,
    pub payload: Vec<u8>,
}

pub struct JournalWriter {
    dir: PathBuf,
    file: File,
    segment_first_seq: u64,
    segment_len: u64,
    next_seq: u64,
    max_segment_bytes: u64,
    compress: bool,
}

impl JournalWriter {
    /// Opens the journal in `dir`, repairing a torn tail if the last write was interrupted.
    pub fn open(dir: impl AsRef<Path>) -> EonResult<Self> {
        Self::open_with_segment_size(dir, DEFAULT_SEGMENT_BYTES)
    }

    pub fn open_with_segment_size(
        dir: impl AsRef<Path>,
        max_segment_bytes: u64,
    ) -> EonResult<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).map_err(|e| io_err("journal.create_dir", e))?;

        let segments = list_segments(&dir)?;
        let Some((first_seq, path)) = segments.last().cloned() else {
            let file = create_segment(&dir, 1)?;
            return Ok(Self {
                dir,
                file,
                segment_first_seq: 1,
                segment_len: 0,
                next_seq: 1,
                max_segment_bytes,
                compress: false,
            });
        };

        let bytes = fs::read(&path).map_err(|e| io_err("journal.read_segment", e))?;
        let scan = scan_segment(&bytes, first_seq)?;
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| io_err("journal.open_segment", e))?;
        if scan.valid_end < bytes.len() {
            log_torn_tail(&path, scan.valid_end);
            file.set_len(scan.valid_end as u64)
                .map_err(|e| io_err("journal.truncate_tail", e))?;
            file.sync_all()
                .map_err(|e| io_err("journal.sync_truncate", e))?;
        }
        drop(file);

        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| io_err("journal.append_segment", e))?;
        Ok(Self {
            dir,
            file,
            segment_first_seq: first_seq,
            segment_len: scan.valid_end as u64,
            next_seq: scan.next_seq,
            max_segment_bytes,
            compress: false,
        })
    }

    /// Enables zstd compression for payloads of at least `COMPRESS_MIN_BYTES`. A payload is stored compressed only
    /// when that is smaller than the original, so enabling this never grows the journal.
    pub fn set_compression(&mut self, enabled: bool) {
        self.compress = enabled;
    }

    /// Appends payloads as one group commit. Returns the sequence number assigned to each payload.
    pub fn append_batch(&mut self, payloads: &[&[u8]]) -> EonResult<Vec<u64>> {
        if payloads.is_empty() {
            return Ok(Vec::new());
        }
        let mut buf = Vec::new();
        let mut seqs = Vec::with_capacity(payloads.len());
        let ts = mono_ns();
        for (i, payload) in payloads.iter().enumerate() {
            if payload.len() > MAX_PAYLOAD_LEN {
                return Err(EonError::ValidationError(format!(
                    "journal payload of {} bytes exceeds {MAX_PAYLOAD_LEN}",
                    payload.len()
                )));
            }
            let seq = self.next_seq + i as u64;
            let compressed = if self.compress && payload.len() >= COMPRESS_MIN_BYTES {
                zstd::bulk::compress(payload, ZSTD_LEVEL).map_err(|e| io_err("journal.compress", e))?
            } else {
                Vec::new()
            };
            if !compressed.is_empty() && compressed.len() < payload.len() {
                encode_frame(seq, ts, FLAG_ZSTD, &compressed, &mut buf);
            } else {
                encode_frame(seq, ts, 0, payload, &mut buf);
            }
            seqs.push(seq);
        }

        if self.segment_len > 0 && self.segment_len + buf.len() as u64 > self.max_segment_bytes {
            self.roll_segment()?;
        }

        if let Err(e) = self.file.write_all(&buf) {
            // Remove any partial bytes so the next append does not land after a torn frame.
            let _ = self.file.set_len(self.segment_len);
            return Err(io_err("journal.write", e));
        }
        self.file
            .sync_data()
            .map_err(|e| io_err("journal.sync_data", e))?;

        self.segment_len += buf.len() as u64;
        self.next_seq += payloads.len() as u64;
        Ok(seqs)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    fn roll_segment(&mut self) -> EonResult<()> {
        self.file
            .sync_all()
            .map_err(|e| io_err("journal.sync_before_roll", e))?;
        let sealed = fs::read(segment_path(&self.dir, self.segment_first_seq))
            .map_err(|e| io_err("journal.read_sealed", e))?;
        write_index(&self.dir, self.segment_first_seq, &sealed)?;
        self.file = create_segment(&self.dir, self.next_seq)?;
        self.segment_first_seq = self.next_seq;
        self.segment_len = 0;
        Ok(())
    }
}

/// Reads every frame across all segments in order. Any invalid frame is an error;
/// call `JournalWriter::open` first so a torn tail has already been repaired.
pub fn replay(dir: impl AsRef<Path>) -> EonResult<Vec<Frame>> {
    let mut frames = Vec::new();
    let mut expected_seq = None;
    for (first_seq, path) in list_segments(dir.as_ref())? {
        let bytes = fs::read(&path).map_err(|e| io_err("journal.read_segment", e))?;
        let scan = scan_segment(&bytes, expected_seq.unwrap_or(first_seq))?;
        if scan.valid_end != bytes.len() {
            return Err(EonError::InternalError(format!(
                "Invalid frame at offset {} in {}",
                scan.valid_end,
                path.display()
            )));
        }
        let mut offset = 0;
        while offset < bytes.len() {
            let (frame, used) = decode_at(&bytes[offset..]).expect("validated by scan_segment");
            frames.push(frame);
            offset += used;
        }
        expected_seq = Some(scan.next_seq);
    }
    Ok(frames)
}

fn index_path(dir: &Path, first_seq: u64) -> PathBuf {
    dir.join(format!("{SEGMENT_PREFIX}{first_seq:020}{INDEX_SUFFIX}"))
}

/// Writes `(seq, offset)` pairs as little-endian u64s for a sealed segment. Written to a temp file and renamed,
/// so a crash never leaves a half-written index in place.
fn write_index(dir: &Path, first_seq: u64, bytes: &[u8]) -> EonResult<()> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    let mut seq = first_seq;
    while let Some((frame, used)) = decode_at(&bytes[offset..]) {
        if frame.seq_no != seq {
            break;
        }
        if (seq - first_seq).is_multiple_of(INDEX_STRIDE) {
            entries.push((seq, offset as u64));
        }
        offset += used;
        seq += 1;
    }
    let mut out = Vec::with_capacity(entries.len() * 16);
    for (seq, offset) in entries {
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
    }
    let final_path = index_path(dir, first_seq);
    let tmp = final_path.with_extension("idx.tmp");
    fs::write(&tmp, &out).map_err(|e| io_err("journal.write_index", e))?;
    File::open(&tmp)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_err("journal.sync_index", e))?;
    fs::rename(&tmp, &final_path).map_err(|e| io_err("journal.rename_index", e))?;
    Ok(())
}

fn read_index(dir: &Path, first_seq: u64) -> Option<Vec<(u64, u64)>> {
    let bytes = fs::read(index_path(dir, first_seq)).ok()?;
    if !bytes.len().is_multiple_of(16) {
        return None;
    }
    Some(
        bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| {
                (
                    u64::from_le_bytes(c[0..8].try_into().expect("8 bytes")),
                    u64::from_le_bytes(c[8..16].try_into().expect("8 bytes")),
                )
            })
            .collect(),
    )
}

/// Returns every frame with `seq_no >= from_seq`. Sealed segments that end before `from_seq` are skipped entirely,
/// and within a segment the sparse index picks the starting offset. An index entry that does not point at a
/// matching frame is ignored, so a damaged index can slow replay but never change its result.
pub fn replay_from(dir: impl AsRef<Path>, from_seq: u64) -> EonResult<Vec<Frame>> {
    let dir = dir.as_ref();
    let segments = list_segments(dir)?;
    let mut frames = Vec::new();
    for (i, (first_seq, path)) in segments.iter().enumerate() {
        let ends_before_wanted = segments.get(i + 1).is_some_and(|(next_first, _)| *next_first <= from_seq);
        if ends_before_wanted {
            continue;
        }
        let bytes = fs::read(path).map_err(|e| io_err("journal.read_segment", e))?;
        let (mut offset, mut seq) = (0usize, *first_seq);
        let seek = read_index(dir, *first_seq)
            .and_then(|entries| entries.iter().rev().find(|(s, _)| *s <= from_seq).copied());
        if let Some((entry_seq, entry_offset)) = seek {
            let start = entry_offset as usize;
            let points_at_entry =
                decode_at(bytes.get(start..).unwrap_or(&[])).is_some_and(|(f, _)| f.seq_no == entry_seq);
            if points_at_entry {
                offset = start;
                seq = entry_seq;
            }
        }
        while let Some((frame, used)) = decode_at(&bytes[offset..]) {
            if frame.seq_no != seq {
                return Err(EonError::InternalError(format!("sequence gap in {}", path.display())));
            }
            if seq >= from_seq {
                frames.push(frame);
            }
            offset += used;
            seq += 1;
        }
    }
    Ok(frames)
}

struct SegmentScan {
    valid_end: usize,
    next_seq: u64,
}

/// Walks frames from the start of a segment. A bad frame followed by any valid frame is
/// mid-file corruption and is refused. A bad frame with nothing valid after it is a torn tail.
fn scan_segment(bytes: &[u8], first_seq: u64) -> EonResult<SegmentScan> {
    let mut offset = 0;
    let mut expected_seq = first_seq;
    while offset < bytes.len() {
        match decode_at(&bytes[offset..]) {
            Some((frame, used)) if frame.seq_no == expected_seq => {
                offset += used;
                expected_seq += 1;
            }
            _ => {
                let valid_after =
                    (offset + 1..bytes.len()).any(|o| decode_at(&bytes[o..]).is_some());
                if valid_after {
                    return Err(EonError::InternalError(format!(
                        "Mid-journal corruption detected at offset {offset}"
                    )));
                }
                if !is_unacknowledged_tail(&bytes[offset..]) {
                    return Err(EonError::InternalError(format!(
                        "Corrupt complete frame at tail offset {offset}; refusing to drop it. Run repair_torn_tail to truncate explicitly"
                    )));
                }
                break;
            }
        }
    }
    Ok(SegmentScan {
        valid_end: offset,
        next_seq: expected_seq,
    })
}

/// Frames are fsynced before they are acknowledged, so a torn write can only leave a prefix of an
/// unacknowledged frame, or zero bytes where the file size was extended before data reached disk.
/// A complete-length frame that fails its checksum is corruption, not a torn write.
fn is_unacknowledged_tail(rest: &[u8]) -> bool {
    if rest.iter().all(|&b| b == 0) || rest.len() < HEADER_LEN {
        return true;
    }
    let u32_at = |at: usize| u32::from_le_bytes(rest[at..at + 4].try_into().expect("4 bytes"));
    let magic_ok = u32_at(0) == FRAME_MAGIC && u16::from_le_bytes([rest[4], rest[5]]) == FRAME_VERSION;
    if !magic_ok {
        return false;
    }
    let payload_len = u32_at(28) as usize;
    payload_len <= MAX_PAYLOAD_LEN && rest.len() < HEADER_LEN + payload_len
}

/// Explicit operator repair: truncates the newest segment to its last valid frame, whatever the cause.
/// Returns the number of bytes removed. Use only after the refusal reason has been reviewed.
pub fn repair_torn_tail(dir: impl AsRef<Path>) -> EonResult<u64> {
    let Some((first_seq, path)) = list_segments(dir.as_ref())?.pop() else {
        return Ok(0);
    };
    let bytes = fs::read(&path).map_err(|e| io_err("journal.read_segment", e))?;
    let mut offset = 0;
    let mut expected = first_seq;
    while let Some((frame, used)) = decode_at(&bytes[offset..]) {
        if frame.seq_no != expected {
            break;
        }
        offset += used;
        expected += 1;
    }
    let removed = (bytes.len() - offset) as u64;
    if removed > 0 {
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| io_err("journal.open_segment", e))?;
        file.set_len(offset as u64)
            .map_err(|e| io_err("journal.truncate_tail", e))?;
        file.sync_all().map_err(|e| io_err("journal.sync_truncate", e))?;
    }
    Ok(removed)
}


fn encode_frame(seq_no: u64, mono_ts_ns: u64, flags: u16, payload: &[u8], out: &mut Vec<u8>) {
    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(&FRAME_MAGIC.to_le_bytes());
    header[4..6].copy_from_slice(&FRAME_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&flags.to_le_bytes());
    header[8..16].copy_from_slice(&seq_no.to_le_bytes());
    header[16..24].copy_from_slice(&mono_ts_ns.to_le_bytes());
    header[28..32].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    let crc = checksum(&header[4..24], payload);
    header[24..28].copy_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
}

fn decode_at(buf: &[u8]) -> Option<(Frame, usize)> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let u32_at = |at: usize| u32::from_le_bytes(buf[at..at + 4].try_into().expect("4 bytes"));
    if u32_at(0) != FRAME_MAGIC || u16::from_le_bytes([buf[4], buf[5]]) != FRAME_VERSION {
        return None;
    }
    let payload_len = u32_at(28) as usize;
    if payload_len > MAX_PAYLOAD_LEN || buf.len() < HEADER_LEN + payload_len {
        return None;
    }
    let stored = &buf[HEADER_LEN..HEADER_LEN + payload_len];
    if checksum(&buf[4..24], stored) != u32_at(24) {
        return None;
    }
    let mut flags = u16::from_le_bytes([buf[6], buf[7]]);
    let payload = if flags & FLAG_ZSTD != 0 {
        flags &= !FLAG_ZSTD;
        zstd::bulk::decompress(stored, MAX_PAYLOAD_LEN).ok()?
    } else {
        stored.to_vec()
    };
    let frame = Frame {
        seq_no: u64::from_le_bytes(buf[8..16].try_into().expect("8 bytes")),
        mono_ts_ns: u64::from_le_bytes(buf[16..24].try_into().expect("8 bytes")),
        flags,
        payload,
    };
    Some((frame, HEADER_LEN + payload_len))
}

fn checksum(header_body: &[u8], payload: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(header_body);
    hasher.update(payload);
    hasher.finalize()
}

fn segment_path(dir: &Path, first_seq: u64) -> PathBuf {
    dir.join(format!("{SEGMENT_PREFIX}{first_seq:020}{SEGMENT_SUFFIX}"))
}

fn create_segment(dir: &Path, first_seq: u64) -> EonResult<File> {
    let file = OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(segment_path(dir, first_seq))
        .map_err(|e| io_err("journal.create_segment", e))?;
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| io_err("journal.sync_dir", e))?;
    Ok(file)
}

fn list_segments(dir: &Path) -> EonResult<Vec<(u64, PathBuf)>> {
    let mut segments = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_err("journal.read_dir", e))? {
        let entry = entry.map_err(|e| io_err("journal.read_dir_entry", e))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(digits) = name
            .strip_prefix(SEGMENT_PREFIX)
            .and_then(|rest| rest.strip_suffix(SEGMENT_SUFFIX))
        else {
            continue;
        };
        if let Ok(first_seq) = digits.parse::<u64>() {
            segments.push((first_seq, entry.path()));
        }
    }
    segments.sort_by_key(|(first_seq, _)| *first_seq);
    Ok(segments)
}

fn mono_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn io_err(operation: &str, error: std::io::Error) -> EonError {
    EonError::OperationFailed {
        operation: operation.to_string(),
        reason: error.to_string(),
    }
}

fn log_torn_tail(path: &Path, offset: usize) {
    eprintln!(
        "journal: truncating torn tail at offset {offset} in {}",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payloads(n: usize) -> Vec<Vec<u8>> {
        (0..n).map(|i| format!("event-{i}").into_bytes()).collect()
    }

    fn refs(v: &[Vec<u8>]) -> Vec<&[u8]> {
        v.iter().map(Vec::as_slice).collect()
    }

    #[test]
    fn batch_round_trips_with_contiguous_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(5);
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        let seqs = writer.append_batch(&refs(&items)).unwrap();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);

        let frames = replay(dir.path()).unwrap();
        let replayed: Vec<Vec<u8>> = frames.iter().map(|f| f.payload.clone()).collect();
        assert_eq!(replayed, items);
        assert!(frames.iter().map(|f| f.seq_no).eq(1..=5));
    }

    #[test]
    fn reopen_continues_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(2);
        JournalWriter::open(dir.path())
            .unwrap()
            .append_batch(&refs(&items))
            .unwrap();

        let mut reopened = JournalWriter::open(dir.path()).unwrap();
        assert_eq!(reopened.next_seq(), 3);
        assert_eq!(reopened.append_batch(&[b"after-restart"]).unwrap(), vec![3]);
    }

    #[test]
    fn torn_tail_is_truncated_and_prior_frames_survive() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(3);
        JournalWriter::open(dir.path())
            .unwrap()
            .append_batch(&refs(&items))
            .unwrap();

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let clean_len = fs::metadata(&segment).unwrap().len();
        let mut partial = Vec::new();
        encode_frame(4, 0, 0, b"half-written", &mut partial);
        partial.truncate(HEADER_LEN + 3);
        fs::OpenOptions::new()
            .append(true)
            .open(&segment)
            .unwrap()
            .write_all(&partial)
            .unwrap();

        let mut writer = JournalWriter::open(dir.path()).unwrap();
        assert_eq!(fs::metadata(&segment).unwrap().len(), clean_len);
        assert_eq!(writer.append_batch(&[b"next"]).unwrap(), vec![4]);
        assert_eq!(replay(dir.path()).unwrap().len(), 4);
    }

    #[test]
    fn mid_journal_corruption_is_refused_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(3);
        JournalWriter::open(dir.path())
            .unwrap()
            .append_batch(&refs(&items))
            .unwrap();

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let mut bytes = fs::read(&segment).unwrap();
        let second_payload_at = HEADER_LEN + "event-0".len() + HEADER_LEN;
        bytes[second_payload_at] ^= 0xFF;
        fs::write(&segment, &bytes).unwrap();

        let err = JournalWriter::open(dir.path())
            .err()
            .expect("corruption must be refused");
        assert!(
            matches!(err, EonError::InternalError(msg) if msg.contains("Mid-journal corruption"))
        );
        assert_eq!(
            fs::read(&segment).unwrap(),
            bytes,
            "forensic bytes must not be truncated"
        );
    }

    #[test]
    fn corrupt_complete_tail_frame_is_refused_and_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(3);
        JournalWriter::open(dir.path()).unwrap().append_batch(&refs(&items)).unwrap();

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let mut bytes = fs::read(&segment).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        fs::write(&segment, &bytes).unwrap();

        let err = JournalWriter::open(dir.path()).err().expect("corrupt tail must be refused");
        assert!(matches!(err, EonError::InternalError(msg) if msg.contains("Corrupt complete frame")));
        assert_eq!(fs::read(&segment).unwrap(), bytes, "refused tail must not be modified");
    }

    #[test]
    fn zero_filled_tail_is_treated_as_unacknowledged_and_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(2);
        JournalWriter::open(dir.path()).unwrap().append_batch(&refs(&items)).unwrap();

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let clean_len = fs::metadata(&segment).unwrap().len();
        fs::OpenOptions::new()
            .append(true)
            .open(&segment)
            .unwrap()
            .write_all(&[0u8; 100])
            .unwrap();

        JournalWriter::open(dir.path()).unwrap();
        assert_eq!(fs::metadata(&segment).unwrap().len(), clean_len);
        assert_eq!(replay(dir.path()).unwrap().len(), 2);
    }

    #[test]
    fn explicit_repair_removes_a_corrupt_tail_frame() {
        let dir = tempfile::tempdir().unwrap();
        let items = payloads(3);
        JournalWriter::open(dir.path()).unwrap().append_batch(&refs(&items)).unwrap();

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let mut bytes = fs::read(&segment).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        fs::write(&segment, &bytes).unwrap();

        let removed = repair_torn_tail(dir.path()).unwrap();
        assert_eq!(removed as usize, HEADER_LEN + items[2].len());
        assert_eq!(replay(dir.path()).unwrap().len(), 2);
        JournalWriter::open(dir.path()).unwrap();
    }

    fn write_many(dir: &Path, count: usize) -> Vec<Vec<u8>> {
        let items: Vec<Vec<u8>> = (0..count).map(|i| format!("evt-{i:05}-{}", "x".repeat(40)).into_bytes()).collect();
        let mut writer = JournalWriter::open_with_segment_size(dir, 2048).unwrap();
        for item in &items {
            writer.append_batch(&[item.as_slice()]).unwrap();
        }
        items
    }

    #[test]
    fn sealed_segments_get_an_index_and_seeking_matches_full_replay() {
        let dir = tempfile::tempdir().unwrap();
        let items = write_many(dir.path(), 700);
        let indexes = fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "idx"))
            .count();
        assert!(indexes >= 2, "every sealed segment should have an index, found {indexes}");

        let full = replay(dir.path()).unwrap();
        for from in [1u64, 2, 63, 64, 65, 300, 699, 700] {
            let seeked = replay_from(dir.path(), from).unwrap();
            let expected: Vec<&Frame> = full.iter().filter(|f| f.seq_no >= from).collect();
            assert_eq!(seeked.len(), expected.len(), "from {from}");
            assert!(seeked.iter().zip(expected).all(|(a, b)| a == b), "from {from} differs");
        }
        assert_eq!(full.len(), items.len());
    }

    #[test]
    fn a_corrupt_index_slows_replay_but_never_changes_it() {
        let dir = tempfile::tempdir().unwrap();
        write_many(dir.path(), 300);
        let full = replay_from(dir.path(), 1).unwrap();

        for entry in fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|x| x == "idx") {
                let mut bytes = fs::read(&path).unwrap();
                for b in bytes.iter_mut().skip(8).step_by(16) {
                    *b = b.wrapping_add(7);
                }
                fs::write(&path, bytes).unwrap();
            }
        }
        assert_eq!(replay_from(dir.path(), 1).unwrap(), full);
        let target = full.iter().find(|f| f.seq_no == 150).unwrap().clone();
        assert_eq!(replay_from(dir.path(), 150).unwrap()[0], target);
    }

    #[test]
    fn replay_from_beyond_the_end_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        write_many(dir.path(), 10);
        assert!(replay_from(dir.path(), 11).unwrap().is_empty());
    }

    #[test]
    fn segments_roll_over_and_replay_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open_with_segment_size(dir.path(), 100).unwrap();
        let items = payloads(10);
        for item in &items {
            writer.append_batch(&[item.as_slice()]).unwrap();
        }
        assert!(list_segments(dir.path()).unwrap().len() > 1);

        let replayed: Vec<Vec<u8>> = replay(dir.path())
            .unwrap()
            .into_iter()
            .map(|f| f.payload)
            .collect();
        assert_eq!(replayed, items);
    }

    #[test]
    fn oversized_payload_is_rejected_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        let too_big = vec![0u8; MAX_PAYLOAD_LEN + 1];
        assert!(matches!(
            writer.append_batch(&[too_big.as_slice()]),
            Err(EonError::ValidationError(_))
        ));
        assert!(replay(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn compressed_payloads_round_trip_and_shrink_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b'a'; 4096];
        let small = b"tiny".to_vec();
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        writer.set_compression(true);
        writer.append_batch(&[big.as_slice(), small.as_slice()]).unwrap();

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let on_disk = fs::metadata(&segment).unwrap().len() as usize;
        assert!(on_disk < HEADER_LEN * 2 + big.len() + small.len(), "on-disk {on_disk} not compressed");

        let frames = replay(dir.path()).unwrap();
        assert_eq!(frames[0].payload, big);
        assert_eq!(frames[1].payload, small);
        assert_eq!(frames[0].flags & FLAG_ZSTD, 0, "replay must not expose the storage flag");
    }

    #[test]
    fn corrupt_compressed_frame_is_not_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        writer.set_compression(true);
        writer.append_batch(&[vec![b'z'; 4096].as_slice()]).unwrap();
        drop(writer);

        let segment = list_segments(dir.path()).unwrap().remove(0).1;
        let mut bytes = fs::read(&segment).unwrap();
        let mut header = [0u8; HEADER_LEN];
        header.copy_from_slice(&bytes[..HEADER_LEN]);
        let stored = &bytes[HEADER_LEN..];
        let garbage = vec![0x5Au8; stored.len()];
        let mut forged = Vec::new();
        encode_frame(1, u64::from_le_bytes(header[16..24].try_into().unwrap()), FLAG_ZSTD, &garbage, &mut forged);
        bytes = forged;
        fs::write(&segment, &bytes).unwrap();

        assert!(replay(dir.path()).is_err(), "undecodable complete frame must be refused, not dropped");
    }
}
