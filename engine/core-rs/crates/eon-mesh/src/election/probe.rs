//! One explicit control-stream round trip on an established paired election link.
//! The responder supplies no timestamps, commands or data. No election or playback state changes.

use super::PairedElection;
use crate::qos::TrafficClass;
use eon_primitives::{EonError, EonResult};
use iroh::endpoint::ConnectionError;
use std::time::{Duration, Instant};
use tokio::time::timeout;

const REQUEST: &[u8] = b"EON-RTT/1?";
const RESPONSE: &[u8] = b"EON-RTT/1!";
const BUDGET: Duration = Duration::from_secs(2);

fn probe_error(reason: &str) -> EonError {
    EonError::OperationFailed {
        operation: "mesh.paired_probe".into(),
        reason: reason.into(),
    }
}

fn unavailable(_: impl std::fmt::Display) -> EonError {
    probe_error("control stream unavailable")
}

impl PairedElection {
    /// Local time from opening a fresh bidirectional stream to receiving the full
    /// fixed reply and its FIN. Excludes mDNS, pairing and QUIC connection setup.
    pub async fn measure_control_rtt(&self) -> EonResult<Duration> {
        let start = Instant::now();
        timeout(BUDGET, async {
            let (mut send, mut receive) =
                self.link.connection.open_bi().await.map_err(unavailable)?;
            send.set_priority(TrafficClass::Control.stream_priority().unwrap())
                .map_err(unavailable)?;
            send.write_all(REQUEST).await.map_err(unavailable)?;
            send.finish().map_err(unavailable)?;
            if receive
                .read_to_end(RESPONSE.len())
                .await
                .map_err(unavailable)?
                != RESPONSE
            {
                return Err(EonError::ValidationError(
                    "invalid paired probe reply".into(),
                ));
            }
            Ok(start.elapsed())
        })
        .await
        .map_err(|_| probe_error("control probe request timed out"))?
    }

    /// Queue the fixed reply and FIN to one exact request. Keep the link open for
    /// the next request or the peer's close; FIN alone is not proof of delivery.
    pub async fn answer_control_probe(&self) -> EonResult<()> {
        timeout(BUDGET, async {
            let (mut send, mut receive) = self
                .link
                .connection
                .accept_bi()
                .await
                .map_err(unavailable)?;
            if receive
                .read_to_end(REQUEST.len())
                .await
                .map_err(unavailable)?
                != REQUEST
            {
                return Err(EonError::ValidationError(
                    "invalid paired probe request".into(),
                ));
            }
            send.set_priority(TrafficClass::Control.stream_priority().unwrap())
                .map_err(unavailable)?;
            send.write_all(RESPONSE).await.map_err(unavailable)?;
            send.finish().map_err(unavailable)?;
            Ok(())
        })
        .await
        .map_err(|_| probe_error("control probe responder timed out"))?
    }

    /// At the end of a probe session, the reader of the last reply closes first.
    /// Wait once here, not for a (potentially delayed) QUIC ACK after each response.
    pub async fn wait_for_control_peer_close(&self) -> EonResult<()> {
        match timeout(BUDGET, self.link.connection.closed()).await {
            Ok(ConnectionError::ApplicationClosed(close)) if close.error_code == 0u32.into() => {
                Ok(())
            }
            Ok(_) => Err(probe_error("unexpected control peer closure")),
            Err(_) => Err(probe_error("control peer close timed out")),
        }
    }
}
