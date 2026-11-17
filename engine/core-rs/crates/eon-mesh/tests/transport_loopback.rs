//! Loopback tests for the iroh peer transport. No relay and no external network: both endpoints bind on this host.

use eon_mesh::transport::{PeerNode, ALPN};

fn reversed(bytes: Vec<u8>) -> Vec<u8> {
    bytes.into_iter().rev().collect()
}

#[tokio::test]
async fn request_and_reply_round_trip_between_two_loopback_peers() {
    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();

    let serving = tokio::spawn(async move {
        server.serve_one(reversed).await.unwrap();
        server.close().await;
    });

    let reply = client.request(server_addr, b"hello mesh").await.unwrap();
    assert_eq!(reply, b"hsem olleh".to_vec());
    serving.await.unwrap();
    client.close().await;
}

#[tokio::test]
async fn a_peer_that_does_not_offer_the_protocol_is_refused() {
    let other = PeerNode::start_with_alpn(b"eon/other/9").await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let other_addr = other.addr();

    let attempt = tokio::time::timeout(std::time::Duration::from_secs(5), client.request(other_addr, b"ping")).await;
    assert!(!matches!(attempt, Ok(Ok(_))), "a peer on a different ALPN must not be reachable");
    assert_ne!(ALPN, b"eon/other/9");

    other.close().await;
    client.close().await;
}
