//! Mutually pinned `eon/mesh/paired/3`. V2 peers are refused at the handshake:
//! in v3 a Changes reply carries at most one causal page, and Applied carries
//! the receiver's actual protected version vector. Unpaired exchange is unchanged.

#[cfg(test)]
mod tests;

use eon_primitives::{EonError, EonResult};
use iroh::EndpointAddr;
use loro::VersionVector;
use serde::{Deserialize, Serialize};

use crate::qos::TrafficClass;
use crate::split_inference::SpeechScores;
use crate::sync::{Page, Replica};
use crate::transport::{MAX_FRAME_BYTES, PeerNode};

const MAX_SYNC_RPCS: usize = 1024;

#[derive(Deserialize, Serialize)]
pub(crate) enum Request<'a> {
    ChangesSince(&'a [u8]),
    Apply(&'a [u8]),
    Decode(#[serde(borrow)] SpeechScores<'a>),
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Reply<'a> {
    Changes { version: &'a [u8], delta: &'a [u8] },
    Applied(&'a [u8]),
    Tokens(&'a [u8]),
    ItemTooLarge,
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

fn invalid(reason: &str) -> EonError {
    EonError::ValidationError(format!("paired sync: {reason}"))
}

fn fits_frame(frame: &impl Serialize) -> EonResult<bool> {
    Ok(postcard::experimental::serialized_size(frame).map_err(wire_err)? <= MAX_FRAME_BYTES)
}

fn pack(frame: &impl Serialize) -> EonResult<Vec<u8>> {
    if !fits_frame(frame)? {
        return Err(invalid("frame too large"));
    }
    postcard::to_allocvec(frame).map_err(wire_err)
}

// Loro VVs are postcard maps. Reject trailing data, duplicate writers and
// non-positive counters rather than treating a malformed claim as progress.
fn frontier(bytes: &[u8]) -> EonResult<VersionVector> {
    let (writers, entries) = postcard::take_from_bytes::<usize>(bytes).map_err(wire_err)?;
    if writers > entries.len() / 2 {
        return Err(invalid("invalid frontier length"));
    }
    let vv: VersionVector = unpack(bytes)?;
    if vv.len() != writers || vv.values().any(|&end| end <= 0) {
        return Err(invalid("invalid frontier entries"));
    }
    Ok(vv)
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
    let bytes = pack(&req)?;
    node.request_with_class(peer, class, &bytes).await
}

/// Serves one RPC, not a background listener. The transport checks the TLS identity
/// before opening the stream. Failed handlers never send a success acknowledgement.
pub async fn serve_one(node: &PeerNode, replica: &Replica) -> EonResult<()> {
    require_pair(node)?;
    node.try_serve_one(|bytes| answer(replica, &bytes)).await
}

fn answer(replica: &Replica, bytes: &[u8]) -> EonResult<Vec<u8>> {
    match unpack(bytes)? {
        Request::ChangesSince(version) => {
            let from = frontier(version)?;
            let tip = replica.paired_version();
            let version = &pack(&tip)?;
            let page = replica.paired_page(&from, &tip, |delta| fits_frame(&Reply::Changes { version, delta }))?;
            match page {
                Page::Current => pack(&Reply::Changes { version, delta: &[] }),
                Page::Update { bytes, .. } => pack(&Reply::Changes { version, delta: &bytes }),
                Page::ItemTooLarge => pack(&Reply::ItemTooLarge),
            }
        }
        Request::Apply(update) => {
            replica.paired_apply_page(update, None)?;
            pack(&Reply::Applied(&pack(&replica.paired_version())?))
        }
        Request::Decode(scores) => pack(&Reply::Tokens(&scores.decode()?)),
    }
}

/// Pull then push, resuming from the two documents (no session cursor). Each
/// call uses at most 1024 RPCs; after interruption or budget exhaustion, retry.
/// New local records have a conservative [`crate::sync::MAX_PROTECTED_RECORD_BYTES`]
/// limit. A legacy operation exceeding 1 MiB raw, or not fitting its framed
/// Loro update, is reported rather than silently skipped.
pub async fn sync(node: &PeerNode, peer: EndpointAddr, replica: &Replica) -> EonResult<()> {
    sync_with_budget(node, peer, replica, MAX_SYNC_RPCS).await
}

async fn sync_with_budget(node: &PeerNode, peer: EndpointAddr, replica: &Replica, limit: usize) -> EonResult<()> {
    let mut budget = 0..limit;
    let mut next_rpc = || budget.next().ok_or_else(|| invalid("RPC limit reached; resume sync"));
    let oversized = || invalid("single protected operation too large for a page");
    let mut remote = VersionVector::default();
    loop {
        next_rpc()?;
        let local = replica.paired_version();
        let reply = request(node, peer.clone(), Request::ChangesSince(&pack(&local)?)).await?;
        let (version, delta) = match unpack(&reply)? {
            Reply::Changes { version, delta } => (frontier(version)?, delta),
            Reply::ItemTooLarge => return Err(oversized()),
            _ => return Err(invalid("expected changes")),
        };
        if !version.includes_vv(&remote) || delta.is_empty() != local.includes_vv(&version) {
            return Err(invalid("regressing frontier or missing/unexpected page"));
        }
        if !delta.is_empty() {
            replica.paired_apply_page(delta, Some(&version))?;
        }
        remote = version;
        if replica.paired_version().includes_vv(&remote) {
            break;
        }
    }

    replica.check_paired_frontier(&remote)?;
    let tip = replica.paired_version();
    while !remote.includes_vv(&tip) {
        next_rpc()?;
        let page = replica.paired_page(&remote, &tip, |delta| fits_frame(&Request::Apply(delta)))?;
        let (bytes, end) = match page {
            Page::Update { bytes, end } => (bytes, end),
            Page::ItemTooLarge => return Err(oversized()),
            Page::Current => return Err(invalid("no outgoing progress")),
        };
        let ack = request(node, peer.clone(), Request::Apply(&bytes)).await?;
        let Reply::Applied(version) = unpack(&ack)? else {
            return Err(invalid("expected acknowledgement"));
        };
        let verified = frontier(version)?;
        let mut expected = remote;
        expected.set_end(end);
        if verified != expected {
            return Err(invalid("acknowledgement does not match applied page"));
        }
        remote = verified;
    }
    Ok(())
}
