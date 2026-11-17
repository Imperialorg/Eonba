//! Replicated projection state over Loro CRDTs (docs/implementation/05, section 3).
//!
//! Each device holds a replica of the shared projection. Replicas accept local edits offline, exchange update
//! bytes, and converge without coordination. Imports are idempotent, so a duplicate or replayed update is harmless.
//! The transport is out of scope here. Only the sync semantics are implemented and tested.

use loro::{ExportMode, LoroDoc, LoroValue};

use eon_primitives::{EonError, EonResult};

const PROJECTION: &str = "projection";

pub struct Replica {
    doc: LoroDoc,
}

impl Replica {
    pub fn new(peer_id: u64) -> EonResult<Self> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(loro_err)?;
        Ok(Self { doc })
    }

    /// Writes a key in the shared projection and commits it locally.
    pub fn set(&self, key: &str, value: &str) -> EonResult<()> {
        self.doc
            .get_map(PROJECTION)
            .insert(key, value)
            .map_err(loro_err)?;
        self.doc.commit();
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match self.doc.get_map(PROJECTION).get(key)? {
            loro::ValueOrContainer::Value(LoroValue::String(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Everything this replica knows, as one update blob. Pass it to another replica's `apply`.
    pub fn export_all(&self) -> EonResult<Vec<u8>> {
        self.doc.export(ExportMode::all_updates()).map_err(loro_err)
    }

    /// Applies an update blob. Applying the same blob twice has no further effect.
    pub fn apply(&self, update: &[u8]) -> EonResult<()> {
        self.doc.import(update).map_err(loro_err)?;
        Ok(())
    }
}

fn loro_err(e: impl std::fmt::Display) -> EonError {
    EonError::InternalError(format!("loro: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_way_sync_delivers_writes() {
        let a = Replica::new(1).unwrap();
        let b = Replica::new(2).unwrap();
        a.set("device", "phone").unwrap();
        b.apply(&a.export_all().unwrap()).unwrap();
        assert_eq!(b.get("device").as_deref(), Some("phone"));
    }

    #[test]
    fn concurrent_offline_edits_converge_on_both_replicas() {
        let a = Replica::new(1).unwrap();
        let b = Replica::new(2).unwrap();
        a.set("owner_note", "from laptop").unwrap();
        b.set("owner_note", "from phone").unwrap();
        b.set("battery", "80").unwrap();

        a.apply(&b.export_all().unwrap()).unwrap();
        b.apply(&a.export_all().unwrap()).unwrap();

        assert_eq!(a.get("owner_note"), b.get("owner_note"), "replicas must agree on the conflicting key");
        assert!(a.get("owner_note").is_some());
        assert_eq!(a.get("battery").as_deref(), Some("80"));
        assert_eq!(b.get("battery").as_deref(), Some("80"));
    }

    #[test]
    fn replayed_updates_are_idempotent() {
        let a = Replica::new(1).unwrap();
        let b = Replica::new(2).unwrap();
        a.set("k", "v").unwrap();
        let update = a.export_all().unwrap();
        b.apply(&update).unwrap();
        let once = b.export_all().unwrap();
        b.apply(&update).unwrap();
        b.apply(&update).unwrap();
        assert_eq!(b.get("k").as_deref(), Some("v"));
        assert_eq!(b.export_all().unwrap(), once);
    }

    #[test]
    fn corrupt_update_is_refused() {
        let b = Replica::new(2).unwrap();
        assert!(b.apply(b"not a loro update").is_err());
    }

    #[test]
    fn three_replicas_converge_in_any_order() {
        let replicas: Vec<Replica> = (1..=3).map(|i| Replica::new(i).unwrap()).collect();
        replicas[0].set("a", "1").unwrap();
        replicas[1].set("b", "2").unwrap();
        replicas[2].set("c", "3").unwrap();
        let updates: Vec<Vec<u8>> = replicas.iter().map(|r| r.export_all().unwrap()).collect();
        for order in [[0usize, 1, 2], [2, 0, 1], [1, 2, 0]] {
            let fresh: Vec<Replica> = (10..13).map(|i| Replica::new(i).unwrap()).collect();
            for &i in &order {
                for r in &fresh {
                    r.apply(&updates[i]).unwrap();
                }
            }
            for r in &fresh {
                assert_eq!(r.get("a").as_deref(), Some("1"));
                assert_eq!(r.get("b").as_deref(), Some("2"));
                assert_eq!(r.get("c").as_deref(), Some("3"));
            }
        }
    }
}
