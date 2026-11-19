//! Two peers over the iroh loopback transport exchange their Loro replicas and converge.
//!
//! Each peer edits offline first. The client sends its full state to the server, the server applies it and replies
//! with its own full state, and the client applies the reply. Both replicas must then hold every key, and the keys both
//! peers wrote must resolve to the same value on both sides.

use std::sync::Arc;

use eon_mesh::sync::Replica;
use eon_mesh::transport::PeerNode;

#[tokio::test]
async fn two_peers_converge_after_one_round_trip_over_the_transport() {
    let client_replica = Arc::new(Replica::new(1).unwrap());
    let server_replica = Arc::new(Replica::new(2).unwrap());

    client_replica.set("client_only", "a").unwrap();
    server_replica.set("server_only", "b").unwrap();
    client_replica.set("shared", "from-client").unwrap();
    server_replica.set("shared", "from-server").unwrap();

    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();

    let served = Arc::clone(&server_replica);
    let serving = tokio::spawn(async move {
        server
            .serve_one(move |request| {
                served.apply(&request).expect("the client's state must apply");
                served.export_all().expect("the server's state must export")
            })
            .await
            .unwrap();
        server.close().await;
    });

    let reply = client
        .request(server_addr, &client_replica.export_all().unwrap())
        .await
        .unwrap();
    client_replica.apply(&reply).unwrap();
    serving.await.unwrap();
    client.close().await;

    for replica in [&client_replica, &server_replica] {
        assert_eq!(replica.get("client_only").as_deref(), Some("a"));
        assert_eq!(replica.get("server_only").as_deref(), Some("b"));
    }
    let client_value = client_replica.get("shared");
    let server_value = server_replica.get("shared");
    assert!(client_value.is_some());
    assert_eq!(client_value, server_value, "conflicting writes must resolve identically on both peers");
}

async fn push_update(replica: &Arc<Replica>, update: &[u8]) {
    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();
    let served = Arc::clone(replica);
    let serving = tokio::spawn(async move {
        server
            .serve_one(move |request| {
                served.apply(&request).unwrap();
                served.export_all().unwrap()
            })
            .await
            .unwrap();
        server.close().await;
    });
    client.request(server_addr, update).await.unwrap();
    serving.await.unwrap();
    client.close().await;
}

#[tokio::test]
async fn replaying_an_older_update_over_the_wire_does_not_roll_a_key_back() {
    let replica = Arc::new(Replica::new(3).unwrap());
    let peer = Replica::new(4).unwrap();

    peer.set("k", "v").unwrap();
    let older = peer.export_all().unwrap();
    push_update(&replica, &older).await;
    assert_eq!(replica.get("k").as_deref(), Some("v"));

    peer.set("k", "w").unwrap();
    let newer = peer.export_all().unwrap();
    push_update(&replica, &newer).await;
    assert_eq!(replica.get("k").as_deref(), Some("w"));

    push_update(&replica, &older).await;
    assert_eq!(replica.get("k").as_deref(), Some("w"), "replaying an older update must not roll the key back");
}
