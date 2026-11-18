//! Three peers sync through a hub over the iroh loopback transport. Each peer writes its own key and a shared key offline.
//! Every peer then syncs once with the hub, in turn, and then once more. Each sync sends the peer's full state and gets back
//! the updates it lacks. After the second round, all four replicas must hold every key and agree on the shared one.

use std::sync::Arc;

use eon_mesh::sync::Replica;
use eon_mesh::transport::PeerNode;
use iroh::EndpointAddr;

fn frame(version: &[u8], update: &[u8]) -> Vec<u8> {
    let mut out = (version.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(version);
    out.extend_from_slice(update);
    out
}

fn unframe(bytes: &[u8]) -> (&[u8], &[u8]) {
    let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    (&bytes[4..4 + len], &bytes[4 + len..])
}

async fn sync_with_hub(client: &PeerNode, hub: EndpointAddr, replica: &Replica) {
    let request = frame(&replica.version(), &replica.export_all().unwrap());
    let reply = client.request(hub, &request).await.unwrap();
    replica.apply(&reply).unwrap();
}

#[tokio::test]
async fn three_peers_converge_through_a_hub_in_two_rounds() {
    let hub_replica = Arc::new(Replica::new(1).unwrap());
    let peers: Vec<Replica> = (2..=4).map(|id| Replica::new(id).unwrap()).collect();
    for (index, peer) in peers.iter().enumerate() {
        peer.set(&format!("own{index}"), "written offline").unwrap();
        peer.set("owner", &format!("from peer {index}")).unwrap();
    }

    let hub = PeerNode::start().await.unwrap();
    let hub_addr = hub.addr();
    let rounds = 2 * peers.len();
    let served = Arc::clone(&hub_replica);
    let hub_task = tokio::spawn(async move {
        for _ in 0..rounds {
            let served = Arc::clone(&served);
            hub.serve_one(move |request| {
                let (version, update) = unframe(&request);
                served.apply(update).expect("a peer's state must apply");
                served.export_since(version).expect("a peer's version must decode")
            })
            .await
            .unwrap();
        }
        hub.close().await;
    });

    let mut clients = Vec::new();
    for _ in &peers {
        clients.push(PeerNode::start().await.unwrap());
    }
    for _ in 0..2 {
        for (peer, client) in peers.iter().zip(&clients) {
            sync_with_hub(client, hub_addr.clone(), peer).await;
        }
    }
    hub_task.await.unwrap();
    for client in clients {
        client.close().await;
    }

    let replicas: Vec<&Replica> = std::iter::once(hub_replica.as_ref()).chain(peers.iter()).collect();
    let agreed = replicas[0].get("owner");
    assert!(agreed.is_some());
    for replica in &replicas {
        assert_eq!(replica.get("owner"), agreed, "the shared key must resolve identically everywhere");
        for index in 0..peers.len() {
            assert_eq!(replica.get(&format!("own{index}")).as_deref(), Some("written offline"));
        }
    }
}
