//! Delta sync over the iroh loopback transport. Both peers already share a large history, and each has written one new key
//! offline. Exchange: the client sends its version vector, and the server replies with its own version vector plus the
//! updates the client lacks. The client then sends the updates the server lacks. Each reply must be far smaller than a full
//! state export, and both replicas must hold both new keys.

use std::sync::Arc;

use eon_mesh::sync::Replica;
use eon_mesh::transport::PeerNode;

fn frame(version: &[u8], delta: &[u8]) -> Vec<u8> {
    let mut out = (version.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(version);
    out.extend_from_slice(delta);
    out
}

fn unframe(bytes: &[u8]) -> (&[u8], &[u8]) {
    let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    (&bytes[4..4 + len], &bytes[4 + len..])
}

#[tokio::test]
async fn delta_sync_over_the_transport_sends_only_new_writes() {
    let server_replica = Arc::new(Replica::new(1).unwrap());
    let client_replica = Replica::new(2).unwrap();
    for i in 0..200 {
        server_replica.set(&format!("shared{i}"), "base").unwrap();
    }
    client_replica.apply(&server_replica.export_all().unwrap()).unwrap();
    server_replica.set("server_late", "s").unwrap();
    client_replica.set("client_late", "c").unwrap();
    let full_state = server_replica.export_all().unwrap().len();

    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();

    let served = Arc::clone(&server_replica);
    let serving = tokio::spawn(async move {
        let hello = Arc::clone(&served);
        server
            .serve_one(move |request| {
                frame(&hello.version(), &hello.export_since(&request).expect("client version must decode"))
            })
            .await
            .unwrap();
        server
            .serve_one(move |request| {
                served.apply(&request).expect("client delta must apply");
                Vec::new()
            })
            .await
            .unwrap();
        server.close().await;
    });

    let reply = client.request(server_addr.clone(), &client_replica.version()).await.unwrap();
    let (server_version, server_delta) = unframe(&reply);
    assert!(reply.len() < full_state / 4, "the reply must not repeat the shared history");
    client_replica.apply(server_delta).unwrap();

    let client_delta = client_replica.export_since(server_version).unwrap();
    client.request(server_addr, &client_delta).await.unwrap();
    serving.await.unwrap();
    client.close().await;

    assert_eq!(client_replica.get("server_late").as_deref(), Some("s"));
    assert_eq!(server_replica.get("client_late").as_deref(), Some("c"));
}
