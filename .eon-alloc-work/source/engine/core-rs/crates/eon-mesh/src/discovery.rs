//! Peer discovery by signed address announcements (docs/implementation/05, section 4).
//!
//! A peer signs its own address with its endpoint key. Anyone holding the announcement can check it against the peer's
//! public key, so a relay or another peer can carry it without being trusted. A directory keeps the newest valid
//! announcement per peer and refuses forged, tampered, or older ones.

use std::collections::HashMap;

use eon_primitives::{EonError, EonResult};
use iroh::{EndpointAddr, EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

fn discovery_err(reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "mesh.discovery".into(),
        reason: reason.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announcement {
    pub id: EndpointId,
    pub addr: EndpointAddr,
    pub seq: u64,
    pub signature: Signature,
}

fn signed_bytes(id: &EndpointId, addr: &EndpointAddr, seq: u64) -> EonResult<Vec<u8>> {
    postcard::to_allocvec(&(id, addr, seq)).map_err(discovery_err)
}

/// Signs `addr` as the address of the peer that holds `secret`, at sequence number `seq`.
pub fn sign(secret: &SecretKey, addr: EndpointAddr, seq: u64) -> EonResult<Announcement> {
    let id = secret.public();
    let bytes = signed_bytes(&id, &addr, seq)?;
    Ok(Announcement {
        id,
        addr,
        seq,
        signature: secret.sign(&bytes),
    })
}

impl Announcement {
    /// Checks the signature against the announced peer key, and that the address belongs to that key.
    pub fn verify(&self) -> EonResult<()> {
        if self.addr.id != self.id {
            return Err(discovery_err("the announced address belongs to another peer"));
        }
        let bytes = signed_bytes(&self.id, &self.addr, self.seq)?;
        self.id
            .verify(&bytes, &self.signature)
            .map_err(|_| discovery_err("the announcement signature does not verify"))
    }

    pub fn encode(&self) -> EonResult<Vec<u8>> {
        postcard::to_allocvec(self).map_err(discovery_err)
    }

    pub fn decode(bytes: &[u8]) -> EonResult<Self> {
        postcard::from_bytes(bytes).map_err(discovery_err)
    }
}

/// The newest verified announcement held for each peer.
#[derive(Default)]
pub struct Directory {
    newest: HashMap<EndpointId, Announcement>,
}

impl Directory {
    /// Stores the announcement when it verifies and is newer than the one held. Returns whether it was stored.
    pub fn accept(&mut self, announcement: Announcement) -> EonResult<bool> {
        announcement.verify()?;
        if self
            .newest
            .get(&announcement.id)
            .is_some_and(|held| held.seq >= announcement.seq)
        {
            return Ok(false);
        }
        self.newest.insert(announcement.id, announcement);
        Ok(true)
    }

    pub fn lookup(&self, id: &EndpointId) -> Option<&EndpointAddr> {
        self.newest.get(id).map(|announcement| &announcement.addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::TransportAddr;

    fn addr_on(secret: &SecretKey, port: u16) -> EndpointAddr {
        let socket = format!("127.0.0.1:{port}").parse().unwrap();
        EndpointAddr::from_parts(secret.public(), [TransportAddr::Ip(socket)])
    }

    #[test]
    fn a_signed_announcement_verifies_and_survives_encoding() {
        let secret = SecretKey::from_bytes(&[1; 32]);
        let announcement = sign(&secret, addr_on(&secret, 4000), 1).unwrap();
        let decoded = Announcement::decode(&announcement.encode().unwrap()).unwrap();
        decoded.verify().unwrap();
    }

    #[test]
    fn a_changed_sequence_number_breaks_the_signature() {
        let secret = SecretKey::from_bytes(&[1; 32]);
        let mut announcement = sign(&secret, addr_on(&secret, 4000), 1).unwrap();
        announcement.seq = 9;
        assert!(announcement.verify().is_err());
    }

    #[test]
    fn an_announcement_claiming_another_peer_key_is_refused() {
        let victim = SecretKey::from_bytes(&[1; 32]);
        let attacker = SecretKey::from_bytes(&[2; 32]);
        let mut forged = sign(&attacker, addr_on(&attacker, 4000), 1).unwrap();
        forged.id = victim.public();
        forged.addr = addr_on(&victim, 4000);
        assert!(forged.verify().is_err());
    }

    #[test]
    fn an_address_belonging_to_another_key_is_refused_even_with_a_valid_signature() {
        let signer = SecretKey::from_bytes(&[1; 32]);
        let other = SecretKey::from_bytes(&[2; 32]);
        let announcement = sign(&signer, addr_on(&other, 4000), 1).unwrap();
        assert!(announcement.verify().is_err());
    }

    #[test]
    fn the_directory_keeps_only_the_newest_valid_announcement() {
        let secret = SecretKey::from_bytes(&[3; 32]);
        let mut directory = Directory::default();
        assert!(directory.accept(sign(&secret, addr_on(&secret, 5001), 2).unwrap()).unwrap());
        assert!(!directory.accept(sign(&secret, addr_on(&secret, 5000), 1).unwrap()).unwrap());
        assert!(!directory.accept(sign(&secret, addr_on(&secret, 5001), 2).unwrap()).unwrap());
        assert_eq!(directory.lookup(&secret.public()), Some(&addr_on(&secret, 5001)));
    }

    #[test]
    fn the_directory_refuses_an_invalid_announcement_without_storing_it() {
        let secret = SecretKey::from_bytes(&[4; 32]);
        let mut announcement = sign(&secret, addr_on(&secret, 5002), 1).unwrap();
        announcement.seq = 7;
        let mut directory = Directory::default();
        assert!(directory.accept(announcement).is_err());
        assert!(directory.lookup(&secret.public()).is_none());
    }
}
