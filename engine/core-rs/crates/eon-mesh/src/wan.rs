//! Opt-in relay-only QUIC for two operator-provisioned identities. No address
//! publication, peer lookup, direct sockets, hole punching or default relay.
//! While open, iroh maintains its connection to the configured relay. The
//! relay sees endpoint IDs, sizes and timing, not the end-to-end QUIC payload.

#[cfg(test)]
mod tests;
mod wire;

use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use eon_primitives::{EonError, EonResult};
use iroh::endpoint::{
    Connection, IncomingAddr, NetReportConfig, PortmapperConfig, QuicTransportConfig,
    presets::Minimal,
};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMode, RelayUrl, SecretKey};
use prost::Message;
use tokio::time::{Instant, sleep_until, timeout};

use crate::qos::{BulkPacer, TrafficClass};
pub use wire::{AudioPcmChunk, CrdtDeltaSync, EpochInvalidationEvent, ReliableFrame};

const ALPN: &[u8] = b"eon/mesh/wan-qos/1";
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);
const RECEIVED: u8 = 1;
const MAX_AUDIO: usize = 1150;

fn transport_error(error: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "mesh.wan".into(),
        reason: error.to_string(),
    }
}

fn invalid(message: &str) -> EonError {
    EonError::ValidationError(message.into())
}

fn validate_relay(relay: &RelayUrl) -> EonResult<()> {
    let host = relay
        .host_str()
        .ok_or_else(|| invalid("relay must have a host"))?;
    let ip = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok();
    if !(relay.scheme() == "https"
        || (relay.scheme() == "http" && ip.is_some_and(|ip| ip.is_loopback())))
        || ip.is_some_and(|ip| {
            ip.is_unspecified() || ip.is_multicast() || ip == std::net::Ipv4Addr::BROADCAST
        })
        || !relay.username().is_empty()
        || relay.password().is_some()
        || relay.path() != "/"
        || relay.port() == Some(0)
        || relay.query().is_some()
        || relay.fragment().is_some()
    {
        return Err(invalid(
            "expected a trusted HTTPS relay origin (HTTP only on literal loopback)",
        ));
    }
    Ok(())
}

/// One application connection to exactly one peer via exactly one relay.
/// Neither peer announcements nor per-request addresses enter the dial path.
pub struct WanEndpoint {
    endpoint: Endpoint,
    conn: Connection,
    relay: RelayUrl,
    budget: Budget,
}

impl WanEndpoint {
    /// Both peers explicitly call `open` with overlapping `wait` windows.
    /// There is no relay presence before this call; `close` ends the session.
    /// Both operators must provision the same relay and reciprocal keys.
    /// `bytes_per_sec` is the operator's outgoing bandwidth budget, not a probe.
    pub async fn open(
        secret: SecretKey,
        pinned: EndpointId,
        relay: RelayUrl,
        bytes_per_sec: NonZeroU32,
        wait: Duration,
    ) -> EonResult<Self> {
        validate_relay(&relay)?;
        if secret.public() == pinned {
            return Err(EonError::SecurityViolation(
                "WAN peer must have a distinct key".into(),
            ));
        }
        let config = RelayConfig::new(relay.clone(), None); // no QUIC address discovery
        let transport = QuicTransportConfig::builder()
            .max_concurrent_uni_streams(0u8.into())
            .max_concurrent_bidi_streams(8u8.into())
            .build();
        timeout(wait, async {
            let ep = Endpoint::builder(Minimal)
                .clear_ip_transports()
                .clear_address_lookup()
                .portmapper_config(PortmapperConfig::Disabled)
                .net_report_config(NetReportConfig::minimal())
                .relay_mode(RelayMode::Custom(config.into()))
                .transport_config(transport)
                .secret_key(secret)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
                .map_err(transport_error)?;
            let conn = rendezvous(&ep, pinned, &relay).await?;
            Ok(Self {
                endpoint: ep,
                conn,
                relay,
                budget: Budget::new(bytes_per_sec),
            })
        })
        .await
        .map_err(|_| transport_error("timed out opening the provisioned relay path"))?
    }

    /// Identity authenticated by the current QUIC handshake.
    pub fn peer_id(&self) -> EndpointId {
        self.conn.remote_id()
    }

    /// Abandon a stalled session and rendezvous once more on the *same* relay.
    /// The pinned peer must explicitly `open` or `reconnect` during `wait`.
    /// This is a new QUIC connection, not migration of streams: outstanding
    /// frames are never replayed, and a missing receipt means delivery is unknown.
    /// The bandwidth budget (including existing bulk reservations) is retained.
    ///
    /// Timeout, error or cancellation ends the session; there is no background retry.
    pub async fn reconnect(mut self, wait: Duration) -> EonResult<Self> {
        let pinned = self.peer_id();
        self.conn.close(0u8.into(), b"reconnecting");
        self.conn = timeout(wait, rendezvous(&self.endpoint, pinned, &self.relay))
            .await
            .map_err(|_| {
                transport_error("timed out reconnecting to the provisioned relay peer")
            })??;
        Ok(self)
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

async fn rendezvous(ep: &Endpoint, pinned: EndpointId, relay: &RelayUrl) -> EonResult<Connection> {
    // iroh 1.4 chooses a listening "home relay" from probe results. Both
    // peers instead register by dialing. Choose the same connection by key order.
    let peer = EndpointAddr::new(pinned).with_relay_url(relay.clone());
    let dial = async { ep.connect(peer, ALPN).await.map_err(transport_error) };
    let (outgoing, incoming) = tokio::try_join!(dial, accept(ep, pinned, relay))?;
    if outgoing.remote_id() != pinned || incoming.remote_id() != pinned {
        return Err(EonError::SecurityViolation("unpaired WAN identity".into()));
    }
    Ok(if ep.id() < pinned { outgoing } else { incoming })
}

async fn accept(ep: &Endpoint, pinned: EndpointId, relay: &RelayUrl) -> EonResult<Connection> {
    loop {
        let incoming = ep
            .accept()
            .await
            .ok_or_else(|| transport_error("endpoint closed"))?;
        if matches!(incoming.remote_addr(), IncomingAddr::Relay { url, endpoint_id }
            if &url == relay && endpoint_id == pinned)
        {
            // The caller also checks the TLS identity, not just relay metadata.
            return incoming.await.map_err(transport_error);
        }
        incoming.refuse();
    }
}

struct Budget {
    start: Instant,
    pacer: Mutex<BulkPacer>,
    // At most one waiter checks the clock. Only reserve once admitted, so a
    // cancelled waiter leaves no debt and observes the current busy window.
    bulk: tokio::sync::Mutex<()>,
}

impl Budget {
    fn new(rate: NonZeroU32) -> Self {
        Self {
            start: Instant::now(),
            pacer: Mutex::new(BulkPacer::new(f64::from(rate.get()))),
            bulk: tokio::sync::Mutex::new(()),
        }
    }

    fn observe(&self, class: TrafficClass) {
        self.pacer
            .lock()
            .unwrap()
            .observe(class, self.start.elapsed());
    }

    async fn pace(&self, size: usize) {
        let _turn = self.bulk.lock().await;
        let at = self.pacer.lock().unwrap().next_bulk_at();
        sleep_until(self.start + at).await;
        self.pacer.lock().unwrap().admit(self.start.elapsed(), size);
    }
}

impl WanEndpoint {
    pub fn send_audio(&self, pcm: &AudioPcmChunk) -> EonResult<()> {
        pcm.validate()?;
        if pcm.encoded_len() > self.conn.max_datagram_size().unwrap_or(0) {
            return Err(invalid("audio exceeds the path datagram size"));
        }

        self.conn
            .send_datagram(Bytes::from(pcm.encode_to_vec()))
            .map_err(transport_error)?;
        self.budget.observe(TrafficClass::Audio);
        Ok(())
    }

    pub async fn recv_audio(&self) -> EonResult<AudioPcmChunk> {
        let bytes = self.conn.read_datagram().await.map_err(transport_error)?;
        if bytes.len() > MAX_AUDIO {
            return Err(invalid("oversized audio datagram"));
        }
        let pcm = AudioPcmChunk::decode(bytes).map_err(transport_error)?;
        pcm.validate()?;
        self.budget.observe(TrafficClass::Audio);
        Ok(pcm)
    }

    /// Finishes when the peer has decoded the frame, not when it has persisted
    /// or applied it. One stream per frame, allowing independent class priorities.
    pub async fn send(&self, frame: &ReliableFrame) -> EonResult<()> {
        let (lane, len) = frame.checked_size()?;
        timeout(STREAM_TIMEOUT, async {
            if lane == wire::Lane::Bulk {
                self.budget.pace(len + wire::HEADER).await;
            } else {
                self.budget.observe(lane.class());
            }
            let (mut send, mut recv) = self.conn.open_bi().await.map_err(transport_error)?;
            send.set_priority(lane.priority())
                .map_err(transport_error)?;
            send.write_all(&frame.encode(lane, len))
                .await
                .map_err(transport_error)?;
            send.finish().map_err(transport_error)?;
            if recv.read_to_end(1).await.map_err(transport_error)? != [RECEIVED] {
                return Err(invalid("invalid WAN receipt"));
            }
            Ok(())
        })
        .await
        .map_err(transport_error)?
    }

    pub async fn recv(&self) -> EonResult<ReliableFrame> {
        let (mut send, mut recv) = self.conn.accept_bi().await.map_err(transport_error)?;
        timeout(STREAM_TIMEOUT, async {
            let mut header = [0; wire::HEADER];
            recv.read_exact(&mut header)
                .await
                .map_err(transport_error)?;
            let (lane, len) = wire::parse_header(header)?;
            let payload = recv.read_to_end(len).await.map_err(transport_error)?;
            if payload.len() != len {
                return Err(invalid("truncated WAN frame"));
            }
            let frame = lane.decode(&payload)?;
            self.budget.observe(lane.class());
            send.set_priority(lane.priority())
                .map_err(transport_error)?;
            send.write_all(&[RECEIVED]).await.map_err(transport_error)?;
            send.finish().map_err(transport_error)?;
            Ok(frame)
        })
        .await
        .map_err(transport_error)?
    }
}
