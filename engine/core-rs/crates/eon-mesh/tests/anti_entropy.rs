//! Five peers, each syncing with a seeded random subset of the others in every round (fan-out 2). Replicas must reach the
//! same state within a bounded number of rounds, with no peer ever talking to all the others.

use std::sync::Arc;

use eon_mesh::exchange::{answer, exchange, pick_neighbours};
use eon_mesh::sync::Replica;
use eon_mesh::transport::PeerNode;
use iroh::EndpointAddr;

const PEERS: usize = 5;
const FANOUT: usize = 2;
const MAX_ROUNDS: usize = 8;

async fn serve_forever(node: Arc<PeerNode>, replica: Arc<Replica>) {
    loop {
        let replica = Arc::clone(&replica);
        node.serve_one(move |offer: Vec<u8>| answer(&replica, &offer).expect("a neighbour's offer must apply"))
            .await
            .unwrap();
    }
}

fn holds_every_write(replica: &Replica) -> bool {
    (0..PEERS).all(|i| replica.get(&format!("peer{i}")).is_some())
}

#[tokio::test]
async fn fan_out_anti_entropy_converges_five_peers_within_eight_rounds() {
    let replicas: Vec<Arc<Replica>> = (1..=PEERS as u64).map(|id| Arc::new(Replica::new(id).unwrap())).collect();
    for (index, replica) in replicas.iter().enumerate() {
        replica.set(&format!("peer{index}"), "online").unwrap();
    }

    let mut nodes = Vec::new();
    for _ in 0..PEERS {
        nodes.push(Arc::new(PeerNode::start().await.unwrap()));
    }
    let addrs: Vec<EndpointAddr> = nodes.iter().map(|node| node.addr()).collect();
    let servers: Vec<_> = (0..PEERS)
        .map(|i| tokio::spawn(serve_forever(Arc::clone(&nodes[i]), Arc::clone(&replicas[i]))))
        .collect();

    let mut rounds = 0;
    while !replicas.iter().all(|replica| holds_every_write(replica)) {
        assert!(rounds < MAX_ROUNDS, "not converged after {MAX_ROUNDS} rounds");
        for me in 0..PEERS {
            let others: Vec<usize> = (0..PEERS).filter(|&j| j != me).collect();
            let seed = ((rounds as u64) << 32) | me as u64;
            for pick in pick_neighbours(others.len(), FANOUT, seed) {
                let peer = others[pick];
                exchange(&nodes[me], addrs[peer].clone(), &replicas[me]).await.unwrap();
            }
        }
        rounds += 1;
    }
    for server in servers {
        server.abort();
    }
    assert!(rounds >= 1);
}
