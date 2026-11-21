//! Linux-only roles for scripts/test_wan_nat.sh. No target arguments. Only the
//! test binary gets the IP transport: iroh can probe candidates from its peer.
use super::*;
use crate::sync::Replica;
use iroh::TransportAddr;
use iroh_relay::{
    RelayQuicConfig,
    server::{
        Access, AccessControl, CertConfig, ClientRequest, QuicConfig, Server, ServerConfig,
        TlsConfig,
    },
};
use std::{fs, net::Ipv4Addr, os::unix::fs::MetadataExt, path::Path, sync::Arc};
use tokio::time::{MissedTickBehavior, interval, sleep};

const IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 1);
const URL: &str = "https://10.77.0.1:8443";
const WAIT: Duration = Duration::from_secs(25);
const CHUNK: usize = 64 * 1024;

fn mark(name: &str) {
    fs::write(name, []).unwrap();
}

async fn wait(name: &str) {
    let deadline = if name == "fallback-a" {
        Duration::from_secs(65)
    } else {
        WAIT
    };
    timeout(deadline, async {
        while !Path::new(name).exists() {
            assert!(
                !Path::new("abort").exists(),
                "fixture stopped while waiting for {name}"
            );
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("missing marker {name}"));
}

fn pin(side: &str) -> EndpointId {
    fs::read_to_string(format!("keys/{side}.pub"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[derive(Debug)]
struct Members([EndpointId; 2]);
impl AccessControl for Members {
    async fn on_connect(&self, req: &ClientRequest) -> Access {
        if self.0.contains(&req.endpoint_id()) {
            Access::Allow
        } else {
            Access::Deny { reason: None }
        }
    }
}

async fn relay() {
    let tls = rustls::ServerConfig::builder_with_provider(iroh::tls::default_provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![fs::read("keys/leaf.der").unwrap().into()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(fs::read("keys/leaf.pk8").unwrap()).into(),
        )
        .unwrap();
    let mut relay = iroh_relay::server::RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (IP, 8443),
        CertConfig::Manual { server_config: tls },
    ));
    relay.access = Arc::new(Members([pin("a"), pin("b")]));
    let mut cfg = ServerConfig::default();
    cfg.relay = Some(relay);
    cfg.quic = Some(QuicConfig::new((IP, 7842)));
    let server = Server::spawn(cfg).await.unwrap();
    println!(
        "relay HTTPS={:?} QAD={:?}",
        server.https_addr(),
        server.quic_addr()
    );
    mark("ready");
    wait("fallback-a").await;
    let m = &server.metrics().server;
    let (rx, tx) = (m.bytes_recv.get(), m.bytes_sent.get());
    fs::write("counter-ready", monotonic_ns().to_string()).unwrap();
    wait("finished").await;
    let (rx, tx) = (m.bytes_recv.get() - rx, m.bytes_sent.get() - tx);
    println!(
        "relay fallback_forwarded_rx={rx} fallback_forwarded_tx={tx} qad_connections={}",
        m.qad_connections.get()
    );
    assert!(rx >= CHUNK as u64 && tx >= CHUNK as u64 && m.qad_connections.get() >= 2);
    server.shutdown().await.unwrap();
}

fn audio(seq: usize) -> AudioPcmChunk {
    AudioPcmChunk {
        sequence_number: seq as i64,
        pcm_data: vec![seq as u8; 640],
        ..Default::default()
    }
}

// The roles share a kernel and time namespace; this clock is NOT shared
// between physical devices.
fn monotonic_ns() -> i128 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    i128::from(t.tv_sec) * 1_000_000_000 + i128::from(t.tv_nsec)
}

// Sample at a fixed cadence, independent of receipt/loss and stream backpressure.
// Count local send errors as attempts too.
async fn measure(ep: &WanEndpoint) -> EonResult<()> {
    let mut tick = interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let start = Instant::now();
    let mut samples = Vec::new();
    let mut drain = 0;
    let mut steady = [false; 2];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if Path::new("drain").exists() {
                    drain += 1;
                    if drain > 10 { break; }
                    continue;
                }
                let seq = samples.len();
                assert!(seq < 4000);
                let phase = if Path::new("counter-ready").exists() { "fallback" }
                    else if Path::new("cut-start").exists() { "cut" }
                    else if Path::new("direct-a").exists() { "direct" }
                    else if Path::new("open-start").exists() { "opening" }
                    else if Path::new("bootstrap-a").exists() { "bootstrap" }
                    else { "setup" };
                let path = ep.paths().iter().find(|p| p.is_selected()).map(|p| p.remote_addr().clone());
                let frame = audio(seq);
                let now = Instant::now();
                let ns = monotonic_ns();
                let sent = ep.send_audio(&frame);
                println!("send seq={seq} t_us={} phase={phase} mono_ns={ns} path={path:?}", (now-start).as_micros());
                samples.push((now, phase, path, false));
                if let Err(e) = sent { println!("audio_send_error seq={seq}: {e}"); }
            }
            frame = ep.recv_audio() => {
                let frame = frame?;
                let now = Instant::now();
                let seq = usize::try_from(frame.sequence_number).unwrap();
                assert_eq!(frame, audio(seq));
                let (sent, phase, path, seen) = &mut samples[seq];
                assert!(!*seen);
                *seen = true;
                println!("echo seq={seq} rtt_us={}", (now-*sent).as_micros());
                if ep.paths().iter().any(|p| p.is_selected() && Some(p.remote_addr()) == path.as_ref()) {
                    steady[0] |= *phase == "direct" && path.as_ref().is_some_and(TransportAddr::is_ip);
                    steady[1] |= *phase == "fallback" && path.as_ref().is_some_and(TransportAddr::is_relay);
                }
            }
        }
    }
    assert_eq!(steady, [true; 2], "no verified audio echo on both paths");
    Ok(())
}

async fn batch(ep: &WanEndpoint, side: &str, stage: u32) -> EonResult<()> {
    let ctrl = ReliableFrame::Control(EpochInvalidationEvent {
        invalidated_epoch: stage.into(),
        task_id: "path-change".into(),
        ..Default::default()
    });
    let bulk = ReliableFrame::Bulk {
        index: stage,
        bytes: vec![stage as u8; CHUNK],
    };
    let doc = Replica::new(u64::from(stage) * 2 + u64::from(side == "b"))?;
    if side == "a" {
        doc.set("stage", &stage.to_string())?;
        let delta = ReliableFrame::Delta(CrdtDeltaSync {
            crdt_delta_payload: doc.export_all()?,
            ..Default::default()
        });
        let timed = async |lane, frame: &ReliableFrame| {
            let start = Instant::now();
            let result = ep.send(frame).await;
            println!(
                "stream stage={stage} class={lane} receipt_us={} ok={}",
                start.elapsed().as_micros(),
                result.is_ok()
            );
            result
        };
        tokio::try_join!(timed(1, &ctrl), timed(2, &delta), timed(3, &bulk))?;
    } else {
        let (one, two, three) = tokio::try_join!(ep.recv(), ep.recv(), ep.recv())?;
        let frames = [one, two, three];
        assert!(frames.contains(&ctrl) && frames.contains(&bulk));
        let delta = frames
            .iter()
            .find_map(|f| match f {
                ReliableFrame::Delta(d) => Some(d),
                _ => None,
            })
            .unwrap();
        doc.apply(&delta.crdt_delta_payload)?;
        assert_eq!(doc.get("stage"), Some(stage.to_string()));
    }
    Ok(())
}

// Byte deltas for the exact selected address, not total connection statistics.
fn bytes(ep: &WanEndpoint, addr: &TransportAddr, tx: bool) -> Option<u64> {
    let stats = ep
        .paths()
        .iter()
        .find(|p| p.is_selected() && p.remote_addr() == addr)?
        .stats();
    Some(if tx {
        stats.udp_tx.bytes
    } else {
        stats.udp_rx.bytes
    })
}

async fn witness(ep: &WanEndpoint, side: &str, stage: u32, addr: &TransportAddr) -> EonResult<()> {
    let other = if side == "a" { "b" } else { "a" };
    let phase = match stage {
        0 => "bootstrap",
        1 => "direct",
        3 => "fallback",
        _ => unreachable!(),
    };
    let before = timeout(WAIT, async {
        loop {
            if let Some(n) = bytes(ep, addr, side == "a") {
                break n;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {addr:?}; current paths: {:?}", ep.paths()));
    mark(&format!("{phase}-{side}"));
    wait(&format!("{phase}-{other}")).await;
    if stage == 3 {
        wait("counter-ready").await;
    }
    batch(ep, side, stage).await?;
    let n = bytes(ep, addr, side == "a").unwrap() - before;
    println!(
        "witness side={side} phase={phase} stable_id={} addr={addr:?} wire_bytes={n}",
        ep.conn.stable_id()
    );
    assert!(n >= CHUNK as u64);
    Ok(())
}

async fn peer(side: &str) -> EonResult<()> {
    let (other, host, mapped, remote) = if side == "a" {
        ("b", 1, 2, 3)
    } else {
        ("a", 2, 3, 2)
    };
    let me: SecretKey = fs::read_to_string(format!("keys/{side}.key"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let peer = pin(other);
    let relay: RelayUrl = URL.parse().unwrap();
    let interface_loss = match std::env::var("EON_WAN_CUT").unwrap().as_str() {
        "blackhole" => false,
        "interface-loss" => true,
        _ => panic!("unknown route impairment"),
    };
    let ca = fs::read("keys/ca.der").unwrap();
    let rate = NonZeroU32::new(4 * 1024 * 1024).unwrap();
    // Verify the shipping private-CA, relay-only API, separately from migration.
    let only =
        WanEndpoint::open_with_relay_root(me.clone(), peer, relay.clone(), ca.clone(), rate, WAIT)
            .await?;
    assert_eq!(only.peer_id(), peer);
    batch(&only, side, 4).await?;
    mark(&format!("root-{side}"));
    wait(&format!("root-{other}")).await;
    only.close().await;
    mark(&format!("closed-{side}"));
    wait(&format!("closed-{other}")).await;

    // No peer socket is handed to connect. QAD reports the *translated* socket;
    // the opposite LAN has no route to this socket's untranslated address.
    let bind = (Ipv4Addr::new(192, 168, host, 2), 41770);
    // Neither timing profile gets a notification of a distant black hole.
    let transport = QuicTransportConfig::builder();
    let transport = match std::env::var("EON_WAN_PATH_PROFILE").unwrap().as_str() {
        "default" => transport,
        "short" => transport
            .default_path_keep_alive_interval(Duration::from_millis(250))
            .default_path_max_idle_timeout(Duration::from_secs(1)),
        _ => panic!("unknown path timing profile"),
    };
    let builder = relay_builder(transport)
        .bind_addr(bind)
        .unwrap()
        .ca_tls_config(iroh::tls::CaTlsConfig::custom_roots([ca.into()]));
    let ep = WanEndpoint::open_configured(
        me,
        peer,
        RelayConfig::new(relay.clone(), Some(RelayQuicConfig::new(7842))),
        rate,
        WAIT,
        builder,
    )
    .await?;
    assert_eq!(ep.peer_id(), peer);
    let expected = |i| std::net::SocketAddrV4::new(Ipv4Addr::new(10, 77, 0, i), 41770);
    timeout(WAIT, async {
        loop {
            if ep
                .endpoint
                .addr()
                .ip_addrs()
                .any(|a| *a == expected(mapped).into())
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("QAD did not report this endpoint's SNAT mapping");
    println!(
        "QAD side={side} bound={bind:?} reflected={} peer={peer}",
        expected(mapped)
    );
    let app = async {
        let relay = TransportAddr::Relay(relay);
        witness(&ep, side, 0, &relay).await?;
        let standby = ep
            .paths()
            .iter()
            .find(|p| p.remote_addr() == &relay)
            .expect("bootstrap relay path")
            .id();
        let relay_selected = || {
            ep.paths()
                .get(standby)
                .filter(|p| p.remote_addr() == &relay)
                .map(|p| p.is_selected())
        };
        mark(&format!("bootstrap-proven-{side}"));
        wait("route-open").await;
        // The on-link routers have actually enabled the peer route. Notify
        // iroh, since routers in separate namespaces don't generate local
        // interface notifications. No address or synthetic path is injected.
        ep.endpoint.network_change().await;
        witness(&ep, side, 1, &TransportAddr::Ip(expected(remote).into())).await?;
        if interface_loss {
            assert_eq!(relay_selected(), Some(false), "no open relay standby");
        }
        if side == "a" {
            sleep(Duration::from_millis(600)).await;
            mark("direct-proven");
        }
        wait("cut").await;
        if interface_loss && side == "a" {
            // Only A lost a local interface. B receives no fixture hint.
            ep.endpoint.network_change().await;
        }
        batch(&ep, side, 2).await?; // receipts may stall; never reconnect or replay
        witness(&ep, side, 3, &relay).await?;
        if interface_loss {
            assert_eq!(
                relay_selected(),
                Some(true),
                "original relay path not reused"
            );
        }
        if side == "a" {
            sleep(Duration::from_millis(600)).await;
            mark("drain");
        }
        Ok(())
    };
    if side == "a" {
        tokio::try_join!(app, measure(&ep))?;
        mark("finished");
        wait("b-complete").await;
    } else {
        let echo = async {
            loop {
                tokio::select! {
                    chunk = ep.recv_audio() => {
                        let chunk = chunk?;
                        let received = monotonic_ns();
                        ep.send_audio(&chunk)?;
                        println!("received seq={} mono_ns={received}", chunk.sequence_number);
                    }
                    _ = sleep(Duration::from_millis(20)) => {
                        if Path::new("finished").exists() { return Ok(()); }
                    }
                }
            }
        };
        tokio::try_join!(app, echo)?;
        mark("b-complete");
    }
    ep.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "only via scripts/test_wan_nat.sh; requires a disposable isolated topology"]
async fn iroh_and_qos_tests_nat_path_change() -> EonResult<()> {
    let ns = fs::metadata("/proc/self/ns/net").unwrap().ino();
    assert_ne!(
        ns,
        std::env::var("EON_WAN_PARENT_NS")
            .unwrap()
            .parse::<u64>()
            .unwrap()
    );
    assert_eq!(
        Some(ns),
        std::env::var("EON_WAN_NETNS")
            .ok()
            .and_then(|s| s.parse().ok())
    );
    match std::env::var("EON_WAN_ROLE").unwrap().as_str() {
        "relay" => {
            relay().await;
            Ok(())
        }
        role @ ("a" | "b") => peer(role).await,
        _ => panic!("unknown namespace role"),
    }
}
