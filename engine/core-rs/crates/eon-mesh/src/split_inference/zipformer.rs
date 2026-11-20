//! One endpointed utterance per authenticated QUIC stream, not live microphone
//! streaming. This ALPN fixes the model and the only request type: mono 16 kHz
//! PCM16LE. The reply is the input's BLAKE3 digest followed by final English text.
//! A matching digest binds a result to the audio; it does not prove a trusted
//! workstation computed the right words. No output is played or executed here.

mod model;
pub use model::PinnedZipformer;

use std::net::SocketAddr;
use std::time::Duration;

use eon_primitives::{EonError, EonResult};
use iroh::endpoint::{Connection, Endpoint, presets::Minimal};
use iroh::{EndpointAddr, EndpointId, RelayMode, SecretKey};
use tokio::time::timeout;

const ALPN: &[u8] = b"eon/asr/zipformer-en-2023-06-26/1";
pub const MAX_SAMPLES: usize = 8 * 16_000;
const MAX_PCM_BYTES: usize = 2 * MAX_SAMPLES;
const MAX_TEXT: usize = 2048;
const DIAL_BUDGET: Duration = Duration::from_secs(2);
const RPC_BUDGET: Duration = Duration::from_secs(8);

fn channel_error(_: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "mesh.asr".into(),
        reason: "speech channel unavailable".into(),
    }
}

fn not_paired() -> EonError {
    EonError::SecurityViolation("expected the pinned speech peer".into())
}

fn valid_text(text: &str) -> EonResult<()> {
    if text.len() > MAX_TEXT
        || text.trim() != text
        || !text
            .bytes()
            .all(|b| b.is_ascii_uppercase() || matches!(b, b' ' | b'\''))
    {
        return Err(EonError::ValidationError("invalid English ASR result".into()));
    }
    Ok(())
}

fn decode_reply<'a>(reply: &'a [u8], pcm: &[u8]) -> EonResult<&'a str> {
    let invalid = || EonError::ValidationError("invalid ASR reply".into());
    let (digest, text) = reply.split_at_checked(32).ok_or_else(invalid)?;
    if digest != blake3::hash(pcm).as_bytes() {
        return Err(invalid());
    }
    let text = std::str::from_utf8(text).map_err(|_| invalid())?;
    valid_text(text)?;
    Ok(text)
}

/// Dedicated service (no relays or discovery), mutually pinned with the same
/// out-of-band keys as the mesh. It cannot dispatch arbitrary mesh requests.
pub struct SpeechPeer {
    endpoint: Endpoint,
    paired: EndpointId,
}

impl SpeechPeer {
    pub async fn bind(bind: SocketAddr, key: SecretKey, paired: EndpointId) -> EonResult<Self> {
        if key.public() == paired {
            return Err(not_paired());
        }
        let endpoint = Endpoint::builder(Minimal)
            .secret_key(key)
            .alpns(vec![ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr(bind)
            .map_err(channel_error)?
            .bind()
            .await
            .map_err(channel_error)?;
        Ok(Self { endpoint, paired })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }

    /// Wait for one request. Once an incoming connection arrives, handshake,
    /// reading, inference and sending the reply share a single deadline.
    pub async fn serve_one(&mut self, model: Option<&PinnedZipformer>) -> EonResult<()> {
        let incoming = self.endpoint.accept().await.ok_or_else(|| channel_error("closed"))?;
        timeout(RPC_BUDGET, async {
            let conn = incoming.await.map_err(channel_error)?;
            if conn.remote_id() != self.paired {
                conn.close(403u32.into(), b"unpaired");
                return Err(not_paired());
            }
            let Some(model) = model else {
                conn.close(503u32.into(), b"unavailable");
                return Err(model::unavailable());
            };
            let (mut send, mut recv) = conn.accept_bi().await.map_err(channel_error)?;
            let pcm = recv.read_to_end(MAX_PCM_BYTES).await.map_err(channel_error)?;
            let digest = blake3::hash(&pcm);
            let text = model.infer(pcm).await?;
            send.write_all(digest.as_bytes()).await.map_err(channel_error)?;
            send.write_all(text.as_bytes()).await.map_err(channel_error)?;
            send.finish().map_err(channel_error)?;
            conn.closed().await;
            Ok(())
        })
        .await
        .map_err(channel_error)?
    }
}

/// The caller supplies one consent decision bound to the exact samples. Pairing
/// alone never opts a subsequent utterance in. `LocalOnly` does not need a peer.
pub enum Utterance<'a> {
    LocalOnly(&'a [i16]),
    AllowPairedWorker {
        pcm: &'a [i16],
        edge: &'a SpeechPeer,
        worker: EndpointAddr,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    PairedWorker,
    LocalOnly,
    LocalOnChannelError,
    LocalOnTimeout,
    LocalOnInvalidReply,
}

// Intentionally no Debug implementation containing transcript text.
pub struct Transcript {
    pub text: String,
    pub source: Source,
}

async fn request(conn: &Connection, pcm: &[u8]) -> EonResult<Vec<u8>> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(channel_error)?;
    send.write_all(pcm).await.map_err(channel_error)?;
    send.finish().map_err(channel_error)?;
    recv.read_to_end(32 + MAX_TEXT).await.map_err(channel_error)
}

async fn remote(edge: &SpeechPeer, worker: EndpointAddr, pcm: &[u8]) -> Result<String, Source> {
    let conn = timeout(DIAL_BUDGET, edge.endpoint.connect(worker, ALPN))
        .await
        .map_err(|_| Source::LocalOnTimeout)?
        .map_err(|_| Source::LocalOnChannelError)?;
    let reply = timeout(RPC_BUDGET, request(&conn, pcm)).await;
    conn.close(0u32.into(), b"done");
    let bytes = reply
        .map_err(|_| Source::LocalOnTimeout)?
        .map_err(|_| Source::LocalOnChannelError)?;
    decode_reply(&bytes, pcm)
        .map(str::to_owned)
        .map_err(|_| Source::LocalOnInvalidReply)
}

impl Utterance<'_> {
    /// No local inference on success. A channel failure or invalid reply runs
    /// the original PCM through the local model; no model means an explicit error.
    pub async fn transcribe(self, local: Option<&PinnedZipformer>) -> EonResult<Transcript> {
        let (samples, dest) = match self {
            Self::LocalOnly(samples) => (samples, None),
            Self::AllowPairedWorker { pcm, edge, worker } => {
                if edge.paired != worker.id {
                    return Err(not_paired());
                }
                (pcm, Some((edge, worker)))
            }
        };
        if samples.is_empty() || samples.len() > MAX_SAMPLES {
            return Err(EonError::ValidationError("expected up to 8 seconds of PCM16".into()));
        }
        let pcm: Vec<_> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let source = if let Some((edge, worker)) = dest {
            match remote(edge, worker, &pcm).await {
                Ok(text) => {
                    return Ok(Transcript {
                        text,
                        source: Source::PairedWorker,
                    });
                }
                Err(reason) => reason,
            }
        } else {
            Source::LocalOnly
        };
        let text = timeout(RPC_BUDGET, local.ok_or_else(model::unavailable)?.infer(pcm))
            .await
            .map_err(|_| model::unavailable())??;
        Ok(Transcript { text, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_inference_tests_zipformer_reply_binding_and_limits() {
        let pcm = [1, 0];
        let prefix = blake3::hash(&pcm);
        let reply = |text: &[u8]| [prefix.as_bytes().as_slice(), text].concat();
        assert!(decode_reply(&reply(b"HELLO"), &pcm).is_ok());
        assert!(decode_reply(&reply(b""), &pcm).unwrap().is_empty()); // silence
        assert!(decode_reply(&reply(b"HELLO"), &[0, 0]).is_err()); // stale reply
        for invalid in [b"raw\ntext".as_slice(), &[0xff], &vec![b'A'; MAX_TEXT + 1]] {
            assert!(decode_reply(&reply(invalid), &pcm).is_err());
        }
        assert!(decode_reply(&[], &pcm).is_err());
    }
}
