//! One-round state exchange between two replicas over the transport (docs/implementation/05, section 3).
//!
//! The initiator offers its version vector and its full state. The responder applies that state and answers with the
//! updates the initiator lacks, and the initiator applies the answer. Both sides converge after one round trip. Repeating
//! a round is harmless because imports are idempotent.

use eon_primitives::{EonError, EonResult};
use iroh::EndpointAddr;

use crate::sync::Replica;
use crate::transport::PeerNode;

fn exchange_err(reason: &str) -> EonError {
    EonError::OperationFailed {
        operation: "mesh.exchange".into(),
        reason: reason.into(),
    }
}

/// The initiator's message: its version vector, length-prefixed, followed by its full state.
pub fn offer(replica: &Replica) -> EonResult<Vec<u8>> {
    let version = replica.version();
    let mut out = (version.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&version);
    out.extend_from_slice(&replica.export_all()?);
    Ok(out)
}

/// The responder's side: applies the initiator's state and returns the updates the initiator lacks.
pub fn answer(replica: &Replica, offer: &[u8]) -> EonResult<Vec<u8>> {
    let (version, update) = split(offer)?;
    replica.apply(update)?;
    replica.export_since(version)
}

fn split(bytes: &[u8]) -> EonResult<(&[u8], &[u8])> {
    let header: [u8; 4] = bytes
        .get(..4)
        .ok_or_else(|| exchange_err("offer is shorter than its header"))?
        .try_into()
        .map_err(|_| exchange_err("offer header is malformed"))?;
    let len = u32::from_le_bytes(header) as usize;
    let version = bytes
        .get(4..4 + len)
        .ok_or_else(|| exchange_err("offer version is truncated"))?;
    Ok((version, &bytes[4 + len..]))
}

/// Offers `replica` to `peer` and applies the answer. Afterwards `replica` holds everything `peer` held.
pub async fn exchange(client: &PeerNode, peer: EndpointAddr, replica: &Replica) -> EonResult<()> {
    let reply = client.request(peer, &offer(replica)?).await?;
    replica.apply(&reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_truncated_offer_is_refused() {
        let replica = Replica::new(1).unwrap();
        assert!(answer(&replica, &[9, 0, 0, 0, 1]).is_err());
        assert!(answer(&replica, &[1]).is_err());
    }

    #[test]
    fn offer_and_answer_converge_two_replicas_in_memory() {
        let a = Replica::new(1).unwrap();
        let b = Replica::new(2).unwrap();
        a.set("from_a", "1").unwrap();
        b.set("from_b", "2").unwrap();
        let reply = answer(&b, &offer(&a).unwrap()).unwrap();
        a.apply(&reply).unwrap();
        assert_eq!(a.get("from_b").as_deref(), Some("2"));
        assert_eq!(b.get("from_a").as_deref(), Some("1"));
    }
}
