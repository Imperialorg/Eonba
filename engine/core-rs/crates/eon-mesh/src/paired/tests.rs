use std::time::Duration;

use iroh::SecretKey;
use tokio::time::timeout;

use super::*;
use crate::sync::{Entity, MAX_PROTECTED_RECORD_BYTES};

async fn nodes() -> (PeerNode, PeerNode) {
    let a = SecretKey::generate();
    let b = SecretKey::generate();
    let remote = b.public();
    let desk = PeerNode::start_paired(b, a.public()).await.unwrap();
    (PeerNode::start_paired(a, remote).await.unwrap(), desk)
}

fn copy_missing(from: &Replica, to: &Replica) {
    let delta = from.paired_export_since(&to.paired_version().encode()).unwrap();
    to.paired_apply(&delta).unwrap();
}

fn offline_history(local: &Replica, other_id: u64) {
    let other = Replica::new(other_id).unwrap();
    let mut random = blake3::Hasher::new().update(&other_id.to_le_bytes()).finalize_xof();
    for (batch, writer) in [local, &other, local].into_iter().enumerate() {
        for i in 0..3 {
            let mut label = vec![0; MAX_PROTECTED_RECORD_BYTES - 128];
            random.fill(&mut label);
            label.iter_mut().for_each(|c| *c = b'!' + *c % 90);
            let entity = Entity {
                kind: "document".into(),
                label: String::from_utf8(label).unwrap(),
            };
            writer.put_entity(&format!("{other_id}-{batch}-{i}"), &entity).unwrap();
        }
        // Make this writer's next batch depend on the other writer, including
        // a tombstone. Writer order is not a valid order for this history.
        if batch == 0 {
            copy_missing(local, &other);
        } else if batch == 1 {
            other.remove_entity(&format!("{other_id}-0-0")).unwrap();
            copy_missing(&other, local);
        }
    }
    assert!(
        local
            .paired_export_since(&VersionVector::default().encode())
            .unwrap()
            .len()
            > MAX_FRAME_BYTES
    );
}

#[tokio::test]
async fn paired_pages_resume_large_mixed_writer_histories_and_replay_over_quic() {
    let (a, b) = (Replica::new(1).unwrap(), Replica::new(2).unwrap());
    offline_history(&a, 11);
    offline_history(&b, 22);
    a.set("only-public", "never paired").unwrap();
    let (a0, b0) = (a.paired_version(), b.paired_version());
    let mut merged = a0.clone();
    merged.merge(&b0);
    let (edge, desk) = nodes().await;

    timeout(Duration::from_secs(30), async {
        // Exhaust the RPC budget after one successful pull. A later call must
        // start at the actual partial import, not b's advertised tip.
        let (stopped, served) = tokio::join!(sync_with_budget(&edge, desk.addr(), &a, 1), serve_one(&desk, &b),);
        served.unwrap();
        assert!(stopped.unwrap_err().to_string().contains("RPC limit"));
        let partial_pull = a.paired_version();
        assert!(partial_pull > a0 && partial_pull < merged);
        assert_eq!(b.paired_version(), b0);

        // Resume, then interrupt the first push *after* the receiver imports it.
        // A forged ACK claiming all remaining pages must not complete the sync.
        let interrupt = async {
            let mut first = true;
            let mut pushed = None;
            while pushed.is_none() {
                desk.try_serve_one(|bytes| {
                    if first {
                        let Request::ChangesSince(v) = unpack(&bytes)? else {
                            panic!("expected pull")
                        };
                        assert_eq!(frontier(v)?, partial_pull);
                        first = false;
                    }
                    let reply = answer(&b, &bytes)?;
                    if matches!(unpack(&reply)?, Reply::Applied(_)) {
                        pushed = Some(bytes);
                        pack(&Reply::Applied(&merged.encode()))
                    } else {
                        Ok(reply)
                    }
                })
                .await
                .unwrap();
            }
            pushed.unwrap()
        };
        let (stopped, pushed) = tokio::join!(sync(&edge, desk.addr(), &a), interrupt);
        assert!(stopped.unwrap_err().to_string().contains("acknowledgement"));
        assert_eq!(a.paired_version(), merged);
        let partial_push = b.paired_version();
        assert!(partial_push > b0 && partial_push < merged);

        // Replay the exact captured Apply frame: no duplicate operation, and
        // the reply is the actual post-import version, even after an ACK loss.
        let (reply, ()) = tokio::try_join!(edge.request(desk.addr(), &pushed), serve_one(&desk, &b)).unwrap();
        let Reply::Applied(v) = unpack(&reply).unwrap() else {
            panic!("expected ACK")
        };
        assert_eq!(frontier(v).unwrap(), partial_push);
        assert_eq!(b.paired_version(), partial_push);

        tokio::select! {
            result = sync(&edge, desk.addr(), &a) => result.unwrap(),
            () = async { loop { serve_one(&desk, &b).await.unwrap(); } } => unreachable!(),
        }
        assert_eq!(b.paired_version(), merged);
        assert_eq!(a.knowledge_graph().unwrap(), b.knowledge_graph().unwrap());
        assert_eq!(b.get("only-public"), None);
        // A duplicate round has nothing left to send in either direction.
        tokio::try_join!(sync_with_budget(&edge, desk.addr(), &a, 1), serve_one(&desk, &b)).unwrap();
    })
    .await
    .unwrap();
    edge.close().await;
    desk.close().await;
}

#[tokio::test]
async fn paired_pages_reject_indivisible_oversized_record_in_either_direction() {
    let (edge, desk) = nodes().await;
    let small = Replica::new(1).unwrap();
    let large = Replica::new(2).unwrap();
    large.seed_legacy_entity(
        "k",
        &Entity {
            kind: "doc".into(),
            label: "x".repeat(MAX_FRAME_BYTES),
        },
    );
    for (a, b) in [(&small, &large), (&large, &small)] {
        let (result, server) = timeout(Duration::from_secs(8), async {
            tokio::join!(sync(&edge, desk.addr(), a), serve_one(&desk, b))
        })
        .await
        .unwrap();
        server.unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("single protected operation too large")
        );
        assert!(small.paired_version().is_empty());
    }
    edge.close().await;
    desk.close().await;
}

#[tokio::test]
async fn paired_pages_rejected_local_write_leaves_frontier_syncable() {
    let (a, b) = (Replica::new(1).unwrap(), Replica::new(2).unwrap());
    let key = "k".repeat(256);
    let keep = Entity {
        kind: "doc".into(),
        label: "keep".into(),
    };
    a.put_entity(&key, &keep).unwrap();
    let before = a.paired_version();
    // Neither field alone exceeds the limit, but their serialized sum does.
    let too_large = Entity {
        kind: "doc".into(),
        label: "v".repeat(MAX_PROTECTED_RECORD_BYTES - 128),
    };
    assert!(a.put_entity(&key, &too_large).is_err());
    assert_eq!(a.paired_version(), before);
    assert_eq!(a.knowledge_graph().unwrap().entities[&key], keep);
    a.put_entity("later", &keep).unwrap();

    // Admission is only for local writes. An older item above the new limit
    // must still import, and must remain exportable to another peer.
    let old = Replica::new(3).unwrap();
    let legacy = Entity {
        kind: "doc".into(),
        label: "v".repeat(MAX_PROTECTED_RECORD_BYTES + 1),
    };
    old.seed_legacy_entity("legacy", &legacy);
    let (edge, desk) = nodes().await;
    for (client, server) in [(&a, &old), (&a, &b)] {
        timeout(Duration::from_secs(8), async {
            tokio::select! {
                result = sync(&edge, desk.addr(), client) => result.unwrap(),
                () = async { loop { serve_one(&desk, server).await.unwrap(); } } => unreachable!(),
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(b.paired_version(), a.paired_version());
    let graph = b.knowledge_graph().unwrap();
    assert_eq!(graph.entities["legacy"], legacy);
    assert_eq!(graph.entities["later"], keep);
    edge.close().await;
    desk.close().await;
}

#[tokio::test]
async fn paired_pages_reject_missing_duplicate_or_out_of_frontier_pull() {
    let (edge, desk) = nodes().await;
    let local = Replica::new(1).unwrap();
    let private = Replica::new(2).unwrap();
    private
        .put_entity(
            "k",
            &Entity {
                kind: "doc".into(),
                label: "x".into(),
            },
        )
        .unwrap();
    let empty = VersionVector::default().encode();
    let update = private.paired_export_since(&empty).unwrap();
    let tip = private.paired_version().encode();
    let public = Replica::new(3).unwrap();
    public.set("k", "out of domain").unwrap();
    let mut wrong_tip = VersionVector::default();
    wrong_tip.insert(9, 1);
    let wrong = wrong_tip.encode();
    for (version, delta) in [
        (&tip[..], &[][..]),                                // claims a missing operation but sends no page
        (&wrong, &update),                                  // page belongs to a different writer than the tip
        (&public.version(), &public.export_all().unwrap()), // public history is never a paired page
    ] {
        let reply = pack(&Reply::Changes { version, delta }).unwrap();
        let (result, served) = timeout(Duration::from_secs(8), async {
            tokio::join!(sync(&edge, desk.addr(), &local), desk.serve_one(|_| reply))
        })
        .await
        .unwrap();
        served.unwrap();
        assert!(result.is_err());
        assert!(local.paired_version().is_empty());
    }
    copy_missing(&private, &local);
    wrong_tip.merge(&private.paired_version());
    let repeated = pack(&Reply::Changes {
        version: &wrong_tip.encode(),
        delta: &update,
    })
    .unwrap();
    let (result, served) = timeout(Duration::from_secs(8), async {
        tokio::join!(sync(&edge, desk.addr(), &local), desk.serve_one(|_| repeated))
    })
    .await
    .unwrap();
    served.unwrap();
    assert!(result.is_err());
    assert_eq!(local.paired_version(), private.paired_version());
    edge.close().await;
    desk.close().await;
}

#[test]
fn paired_pages_require_exact_frontier_framing() {
    assert!(frontier(&[1, 2]).is_err()); // missing counter
    assert!(frontier(&[0, 0]).is_err()); // trailing bytes
    assert!(frontier(&[2, 1, 2, 1, 4]).is_err()); // duplicate writer
    assert!(frontier(&[1, 1, 1]).is_err()); // negative (zig-zag) counter
}
