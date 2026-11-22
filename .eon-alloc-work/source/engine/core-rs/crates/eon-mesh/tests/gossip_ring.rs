//! Three peers in a ring. Each exchanges state only with its neighbour: A with B, then B with C, then C with A. No peer talks
//! to every other peer, yet after one pass every replica holds every write.

use std::sync::Arc;

use eon_mesh::exchange::{answer, exchange};
use eon_mesh::sync::Replica;
use eon_mesh::transport::PeerNode;
use iroh::EndpointAddr;

async fn serve_one_exchange(node: Arc<PeerNode>, replica: Arc<Replica>) {
    node.serve_one(move |offer: Vec<u8>| answer(&replica, &offer).expect("a neighbour's offer must apply"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_ring_of_three_peers_converges_after_one_pass() {
    let names = ["a", "b", "c"];
    let replicas: Vec<Arc<Replica>> = (1..=3).map(|id| Arc::new(Replica::new(id).unwrap())).collect();
    for (replica, name) in replicas.iter().zip(names) {
        replica.set(&format!("from_{name}"), name).unwrap();
    }

    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes.push(Arc::new(PeerNode::start().await.unwrap()));
    }
    let addrs: Vec<EndpointAddr> = nodes.iter().map(|node| node.addr()).collect();

    let servers: Vec<_> = (0..3)
        .map(|i| tokio::spawn(serve_one_exchange(Arc::clone(&nodes[i]), Arc::clone(&replicas[i]))))
        .collect();
    for (from, to) in [(0usize, 1usize), (1, 2), (2, 0)] {
        exchange(&nodes[from], addrs[to].clone(), &replicas[from]).await.unwrap();
    }
    for server in servers {
        server.await.unwrap();
    }

    for replica in &replicas {
        for name in names {
            assert_eq!(replica.get(&format!("from_{name}")).as_deref(), Some(name));
        }
    }
}
