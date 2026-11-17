//! Peer transport over iroh (docs/implementation/05, transport layer).
//!
//! Peers are dialed by public key over QUIC. Each request runs on its own bidirectional stream and is answered on the
//! same stream. Relays are disabled, so connections go directly between the endpoints and nothing leaves the host
//! unless a peer address points elsewhere. Protocol negotiation uses an ALPN string, so a peer that does not speak it
//! is refused during the handshake.

use eon_primitives::{EonError, EonResult};
use iroh::endpoint::{presets::Minimal, Endpoint};
use iroh::{EndpointAddr, RelayMode};

/// Application protocol identifier. Peers that do not offer it are refused during the handshake.
pub const ALPN: &[u8] = b"eon/mesh/1";

/// Largest request or reply accepted on one stream. Larger frames are refused rather than buffered.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

fn transport_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("mesh.{operation}"),
        reason: reason.to_string(),
    }
}

pub struct PeerNode {
    endpoint: Endpoint,
}

impl PeerNode {
    /// Binds an endpoint that accepts `alpn` connections and uses no relays.
    pub async fn start_with_alpn(alpn: &[u8]) -> EonResult<Self> {
        let endpoint = Endpoint::builder(Minimal)
            .alpns(vec![alpn.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .map_err(|e| transport_err("bind", e))?;
        Ok(Self { endpoint })
    }

    pub async fn start() -> EonResult<Self> {
        Self::start_with_alpn(ALPN).await
    }

    /// The address other peers use to dial this node.
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Sends `payload` to `peer` and returns the reply.
    pub async fn request(&self, peer: impl Into<EndpointAddr>, payload: &[u8]) -> EonResult<Vec<u8>> {
        let connection = self
            .endpoint
            .connect(peer, ALPN)
            .await
            .map_err(|e| transport_err("connect", e))?;
        let (mut send, mut recv) = connection.open_bi().await.map_err(|e| transport_err("open_stream", e))?;
        send.write_all(payload).await.map_err(|e| transport_err("send", e))?;
        send.finish().map_err(|e| transport_err("send", e))?;
        let reply = recv
            .read_to_end(MAX_FRAME_BYTES)
            .await
            .map_err(|e| transport_err("receive", e))?;
        connection.close(0u32.into(), b"done");
        Ok(reply)
    }

    /// Accepts one connection, reads one request, answers it with `handler`, and waits for the peer to close.
    pub async fn serve_one(&self, handler: impl FnOnce(Vec<u8>) -> Vec<u8>) -> EonResult<()> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| transport_err("accept", "endpoint closed"))?;
        let connection = incoming.await.map_err(|e| transport_err("handshake", e))?;
        let (mut send, mut recv) = connection.accept_bi().await.map_err(|e| transport_err("accept_stream", e))?;
        let request = recv
            .read_to_end(MAX_FRAME_BYTES)
            .await
            .map_err(|e| transport_err("receive", e))?;
        send.write_all(&handler(request)).await.map_err(|e| transport_err("send", e))?;
        send.finish().map_err(|e| transport_err("send", e))?;
        connection.closed().await;
        Ok(())
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}
