use super::{Server, endpoint, relay};
use eon_mesh::{
    EonResult,
    wan::{
        AudioPcmChunk, ReliableFrame, WanEndpoint,
        continuity::{CapturePlan, receive, transmit},
    },
};
use iroh::SecretKey;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::{sync::oneshot, time::sleep};

async fn pair() -> EonResult<(Server, WanEndpoint, WanEndpoint)> {
    let (a, b) = (SecretKey::generate(), SecretKey::generate());
    let (aid, bid) = (a.public(), b.public());
    let (server, url) = relay(&[&a, &b]).await;
    let (tx, rx) = tokio::try_join!(endpoint(a, bid, &url), endpoint(b, aid, &url))?;
    Ok((server, tx, rx))
}

// Use the existing pin-only loopback relay fixture, never construct a fake
// grant or select a different constructor in the executable.
#[tokio::test]
async fn iroh_and_qos_tests_continuity_pair() -> EonResult<()> {
    let (server, tx, rx) = pair().await?;
    let before = server.metrics().server.bytes_sent.get();
    let plan = CapturePlan::new(2).unwrap();
    let marks = || {
        let (tx, rx) = mpsc::channel();
        tokio::spawn(async move {
            for _ in 0..2 {
                sleep(Duration::from_millis(250)).await;
                tx.send(Instant::now()).unwrap();
            }
        });
        rx
    };
    let (sent, captured) = tokio::join!(transmit(tx, plan), receive(rx, plan, marks));
    let sent = sent.unwrap();
    assert!((3..=100).contains(&sent));
    let text = captured.to_string();
    assert!(captured.bracketed(), "{text}");
    assert!(text.contains("phase=during") && text.contains(&format!("queued={sent} ")));
    assert!(server.metrics().server.bytes_sent.get() - before >= 640 * 3);
    server.shutdown().await.unwrap();
    Ok(())
}

fn chunk(seq: i64) -> AudioPcmChunk {
    AudioPcmChunk {
        sequence_number: seq,
        pcm_data: vec![0; 640],
        ..Default::default()
    }
}

#[tokio::test]
async fn iroh_and_qos_tests_continuity_missing_and_late_tail() -> EonResult<()> {
    let (server, tx, rx) = pair().await?;
    let plan = CapturePlan::new(1).unwrap();
    let (first, started) = oneshot::channel();
    let peer = async {
        tx.recv().await?;
        tx.send_audio(&chunk(2))?;
        started.await.unwrap();
        // Inject a duplicate, reordered tail after FINISH, and an absent seq=3.
        tx.send_audio(&chunk(2))?;
        tx.send(&ReliableFrame::Bulk {
            index: 3,
            bytes: b"eon/continuity/1/queued".to_vec(),
        })
        .await?;
        sleep(Duration::from_millis(50)).await;
        tx.send_audio(&chunk(1))?;
        tx.recv().await?;
        let _ = tx.recv_audio().await; // let the receiver finish the drain receipt
        EonResult::Ok(())
    };
    let (res, captured) = tokio::join!(
        peer,
        receive(rx, plan, || {
            first.send(()).unwrap();
            mpsc::channel().1 // no operator marks
        })
    );
    res?;
    let text = captured.to_string();
    assert!(!captured.bracketed());
    assert!(
        text.contains(
            "status=complete bracketed=false limit=50 queued=3 unique=2 missing=1 duplicates=1"
        ),
        "{text}"
    );
    assert!(text.contains("seq=2 ") && text.contains("seq=1 "));
    tx.close().await;
    server.shutdown().await.unwrap();
    Ok(())
}

#[tokio::test]
async fn iroh_and_qos_tests_continuity_reconnect_or_malformed_is_not_a_capture() -> EonResult<()> {
    for fault in ["reconnect", "wrong_pcm", "short_total"] {
        let (server, tx, rx) = pair().await?;
        let plan = CapturePlan::new(1).unwrap();
        let (first, started) = oneshot::channel();
        let peer = async {
            tx.recv().await?;
            tx.send_audio(&chunk(1))?;
            started.await.unwrap();
            match fault {
                "reconnect" => assert!(tx.reconnect(Duration::from_millis(250)).await.is_err()),
                "wrong_pcm" => {
                    let mut malformed = chunk(2);
                    malformed.pcm_data[0] = 42;
                    tx.send_audio(&malformed)?;
                    let _ = tx.recv_audio().await; // receiver rejects and closes
                    tx.close().await;
                }
                _ => {
                    let _ = tx
                        .send(&ReliableFrame::Bulk {
                            index: 0,
                            bytes: b"eon/continuity/1/queued".to_vec(),
                        })
                        .await;
                    let _ = tx.recv_audio().await;
                    tx.close().await;
                }
            }
            EonResult::Ok(())
        };
        let (res, captured) = tokio::join!(
            peer,
            receive(rx, plan, || {
                first.send(()).unwrap();
                mpsc::channel().1
            })
        );
        res?;
        let text = captured.to_string();
        assert!(!captured.bracketed() && !text.contains("status=complete"));
        assert!(
            text.contains("queued=unknown unique=1 missing=unknown"),
            "{text}"
        );
        match fault {
            "short_total" => assert!(text.contains("status=inconsistent_finish")),
            "wrong_pcm" => assert!(text.contains("status=invalid_probe")),
            _ => (),
        }
        server.shutdown().await.unwrap();
    }
    Ok(())
}
