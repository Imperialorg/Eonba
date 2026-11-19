//! A peer that learns another peer's address from a signed announcement can dial it over the loopback transport.

use eon_mesh::discovery::{Announcement, Directory};
use eon_mesh::transport::PeerNode;

#[tokio::test]
async fn a_peer_found_through_a_signed_announcement_can_be_dialed() {
    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_id = server.addr().id;

    let carried_by_someone_else = server.announce(1).unwrap().encode().unwrap();
    let mut directory = Directory::default();
    assert!(directory.accept(Announcement::decode(&carried_by_someone_else).unwrap()).unwrap());
    let found = directory
        .lookup(&server_id)
        .expect("the announced peer must be in the directory")
        .clone();

    let serving = tokio::spawn(async move {
        server
            .serve_one(|request: Vec<u8>| request.into_iter().rev().collect())
            .await
            .unwrap();
        server.close().await;
    });
    let reply = client.request(found, b"ping").await.unwrap();
    assert_eq!(reply, b"gnip".to_vec());
    serving.await.unwrap();
    client.close().await;
}
