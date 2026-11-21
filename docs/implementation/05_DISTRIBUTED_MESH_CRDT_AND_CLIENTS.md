# Manual 05: Distributed Mesh (`iroh 1.4`), Loro CRDTs, Android Presence, and Native Clients

> **Crates & Directories:** `eon-mesh`, `eon-uniffi`, `proto/eon.proto`, `clients/android/`, `clients/daemon/`, `clients/cli/`, `clients/canvas/`  
> **Companion Specs:** `DISTRIBUTED_NETWORKING_MESH.md`, `ANDROID_INTEGRATION_SPECIFICATION.md`, `MULTIMODAL_VISION_AND_RENDERING.md`, `TECH_STACK_AND_TOOLCHAINS.md`

---

## 1. Crate and Client Directory Trees

```text
Eon/
├── proto/eon.proto
├── engine/core-rs/crates/
│   ├── eon-mesh/src/
│   │   ├── lib.rs
│   │   ├── endpoint.rs         # iroh 1.4 Endpoint, EndpointId, mDNS + relay config
│   │   ├── auth.rs             # Mutual Ed25519 peer allowlist & pairing QR/PAKE handshake
│   │   ├── qos.rs              # 4-Class QUIC lanes (RFC 9221 datagrams + stream priorities)
│   │   ├── spatial.rs          # Clock-skew-immune 15ms SensoryClaim election + hysteresis
│   │   ├── split_inference.rs  # Edge <-> Workstation streaming token/audio offload + 500ms fallback
│   │   └── crdt/
│   │       ├── doc_schema.rs   # Loro 1.4 containers + FIDES-guarded merge rules
│   │       ├── sync_proto.rs   # /eon/crdt/1 VersionVector delta exchange & shallow compaction
│   │       └── sqlite_proj.rs  # Root subscription -> batched SQLite FTS5 & vec0 materializer
│   │
│   └── eon-uniffi/src/
│       ├── lib.rs              # uniffi 0.29 exported objects, callbacks, memory trim hook
│       └── android_hwbuf.rs    # Zero-copy JNI AHardwareBuffer_fromHardwareBuffer bridge
│
└── clients/
    ├── daemon/src/             # Background OS service (UDS 0600 SO_PEERCRED / NamedPipe DACL)
    ├── cli/src/                # ratatui + crossterm + ratatui-image TUI client
    ├── canvas/src/             # wgpu + egui AST diff, Sugiyama CFG, and video scrubber
    └── android/                # Gradle Kotlin 2.0 + Compose + cargo-ndk app
```

---

## 2. `eon-mesh`: Implementing on Stabilized `iroh 1.4` (`noq 1.0`)

### 2.1 Post-1.0 `iroh` API Corrections
`iroh` reached `1.0` in May 2026 and `1.4.0` in October 2026. Do **not** use pre-1.0 APIs (`MagicEndpoint`, `NodeId`, or built-in `MdnsDiscovery` feature flags):
1. **Identity & Addressing**: `NodeId` is replaced by `iroh::EndpointId` (an Ed25519 public key backed by `iroh::SecretKey`, persisted in `eon-foundation`'s `CredentialBroker` under `secret://mesh/node_ed25519`).
2. **Modular Discovery**: Add `iroh-mdns-address-lookup` so co-located phone and workstation nodes discover LAN IPs directly in `< 5 ms` with zero internet or relay dependency (leveraging `iroh 1.4`'s immediate local address advertisements when offline).
3. **Router & ALPN Multiplexing**:
   ```rust
   use iroh::{Endpoint, SecretKey, protocol::Router};
   use iroh_mdns_address_lookup::MdnsAddressLookup;

   pub const ALPN_EON_RPC: &[u8]  = b"/eon/rpc/1";
   pub const ALPN_EON_CRDT: &[u8] = b"/eon/crdt/1";

   pub async fn build_mesh_router(
       secret_key: SecretKey,
       rpc_handler: EonRpcProtocol,
       crdt_handler: EonCrdtProtocol,
       blobs_protocol: iroh_blobs::net_protocol::Blobs<iroh_blobs::store::fs::Store>,
   ) -> anyhow::Result<Router> {
       let endpoint = Endpoint::builder()
           .secret_key(secret_key)
           .alpns(vec![
               ALPN_EON_RPC.to_vec(),
               ALPN_EON_CRDT.to_vec(),
               iroh_blobs::ALPN.to_vec(),
           ])
           .address_lookup(MdnsAddressLookup::builder())
           .bind()
           .await?;

       let router = Router::builder(endpoint)
           .accept(ALPN_EON_RPC, rpc_handler)
           .accept(ALPN_EON_CRDT, crdt_handler)
           .accept(iroh_blobs::ALPN, blobs_protocol)
           .spawn();
       Ok(router)
   }
   ```
4. **Peer Authorization Gate (`auth.rs`)**: Because `iroh` allows any party knowing an `EndpointId` to dial over relays, wrap every `ProtocolHandler::accept(connection)` check with `let remote_id = connection.remote_id()?; if !peer_acl.is_paired(&remote_id) { connection.close(403u32.into(), b"unpaired"); return Ok(()); }` BEFORE reading any application bytes! Initial pairing uses a local QR / 6-digit SPAKE2 pin ceremony that exchanges Ed25519 `EndpointId`s and device capability manifests.

### 2.2 Exact Wire Framing and 4-Class QUIC QoS (`src/qos.rs`)

| Lane | QUIC Transport Mechanism (`noq`) | Max Frame Size | Priority & Congestion Rule |
|---|---|---|---|
| **Class 0 (Audio / Barge-In / Spatial)** | `connection.send_datagram(Bytes)` (RFC 9221) | `<= 1,150 bytes` (checked via `connection.max_datagram_size()`) | Unreliable, zero retransmit head-of-line delay. `20 ms` of `16 kHz` PCM16 (`640 B`) + Protobuf header (`~24 B`) fits in 1 MTU packet! |
| **Class 1 (Control / Epochs / Tokens)** | Bidirectional stream (`open_bi`), `send.set_priority(100)` | Length-prefixed (`u32 LE` + `prost` payload, cap `1 MiB`) | Highest stream scheduler weight; preempts Class 2 and Class 3 immediately. |
| **Class 2 (CRDT Deltas / Keyframes)** | Bidirectional stream, `send.set_priority(50)` | Cap `4 MiB` per frame | Coalesced in `25 ms` batches during active voice turns. |
| **Class 3 (Model Weights / Build Blobs)** | `iroh-blobs` verified BLAKE3 Bao tree streams, `send.set_priority(0)` | Chunked resumable | Application-level `Semaphore` throttles chunk reads to `25%` link rate whenever Class 0 datagrams or Class 1 streams were active in the past `200 ms`. |

#### Private WAN path-change test (Linux)

`WanEndpoint::open_with_relay_root` trusts only the supplied DER CA for the provisioned HTTPS relay. Both public WAN constructors remain **relay-only** and check reciprocal Ed25519 pins. `paths()` reads counters without sending traffic. Pinning a peer does not constrain the IP candidates it can advertise: the direct transport used below is compiled only into the Linux test binary.

Run `scripts/test_wan_nat.sh` as the build user on a machine that permits `sudo ip netns`, veth, bridge, conntrack/SNAT and private UDP/TCP. The script creates five namespaces with no uplink or default route. Two peers bootstrap through one privately certified relay behind independent stateful NATs without port forwarding. The relay also provides iroh 1.4's QUIC address discovery (QAD, **not** RFC STUN). After verifying relay transit the fixture opens the private peer route, notifies iroh of the network change, and verifies a 64 KiB transfer on the translated peer sockets. It then drops only the direct routes. It requires the same connection to carry the four typed classes after returning to the relay, with peer path byte deltas and independent forwarding counters. This is not a physical 5G/Wi-Fi handoff.

Class 0 sends numbered 640-byte PCM datagrams every 20 ms, even while the reliable streams stall. `cut` spans route impairment through confirmed relay selection and the start of post-cut counter collection. The deadline is **one-way, application send to peer decode, strictly under 5 ms for every attempt during the cut**. A missing datagram misses the deadline. The roles share Linux `CLOCK_MONOTONIC`; echo RTT is measured and reported separately. Percentiles use received samples only; counts include all attempts. The default exit code gates connectivity, not latency. Add `--require-voice-deadline` to gate both. Raw per-sequence logs, stream receipts and packet counters persist in the printed `target/wan-nat-*` directory on success or failure; ephemeral keys are removed.

The WAN sender budgets 1,150 bytes for unsent datagrams. The receiver budgets 1,200 bytes including queue overhead: one maximum-size legal chunk fits, but not two ordinary 640-byte PCM chunks. A slow reader drops the oldest arrivals rather than accumulating a playback backlog. Smaller frames can share either budget. Neither budget checks frame age or sequence order, or retracts in-flight data.

Run the strict test first with default timing, then add `--short-path-timeout`. Only the direct-capable test endpoints use the shorter profile: 250 ms path keepalive and 1 s default path idle timeout instead of 5 s and 15 s. iroh sets the relay path's idle timeout separately. Neither run signals a network change at the cut. Shorter timers can cost idle traffic and abandon a temporarily slow path; neither public constructor changes its timers.

| Cut phase on the same isolated topology | Attempts | Arrived <5 ms | Arrived late | No receipt | One-way p95 among arrivals | Longest peer decode gap overlapping cut |
|---|---:|---:|---:|---:|---:|---:|
| Before bounding the sender | 754 | 4 | 730 | 20 | 13,864.779 ms | Not recorded |
| Bounded sender only | 754 | 5 | 0 | 749 | 2.801 ms | Not recorded |
| Both queues bounded, default path timing | 755 | 6 | 0 | 749 | 4.435 ms | 15,003.671 ms |
| Both queues bounded, short path timing | 55 | 5 | 0 | 50 | 1.293 ms | 1,020.920 ms |

All four runs migrated without reconnecting and failed the strict deadline. The short profile reduced the interruption, not the 20 ms send cadence. The gap is measured between consecutive peer decodes, including those bracketing the cut if no frame arrived during it. A low latency percentile for the few frames received is not continuous voice delivery.

### 2.3 Schema Reconciliation and Clock-Skew-Immune 15ms Spatial Election (`src/spatial.rs`)

**Protobuf Reconciliation**: Update `SensoryClaim` in `proto/eon.proto` with two optional fields so it matches `DISTRIBUTED_NETWORKING_MESH.md` §8.1 without breaking existing field numbers `1..8`:
```protobuf
message SensoryClaim {
  string claim_id = 1;
  string device_id = 2;
  int64 timestamp_ns = 3;
  float audio_rms_db = 4;
  float snr_score = 5;
  bool is_screen_unlocked = 6;
  bool has_active_hid_input = 7;
  string current_mode = 8;
  uint64 last_hid_activity_ms = 9; // Exact HID recency for continuous decay scoring
  float speaker_confidence = 10;   // WeSpeaker owner cosine similarity [0.0..1.0]
}
```

**Distributed Clock-Skew Trap & Fix**: Never compare `claim.timestamp_ns` across phone and desktop, because their monotonic clocks measure time since each device's respective boot, and NTP wall clocks can skew by `5 to 80 ms`!
- Instead, when Node $A$ detects local VAD onset (or receives a peer's `SensoryClaim` datagram while idle), Node $A$ starts a **local `15.0 ms` `MonotonicClock` timer** and immediately broadcasts its own `SensoryClaim` (even if its local VAD has not crossed threshold yet, reporting its instantaneous `snr_score` and `last_hid_activity_ms`!).
- At `T_local + 15.0 ms`, every node deterministically evaluates the identical normalized score over received claims:
  $$S_i = 0.40 \cdot \text{clamp}\!\left(\frac{\text{SNR}_i - 5.0}{30.0}, 0, 1\right) + 0.30 \cdot \mathbb{I}(\text{HID}_i < 2000\text{ms}) + 0.15 \cdot \mathbb{I}(\text{Unlocked}_i) + 0.15 \cdot \text{SpeakerConf}_i + 0.10 \cdot \mathbb{I}(\text{PrevLeader}_i)$$
- The `+0.10` **stickiness hysteresis term** (active for `15 seconds` after a completed turn unless a different device registers fresh touch/HID input `< 1,000 ms`!) prevents conversational Ping-Pong if the user shifts posture slightly mid-dialogue.

---

## 3. Loro (`1.4`) CRDT Schema, FIDES Merge Guard, and SQLite Projector (`src/crdt/`)

### 3.1 Container Mapping in `LoroDoc`
Use **one `LoroDoc` per logical domain** (`context.loro` and `knowledge.loro`) rather than stuffing millions of historical log lines into a single CRDT state tree:
1. `context.loro`:
   - `LoroMap("nodes")`: Per-peer live presence (`peer_id -> { mode, battery_pct, active_app, leader_lease_until }`).
   - `LoroMap("objectives")`: Active distributed workflow headers and epoch counters (`max_epoch` resolved via monotonic max check on import).
2. `knowledge.loro`:
   - `LoroMap("entities")`: Keyed by `entity_id`, storing `LoroMap` of typed fields.
   - `LoroMap("relations")`: Keyed by `rel_hash = hex(blake3(src || "\0" || pred || "\0" || dst)[..16])`. Each entry stores `{ src, pred, dst, confidence, provenance, integrity_level, updated_lamport }`.

### 3.2 FIDES Integrity Guard Across CRDT Replication
If a compromised or untrusted web-browsing turn on Node B writes a poisoned triple into `knowledge.loro`, standard CRDT Last-Write-Wins would replicate it to Node A! Enforce **Projector-Level Integrity Partitioning**:
- Every CRDT write signs `(container_key, value_digest, integrity_level, peer_id)` or stamps `integrity_level`.
- In `sqlite_proj.rs`, the local SQLite materializer projects `IntegrityLevel::Trusted` triples into `relations_trusted` and `IntegrityLevel::Untrusted` triples into `relations_quarantined`. An update arriving over CRDT sync CANNOT overwrite an existing row in `relations_trusted` if the incoming entry has `integrity_level < Trusted`!
- **Tombstone Compaction**: Every `5,000` local commits during `DORMANT_MODE` when all paired peers have acknowledged a common frontier $F_{\text{acked}}$, compact history via `doc.export(ExportMode::shallow_snapshot(&F_acked))` so CRDT memory stays `< 15 MB` across years of operation.

---

## 4. Android 14/15/16 Presence (`clients/android`) and Zero-Copy FFI (`eon-uniffi`)

### 4.1 Android 14+ (`SDK 35/36`) Foreground Service Type Trap
The baseline `AndroidManifest.xml` declares `android:foregroundServiceType="microphone|connectedDevice|mediaProjection"` on `EonCoreService`. **Critical Android 14+ Crash Trap**: Calling `startForeground(id, notif, FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)` at boot *before* the user has interacted with `MediaProjectionManager.createScreenCaptureIntent()` throws an immediate fatal `SecurityException: Media projections require a foreground service of type ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION` (because the system validates an active projection token inside `ActivityManagerService`!).

**Mandatory Two-Step Service Lifecycle**:
1. On boot / `ROLE_ASSISTANT` wake, call:
   ```kotlin
   ServiceCompat.startForeground(
       this,
       NOTIF_ID,
       notification,
       ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
   )
   ```
2. Only *after* the user grants screen capture and `onActivityResult` delivers `(resultCode == RESULT_OK, data: Intent)`, upgrade the running foreground service mask in-place by calling `ServiceCompat.startForeground(...)` a second time adding `or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION`, and *then* invoke `mediaProjectionManager.getMediaProjection(resultCode, data)`!

### 4.2 Zero-Copy `AHardwareBuffer` Screen Ingestion (`eon-uniffi/src/android_hwbuf.rs`)
To achieve `< 30ms` OmniParser-v2 + SoM vision grounding on Android without copying 12 MB `RGBA_8888` Bitmaps across the JVM heap:
1. Create `ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 3, HardwareBuffer.USAGE_CPU_READ_OFTEN or HardwareBuffer.USAGE_GPU_SAMPLED_IMAGE)`.
2. On `OnImageAvailableListener`, extract `val hwBuf: HardwareBuffer = image.hardwareBuffer ?: return`, pass `hwBuf` to a dedicated 15-line JNI entry point `Java_com_eon_assistant_vision_NativeVision_ingestHardwareBuffer(JNIEnv*, jobject, jobject hwBufObj, jlong timestampNs)`.
3. In Rust (`ndk-sys`), call `AHardwareBuffer_fromHardwareBuffer(env, hwBufObj)`, inspect stride via `AHardwareBuffer_describe`, lock planes read-only with `AHardwareBuffer_lock(ahb, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, ptr::null(), &mut ptr)`, compute the $32 \times 32$ luma delta in SIMD directly over the strided buffer, and unlock immediately if $\Delta \le 5\%$ (`< 0.3 ms` total CPU cost!).

---

## 5. Desktop Daemon, `ratatui` CLI, and `wgpu` Artifact Canvas (`clients/`)

### 5.1 Secure Local IPC in `clients/daemon`
Never bind an unauthenticated localhost TCP socket or world-writable `/tmp/eon.sock` (any browser SSRF or unprivileged local process could submit `TaskExecutionRequest`s to EON!).
- **Linux / macOS**: Bind Unix Domain Socket at `$XDG_RUNTIME_DIR/eon/daemon.sock` (directory `0700`, socket `0600`), and verify `stream.peer_cred()?.uid() == unsafe { libc::geteuid() }` (`SO_PEERCRED` / `LOCAL_PEERCRED`) on every accepted connection!
- **Windows**: Create Named Pipe `\\.\pipe\LOCAL\eon-daemon-<UserSID>` with `FILE_FLAG_FIRST_PIPE_INSTANCE | PIPE_REJECT_REMOTE_CLIENTS` and an explicit `SECURITY_ATTRIBUTES` SDDL string `"D:P(A;;GA;;;OW)"` (Owner-Only Generic All), rejecting network SMB pipe clients and other local accounts.
- **Framing**: Use `tokio_util::codec::LengthDelimitedCodec` (`max_frame_length = 16 MiB`, little-endian 4-byte length header) wrapping a single top-level `IpcEnvelope { oneof payload { ... } }` message added to `proto/eon.proto`.

### 5.2 Terminal UI (`clients/cli`) and GPU Workbench (`clients/canvas`)
- **`clients/cli`**: Built with `ratatui` + `crossterm` + `ratatui-image` (auto-detecting Kitty graphics protocol -> Sixel -> halfblock Unicode fallback). Split into 4 live panes: (1) Duplex Audio Spectrum + Operating Mode pill, (2) Active DAG Topological Tree with live `TaskState` badges and Epoch counter, (3) Streaming `<think>` dim-italic scratchpad + Ciel `Report:` output, and (4) Slash-command prompt (`/mode`, `/shadow accept|reject`, `/cyber audit`, `/mesh peers`, `/pty attach <id>`).
- **`clients/canvas`**: Built in Rust on `wgpu` + `egui` (`eframe`), rendering at 120 Hz:
  1. **AST Structural Diff Pane**: Side-by-side syntax-highlighted source with matched Tree-sitter symbol blocks connected by Bezier ribbons and 1-click NACK/Apply controls.
  2. **P-Code / Disassembly CFG Explorer**: Layered Sugiyama directed graph layout (`petgraph` rank assignment + barycenter crossing minimization) displaying basic blocks, `CBRANCH` true/false edge colors, Z3 solved concrete inputs overlaid on branch edges, and live DAP instruction-pointer (`RIP`/`PC`) highlight!
