//! One bidi stream per utterance. A fresh 16-byte nonce, then frames:
//! kind:u8, count:u8, count * BE u16 Zipformer IDs. Only the final has FIN.
//! Each reply is BLAKE3(nonce || all frames so far), BE u16 caption length,
//! BE u16 pronunciation length, UTF-8 snapshot, then Kokoro u8 IDs (final
//! only). The digest binds order, boundaries and finality, but is NOT proof
//! of correct text or pronunciation: the edge checks both independently.

use eon_primitives::{EonResult, id::generate_ulid_u128};
use iroh::EndpointAddr;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::time::{Instant, timeout, timeout_at};

use super::super::MAX_TEXT;
use super::{
    ALPN, MAX_DELTA, MAX_STEPS, RPC_BUDGET, Source, Step, TokenPeer, Vocabulary, channel_error, invalid, prepare,
    valid_text,
};
use crate::split_inference::REMOTE_BUDGET;

const MORE: u8 = 1;
const FINAL: u8 = 2;
const MAX_PHONEMES: usize = 509;

pub(super) async fn serve(send: &mut SendStream, recv: &mut RecvStream, vocab: &Vocabulary) -> EonResult<()> {
    let mut nonce = [0; 16];
    recv.read_exact(&mut nonce).await.map_err(channel_error)?;
    let mut hash = blake3::Hasher::new();
    hash.update(&nonce);
    let mut pieces = String::new();
    for _ in 0..MAX_STEPS {
        let mut header = [0; 2];
        recv.read_exact(&mut header).await.map_err(channel_error)?;
        let [kind, count] = header;
        if !matches!(kind, MORE | FINAL) || count as usize > MAX_DELTA || (kind == MORE && count == 0) {
            return Err(invalid());
        }
        let mut bytes = vec![0; usize::from(count) * 2];
        recv.read_exact(&mut bytes).await.map_err(channel_error)?;
        hash.update(&header).update(&bytes);
        for &pair in bytes.as_chunks::<2>().0 {
            let id = u16::from_be_bytes(pair);
            let piece = vocab.0.get(&id).filter(|_| id != 0).ok_or_else(invalid)?;
            if pieces.len() + piece.len() > MAX_TEXT + 1 {
                return Err(invalid());
            }
            pieces.push_str(piece);
        }
        let text = pieces.trim();
        valid_text(text)?;
        let pronunciation = if kind == FINAL {
            if recv.read(&mut [0]).await.map_err(channel_error)?.is_some() {
                return Err(invalid());
            }
            let text = text.to_owned();
            tokio::task::spawn_blocking(move || prepare(&text))
                .await
                .map_err(channel_error)??
        } else {
            None
        };
        let phonemes = pronunciation.as_ref().map_or(&[][..], |p| p.ids());
        let mut out = Vec::with_capacity(36 + text.len() + phonemes.len());
        out.extend_from_slice(hash.finalize().as_bytes());
        out.extend_from_slice(&(text.len() as u16).to_be_bytes());
        out.extend_from_slice(&(phonemes.len() as u16).to_be_bytes());
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(phonemes);
        send.write_all(&out).await.map_err(channel_error)?;
        if kind == FINAL {
            return send.finish().map_err(channel_error);
        }
    }
    Err(invalid())
}

fn lost<T>(_: T) -> Source {
    Source::LocalOnChannelError
}

pub(super) struct Link {
    conn: Connection,
    send: SendStream,
    recv: RecvStream,
    hash: blake3::Hasher,
    deadline: Instant,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"done");
    }
}

impl Link {
    pub(super) async fn connect(edge: &TokenPeer, worker: EndpointAddr) -> Result<Self, Source> {
        timeout(REMOTE_BUDGET, async {
            let conn = edge.0.endpoint.connect(worker, ALPN).await.map_err(lost)?;
            let (mut send, recv) = conn.open_bi().await.map_err(lost)?;
            let nonce = generate_ulid_u128().to_be_bytes();
            send.write_all(&nonce).await.map_err(lost)?;
            let mut hash = blake3::Hasher::new();
            hash.update(&nonce);
            Ok(Self {
                conn,
                send,
                recv,
                hash,
                deadline: Instant::now() + RPC_BUDGET,
            })
        })
        .await
        .map_err(|_| Source::LocalOnTimeout)?
    }

    pub(super) async fn exchange(&mut self, step: &Step) -> Result<Vec<u8>, Source> {
        timeout_at(self.deadline.min(Instant::now() + REMOTE_BUDGET), async {
            let mut frame = vec![if step.final_step { FINAL } else { MORE }, step.ids.len() as u8];
            for id in &step.ids {
                frame.extend_from_slice(&id.to_be_bytes());
            }
            self.hash.update(&frame);
            self.send.write_all(&frame).await.map_err(lost)?;
            if step.final_step {
                self.send.finish().map_err(lost)?;
            }
            let mut header = [0; 36];
            self.recv.read_exact(&mut header).await.map_err(lost)?;
            let text_len = u16::from_be_bytes([header[32], header[33]]) as usize;
            let phones_len = u16::from_be_bytes([header[34], header[35]]) as usize;
            if &header[..32] != self.hash.finalize().as_bytes()
                || text_len != step.text.len()
                || phones_len > MAX_PHONEMES
                || (step.prepared.is_none() && phones_len != 0)
            {
                return Err(Source::LocalOnInvalidReply);
            }
            let mut text = vec![0; text_len];
            self.recv.read_exact(&mut text).await.map_err(lost)?;
            if text != step.text.as_bytes() {
                return Err(Source::LocalOnInvalidReply);
            }
            let mut ids = vec![0; phones_len];
            self.recv.read_exact(&mut ids).await.map_err(lost)?;
            if step.final_step && self.recv.read(&mut [0]).await.map_err(lost)?.is_some() {
                return Err(Source::LocalOnInvalidReply);
            }
            Ok(ids)
        })
        .await
        .map_err(|_| Source::LocalOnTimeout)?
    }
}
