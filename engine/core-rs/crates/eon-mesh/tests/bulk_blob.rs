//! Bulk blob transfer over the iroh loopback transport. The client requests one chunk per Bulk-class stream, paced by a
//! `BulkPacer`, and verifies each chunk before accepting it. A server that substitutes bytes in one chunk is refused at that
//! chunk.

use std::time::{Duration, Instant};

use eon_mesh::blob::{fetch, Manifest};
use eon_mesh::qos::BulkPacer;
use eon_mesh::transport::PeerNode;

const CHUNK: usize = 256 * 1024;
const LINK_BYTES_PER_SEC: f64 = 4.0 * 1024.0 * 1024.0;

fn sample_blob() -> Vec<u8> {
    (0..3 * CHUNK + 17).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
}

async fn serve_chunks(server: PeerNode, blob: Vec<u8>, requests: usize, tampered_index: Option<usize>) {
    for _ in 0..requests {
        server
            .serve_one(|request| {
                let bytes: [u8; 4] = request.as_slice().try_into().unwrap();
                let index = u32::from_le_bytes(bytes) as usize;
                let start = index * CHUNK;
                let end = (start + CHUNK).min(blob.len());
                let mut chunk = blob[start..end].to_vec();
                if tampered_index == Some(index) {
                    chunk[0] ^= 0xff;
                }
                chunk
            })
            .await
            .unwrap();
    }
    server.close().await;
}

#[tokio::test]
async fn a_paced_blob_fetched_chunk_by_chunk_matches_the_manifest() {
    let blob = sample_blob();
    let manifest = Manifest::for_bytes(&blob, CHUNK).unwrap();

    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();
    let serving = tokio::spawn(serve_chunks(server, blob.clone(), manifest.chunk_count(), None));

    let started = Instant::now();
    let fetched = fetch(&client, server_addr, &manifest, &mut BulkPacer::new(LINK_BYTES_PER_SEC))
        .await
        .unwrap();
    let elapsed = started.elapsed();
    serving.await.unwrap();
    client.close().await;

    assert_eq!(fetched, blob);
    assert!(
        elapsed >= Duration::from_millis(180),
        "the pacer must space the four chunks at link rate, took {elapsed:?}"
    );
}

#[tokio::test]
async fn a_substituted_chunk_stops_the_transfer_at_that_chunk() {
    let blob = sample_blob();
    let manifest = Manifest::for_bytes(&blob, CHUNK).unwrap();

    let server = PeerNode::start().await.unwrap();
    let client = PeerNode::start().await.unwrap();
    let server_addr = server.addr();
    let serving = tokio::spawn(serve_chunks(server, blob.clone(), 3, Some(2)));

    let err = fetch(&client, server_addr, &manifest, &mut BulkPacer::new(LINK_BYTES_PER_SEC))
        .await
        .unwrap_err()
        .to_string();
    serving.await.unwrap();
    client.close().await;
    assert!(err.contains("chunk 2 failed BLAKE3 verification"), "{err}");
}
