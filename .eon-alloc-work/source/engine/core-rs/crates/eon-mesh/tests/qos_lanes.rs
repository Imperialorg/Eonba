//! Loopback tests for the four traffic classes. Audio travels as datagrams, the other classes as streams.

use eon_mesh::qos::TrafficClass;
use eon_mesh::transport::PeerNode;

fn reversed(bytes: Vec<u8>) -> Vec<u8> {
    bytes.into_iter().rev().collect()
}

#[tokio::test]
async fn audio_frames_arrive_as_datagrams_in_order() {
    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();

    let receiving = tokio::spawn(async move {
        let link = server.accept().await.unwrap();
        let mut frames = Vec::new();
        for _ in 0..5 {
            frames.push(link.recv_audio().await.unwrap());
        }
        server.close().await;
        frames
    });

    let link = client.connect(server_addr).await.unwrap();
    let sent: Vec<Vec<u8>> = (0u8..5).map(|i| vec![i; 640]).collect();
    for frame in &sent {
        link.send_audio(frame).unwrap();
    }

    assert_eq!(receiving.await.unwrap(), sent);
    client.close().await;
}

#[tokio::test]
async fn audio_frame_larger_than_the_datagram_limit_is_refused_not_split() {
    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();

    let accepting = tokio::spawn(async move {
        let link = server.accept().await.unwrap();
        server.close().await;
        link
    });

    let link = client.connect(server_addr).await.unwrap();
    let oversized = vec![0u8; 64 * 1024];
    let refused = link.send_audio(&oversized).unwrap_err().to_string();
    assert!(refused.contains("datagram limit"), "unexpected refusal: {refused}");

    drop(accepting.await.unwrap());
    client.close().await;
}

#[tokio::test]
async fn stream_classes_round_trip_and_audio_is_not_a_stream() {
    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();

    let serving = tokio::spawn(async move {
        server.serve_one(reversed).await.unwrap();
        server.close().await;
    });

    let reply = client
        .request_with_class(server_addr.clone(), TrafficClass::Control, b"epoch 7")
        .await
        .unwrap();
    assert_eq!(reply, b"7 hcope".to_vec());
    serving.await.unwrap();

    assert!(client
        .request_with_class(server_addr, TrafficClass::Audio, b"frame")
        .await
        .is_err());
    client.close().await;
}
