//! Group-commit journal worker (docs/implementation/01, section 3.1).
//!
//! Callers enqueue payloads from any thread. One background thread drains everything queued at that
//! moment and writes it with a single `write` and a single `sync_data`. Each caller is acknowledged only
//! after that sync returns, so an acknowledged sequence number is durable.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};

use eon_primitives::{EonError, EonResult};

use crate::journal::{JournalWriter, MAX_PAYLOAD_LEN};

/// Upper bound on frames per commit, so one burst cannot hold acknowledgements back indefinitely.
pub const MAX_BATCH: usize = 512;

struct Request {
    payload: Vec<u8>,
    reply: Sender<EonResult<u64>>,
}

pub struct GroupCommitJournal {
    tx: Option<SyncSender<Request>>,
    worker: Option<JoinHandle<()>>,
}

impl GroupCommitJournal {
    /// Opens the journal in `dir` (repairing a torn tail, as `JournalWriter::open` does) and starts the worker.
    pub fn spawn(dir: impl AsRef<std::path::Path>, queue_depth: usize) -> EonResult<Self> {
        let mut writer = JournalWriter::open(dir)?;
        let (tx, rx) = mpsc::sync_channel::<Request>(queue_depth.max(1));
        let worker = thread::Builder::new()
            .name("eon-journal".into())
            .spawn(move || run(&mut writer, rx))
            .map_err(|e| EonError::InternalError(format!("cannot start journal worker: {e}")))?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
        })
    }

    /// Enqueues a payload. The receiver yields the assigned sequence number once it is durable.
    pub fn append(&self, payload: Vec<u8>) -> EonResult<Receiver<EonResult<u64>>> {
        if payload.len() > MAX_PAYLOAD_LEN {
            return Err(EonError::ValidationError(format!(
                "journal payload of {} bytes exceeds {MAX_PAYLOAD_LEN}",
                payload.len()
            )));
        }
        let (reply, ack) = mpsc::channel();
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| EonError::InternalError("journal is shutting down".into()))?;
        tx.send(Request { payload, reply })
            .map_err(|_| EonError::InternalError("journal worker stopped".into()))?;
        Ok(ack)
    }

    /// Scrubs secrets and invisible characters from `text`, then journals the result durably.
    /// Raw secrets never reach the log, so the journal stays safe to replicate and to show in a UI.
    pub fn append_scrubbed(&self, text: &str) -> EonResult<u64> {
        self.append_durable(crate::scrubber::scrub(text).into_bytes())
    }

    /// Enqueues and waits for durability.
    pub fn append_durable(&self, payload: Vec<u8>) -> EonResult<u64> {
        self.append(payload)?
            .recv()
            .map_err(|_| EonError::InternalError("journal worker dropped the request".into()))?
    }
}

impl Drop for GroupCommitJournal {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(writer: &mut JournalWriter, rx: Receiver<Request>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while batch.len() < MAX_BATCH {
            match rx.try_recv() {
                Ok(next) => batch.push(next),
                Err(_) => break,
            }
        }
        let payloads: Vec<&[u8]> = batch.iter().map(|r| r.payload.as_slice()).collect();
        match writer.append_batch(&payloads) {
            Ok(seqs) => {
                for (request, seq) in batch.iter().zip(seqs) {
                    let _ = request.reply.send(Ok(seq));
                }
            }
            Err(err) => {
                for request in &batch {
                    let _ = request.reply.send(Err(err.clone()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::replay;

    #[test]
    fn concurrent_producers_get_unique_contiguous_durable_sequences() {
        let dir = tempfile::tempdir().unwrap();
        let journal = std::sync::Arc::new(GroupCommitJournal::spawn(dir.path(), 64).unwrap());
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let journal = journal.clone();
                thread::spawn(move || {
                    (0..200)
                        .map(|i| journal.append_durable(format!("t{t}-{i}").into_bytes()).unwrap())
                        .collect::<Vec<u64>>()
                })
            })
            .collect();
        let mut seqs: Vec<u64> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=1600).collect::<Vec<u64>>());
        drop(journal);
        assert_eq!(replay(dir.path()).unwrap().len(), 1600);
    }

    #[test]
    fn acknowledged_events_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        {
            let journal = GroupCommitJournal::spawn(dir.path(), 8).unwrap();
            assert_eq!(journal.append_durable(b"first".to_vec()).unwrap(), 1);
            assert_eq!(journal.append_durable(b"second".to_vec()).unwrap(), 2);
        }
        let journal = GroupCommitJournal::spawn(dir.path(), 8).unwrap();
        assert_eq!(journal.append_durable(b"third".to_vec()).unwrap(), 3);
        drop(journal);
        let payloads: Vec<Vec<u8>> = replay(dir.path()).unwrap().into_iter().map(|f| f.payload).collect();
        assert_eq!(payloads, vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]);
    }

    #[test]
    fn scrubbed_appends_never_persist_the_raw_secret() {
        let dir = tempfile::tempdir().unwrap();
        let journal = GroupCommitJournal::spawn(dir.path(), 8).unwrap();
        journal
            .append_scrubbed("deploy with ghp_16C7e42F292c6912E7710c838347Ae178B4a now")
            .unwrap();
        drop(journal);
        let text: Vec<u8> = replay(dir.path()).unwrap().into_iter().flat_map(|f| f.payload).collect();
        let text = String::from_utf8(text).unwrap();
        assert!(!text.contains("16C7e42F"), "raw secret reached the journal: {text}");
        assert!(text.contains("[REDACTED_SECRET:b3_"));
    }

    #[test]
    fn oversized_payload_is_rejected_before_queueing() {
        let dir = tempfile::tempdir().unwrap();
        let journal = GroupCommitJournal::spawn(dir.path(), 8).unwrap();
        assert!(matches!(
            journal.append(vec![0u8; MAX_PAYLOAD_LEN + 1]),
            Err(EonError::ValidationError(_))
        ));
        assert_eq!(journal.append_durable(b"ok".to_vec()).unwrap(), 1, "queue must still work");
    }
}
