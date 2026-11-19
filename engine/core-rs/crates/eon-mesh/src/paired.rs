//! Replica and speech requests on the mutually pinned `eon/mesh/paired/2` service.

use eon_primitives::{EonError, EonResult};
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};

use crate::qos::TrafficClass;
use crate::split_inference::SpeechScores;
use crate::sync::Replica;
use crate::transport::PeerNode;

#[derive(Deserialize, Serialize)]
pub(crate) enum Request<'a> {
    ChangesSince(&'a [u8]),
    Apply(&'a [u8]),
    Decode(#[serde(borrow)] SpeechScores<'a>),
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Reply<'a> {
    Changes { version: &'a [u8], delta: &'a [u8] },
    Applied,
    Tokens(&'a [u8]),
}

pub(crate) fn unpack<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> EonResult<T> {
    let (value, rest) = postcard::take_from_bytes(bytes).map_err(wire_err)?;
    if !rest.is_empty() {
        return Err(EonError::ValidationError("trailing paired frame bytes".into()));
    }
    Ok(value)
}

fn wire_err(e: postcard::Error) -> EonError {
    EonError::ValidationError(format!("paired mesh frame: {e}"))
}

fn require_pair(node: &PeerNode) -> EonResult<()> {
    node.paired_id()
        .ok_or_else(|| EonError::SecurityViolation("expected a paired endpoint".into()))?;
    Ok(())
}

pub(crate) async fn request(node: &PeerNode, peer: EndpointAddr, req: Request<'_>) -> EonResult<Vec<u8>> {
    if node.paired_id() != Some(peer.id) {
        return Err(EonError::SecurityViolation(
            "expected the paired mesh destination".into(),
        ));
    }
    let class = match req {
        Request::Decode(_) => TrafficClass::Control,
        _ => TrafficClass::Delta,
    };
    let bytes = postcard::to_allocvec(&req).map_err(wire_err)?;
    node.request_with_class(peer, class, &bytes).await
}

/// Serves one RPC, not a background listener. The transport checks the TLS identity
/// before opening the stream. Failed handlers never send a success acknowledgement.
pub async fn serve_one(node: &PeerNode, replica: &Replica) -> EonResult<()> {
    require_pair(node)?;
    node.try_serve_one(|bytes| {
        match unpack(&bytes)? {
            Request::ChangesSince(version) => postcard::to_allocvec(&Reply::Changes {
                version: &replica.paired_version(),
                delta: &replica.paired_export_since(version)?,
            }),
            Request::Apply(update) => {
                replica.paired_apply(update)?;
                postcard::to_allocvec(&Reply::Applied)
            }
            Request::Decode(scores) => postcard::to_allocvec(&Reply::Tokens(&scores.decode()?)),
        }
        .map_err(wire_err)
    })
    .await
}

/// Pull the missing operations, then push just what the responder lacks. If either
/// RPC is interrupted a fresh round resumes from the actual frontiers on both ends.
pub async fn sync(node: &PeerNode, peer: EndpointAddr, replica: &Replica) -> EonResult<()> {
    let reply = request(node, peer.clone(), Request::ChangesSince(&replica.paired_version())).await?;
    let Reply::Changes { version, delta } = unpack(&reply)? else {
        return Err(EonError::ValidationError("expected paired mesh changes".into()));
    };
    let outgoing = replica.paired_export_since(version)?;
    replica.paired_apply(delta)?;
    let ack = request(node, peer, Request::Apply(&outgoing)).await?;
    if !matches!(unpack(&ack)?, Reply::Applied) {
        return Err(EonError::ValidationError("invalid paired sync acknowledgement".into()));
    }
    Ok(())
}
