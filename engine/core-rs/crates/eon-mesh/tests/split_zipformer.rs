//! Real Zipformer over two loopback-only QUIC endpoints. With `native-zipformer`
//! this gate fails (never skips) if the pinned assets or native runtime are missing.
//! Bootstrap: `python3 scripts/download_models.py --ids voice-test-zipformer-0
//! zipformer-en-2023-06-26-{encoder-int8,decoder,joiner-int8,tokens}` (shell brace expansion).

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eon_mesh::EonError;
use eon_mesh::split_inference::zipformer::{MAX_SAMPLES, PinnedZipformer, Source, SpeechPeer, Utterance};
use iroh::{EndpointAddr, SecretKey};
use sha2::{Digest, Sha256};
use tokio::time::timeout;

fn asset(env: &str, default: &str) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(default))
}

fn release_pcm() -> Vec<i16> {
    let path = asset("EON_ZIPFORMER_TEST_WAV_0", "../eon-voice/tests/fixtures/0.wav");
    let wav = std::fs::read(path).expect("fetch voice-test-zipformer-0 from scripts/models.lock.json");
    assert!(
        format!("{:x}", Sha256::digest(&wav)) == "6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f",
        "expected the pinned 16 kHz mono PCM16 fixture"
    );
    // This exact RIFF file has a canonical 44-byte header.
    wav[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&s| i16::from_le_bytes(s))
        .collect()
}

fn matches_reference(text: &str) -> bool {
    // SHA-256 of the UTF-8 words on the `0.wav` line in the release's
    // test_wavs/trans.txt (without filename, delimiter, or newline).
    format!("{:x}", Sha256::digest(text)) == "c9dc6ebbeac357ed5b118cf0d8323430b6a8ec5368f64337a780420eaf6a479c"
}

async fn bind(key: SecretKey, paired: &SecretKey) -> SpeechPeer {
    SpeechPeer::bind((Ipv4Addr::LOCALHOST, 0).into(), key, paired.public())
        .await
        .unwrap()
}

fn allow<'a>(pcm: &'a [i16], edge: &'a SpeechPeer, worker: &EndpointAddr) -> Utterance<'a> {
    Utterance::AllowPairedWorker {
        pcm,
        edge,
        worker: worker.clone(),
    }
}

#[tokio::test]
async fn split_inference_tests_release_speech_offloads_then_runs_locally_after_disconnect() {
    let dir = asset("EON_ZIPFORMER_DIR", "../../../../perception/models/asr");
    let missing = dir.join("absent-for-negative-test");
    assert!(PinnedZipformer::load(&missing).is_err());
    let workstation = PinnedZipformer::load(&dir).expect("load the verified Zipformer graphs and linked ORT");
    let pcm = release_pcm();
    let (a, b) = (SecretKey::generate(), SecretKey::generate());
    let edge = bind(a.clone(), &b).await;
    let mut worker = bind(b, &a).await;
    let addr = worker.addr();
    assert!(addr.ip_addrs().any(|a| a.ip().is_loopback()));

    let (remote, served) = timeout(Duration::from_secs(12), async {
        tokio::join!(
            allow(&pcm, &edge, &addr).transcribe(None), // no edge model exists yet
            worker.serve_one(Some(&workstation))
        )
    })
    .await
    .expect("loopback RPC deadline");
    served.unwrap();
    let remote = remote.unwrap();
    assert_eq!(remote.source, Source::PairedWorker);
    assert!(
        matches_reference(&remote.text),
        "worker ASR disagrees with release transcript"
    );

    drop(workstation);
    let local = PinnedZipformer::load(&dir).expect("independent local ASR");
    // The local session has never seen the recording. It must not reuse the
    // previous remote transcript when the worker cannot serve a different input.
    let silence = [0; 16_000];
    let (fallback, served) = timeout(Duration::from_secs(12), async {
        tokio::join!(
            allow(&silence, &edge, &addr).transcribe(Some(&local)),
            worker.serve_one(None)
        )
    })
    .await
    .unwrap();
    assert!(matches!(served, Err(EonError::CapabilityNotFound { .. })));
    let fallback = fallback.unwrap();
    assert_eq!(fallback.source, Source::LocalOnChannelError);
    assert!(fallback.text.is_empty());

    worker.close().await; // actually close the endpoint; keep the same authorized destination and route
    let offline = timeout(
        Duration::from_secs(12),
        allow(&pcm, &edge, &addr).transcribe(Some(&local)),
    )
    .await
    .expect("bounded fallback")
    .unwrap();
    assert!(matches!(
        offline.source,
        Source::LocalOnTimeout | Source::LocalOnChannelError
    ));
    assert!(
        matches_reference(&offline.text),
        "local ASR disagrees with release transcript"
    );
    let private = Utterance::LocalOnly(&silence).transcribe(Some(&local)).await.unwrap();
    assert_eq!(private.source, Source::LocalOnly);
    assert!(private.text.is_empty());
    edge.close().await;
}

#[tokio::test]
async fn split_inference_tests_zipformer_requires_the_pair_and_bounded_input() {
    let (a, b) = (SecretKey::generate(), SecretKey::generate());
    let edge = bind(a.clone(), &b).await;
    let mut worker = bind(b, &a).await;
    let addr = worker.addr();
    assert!(matches!(
        allow(&[0], &edge, &edge.addr()).transcribe(None).await,
        Err(EonError::SecurityViolation(_))
    ));
    for bad in [&[][..], &vec![0; MAX_SAMPLES + 1]] {
        assert!(matches!(
            allow(bad, &edge, &addr).transcribe(None).await,
            Err(EonError::ValidationError(_))
        ));
    }
    assert!(matches!(
        Utterance::LocalOnly(&[0]).transcribe(None).await,
        Err(EonError::CapabilityNotFound { .. })
    ));
    let stranger = SpeechPeer::bind((Ipv4Addr::LOCALHOST, 0).into(), SecretKey::generate(), addr.id)
        .await
        .unwrap();
    let (result, served) = timeout(Duration::from_secs(12), async {
        tokio::join!(allow(&[0], &stranger, &addr).transcribe(None), worker.serve_one(None))
    })
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(matches!(served, Err(EonError::SecurityViolation(_))));
    stranger.close().await;
    worker.close().await;
    edge.close().await;
}
