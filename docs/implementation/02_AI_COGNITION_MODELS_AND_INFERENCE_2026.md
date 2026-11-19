# Manual 02: 2026 AI Stack, Cognition, Constrained Decoding, and Multimodal Vision

> **Crates:** `eon-cognition` (Level 5), `eon-vision` (Level 4+), `perception/models/`  
> **Companion Specs:** `CIEL_CORE_SPECIFICATION.md`, `MODELS_AND_WEIGHTS_SETUP.md`, `MEMORY_AND_APPRAISAL_SPECIFICATION.md`, `MULTIMODAL_VISION_AND_RENDERING.md`, `SYSTEM_PROMPTS_AND_PERSONA.md`

---

## 1. Crate Module Trees

```text
engine/core-rs/crates/
├── eon-cognition/src/
│   ├── lib.rs
│   ├── router/
│   │   ├── mod.rs              # 3-Tier Thought Acceleration coordinator
│   │   ├── reflex.rs           # Tier 0 (<20ms): exact hash, regex, BGE/Arctic micro-intent cosine
│   │   ├── pre_cognition.rs    # Streaming ASR partial-token KV-cache prefix warmer
│   │   └── stream_splitter.rs  # Chunk-safe <think>...</think> vs spoken stream state machine
│   ├── backend/
│   │   ├── mod.rs              # LocalInferenceBackend trait, GenerationRequest, TokenEvent
│   │   ├── llamacpp.rs         # llama-cpp-2 (0.1.160) GGUF/QAT + Vulkan/CUDA/NEON + mmap
│   │   ├── mistral_engine.rs   # mistralrs (0.8.2) PagedAttention, UQFF, Gemma 4 / Qwen 3.5/3.6
│   │   └── grammar.rs          # llguidance (1.9.1) + TokTrie async logit mask & cancellation
│   ├── compiler/
│   │   ├── dag_schema.rs       # Discriminated-union JSON Schema 2020-12 generator from Registry
│   │   ├── workflow_compiler.rs# Natural language -> validated Workflow + CognitiveContinuation
│   │   ├── smt_bridge.rs       # z3 (0.21.1) worker pool + mobile mesh RPC delegation
│   │   └── rollout.rs          # Best-of-N parallel shadow candidate scorer
│   ├── memory/
│   │   ├── governor.rs         # 3-Tier mlock / mmap / madvise(MADV_DONTNEED) RSS controller
│   │   ├── ledger.rs           # 4,096-token Layer 1 working context + 5-line auto-compactor
│   │   ├── hybrid_rag.rs       # SQLite FTS5 + sqlite-vec vec0 + Reciprocal Rank Fusion (RRF)
│   │   └── procedural.rs       # Parameterized DAG macros + Wilson score confidence decay
│   └── persona/
│       ├── prompts.rs          # Ciel prompt builder + per-model thinking template flags
│       └── modes.rs            # 5 Macro Operating Modes & sensory gate policy
│
└── eon-vision/src/
    ├── lib.rs
    ├── capture/
    │   ├── sampler.rs          # 32x32 luma + block-SSIM 1-5 FPS adaptive keyframe trigger
    │   └── ring_buffer.rs      # 30-second shared-memory / zero-copy AHardwareBuffer ring
    ├── grounding/
    │   ├── omniparser.rs       # ort 2.0 YOLOv11 icon detector + PaddleOCR/Surya ONNX
    │   ├── som_renderer.rs     # Non-overlapping [1]..[K] Set-of-Mark badge layout & draw
    │   └── zoom_cascade.rs     # Coarse-to-Fine 2-stage ZoomIn for 4K desktop / dense IDEs
    └── video_rag/
        ├── indexer.rs          # Demux -> Zipformer word timeline + 1 FPS SigLIP2 int8 vec0
        └── inspector.rs        # Timestamp RRF lookup -> 3-5 high-res frame cropper for VLM
```

---

## 2. 2026 Open-Weight Model Matrix and Hardware Profiles

Between mid-2025 and late 2026, open-weight small and medium models underwent three architectural leaps that directly benefit EON:
1. **Apache-2.0 Unification**: Both Google's **Gemma 4** family (March to June 2026) and Alibaba's **Qwen 3.5 / Qwen 3.6** families (February to April 2026) ship under plain **Apache-2.0** licenses with zero commercial or field-of-use restrictions, replacing the restrictive legacy `gemma-2-2b-it` custom license.
2. **Mobile RAM Optimization**: Gemma 4's smaller dense models (`E2B`, `E4B`) incorporate **Per-Layer Embeddings (PLE)**, **Shared KV Cache** across final layers, alternating 512-token sliding-window and pruned-RoPE global attention, and native audio/vision encoders, delivering frontier multimodal reasoning in edge memory budgets.
3. **Sub-30B Coding Supremacy**: `Qwen3.6-27B` (dense, hybrid Gated DeltaNet + gated attention) achieves **77.2% on SWE-bench Verified** in ~16.8 GB at 4-bit, outperforming 397B MoE models from two months prior, while `Qwen3.6-35B-A3B` activates only 3B parameters per token for ultra-fast workstation CPU/GPU inference.

### 2.1 Canonical 2026 Deployment Profiles

Replace the legacy model table in `MODELS_AND_WEIGHTS_SETUP.md` with these four hardware-keyed profiles:

| Profile | Target Hardware | Tactical / Multimodal Model | Format & Exact Disk Footprint | Key Architectural Notes & Caveats |
|---|---|---|---|---|
| **P0: Ultra-Light Mobile** (`<= 6 GB` device RAM) | Mid-range Android (`aarch64`) | `Qwen/Qwen3.5-1.7B` (or `Qwen3.5-0.8B`) | GGUF `Q4_K_M` (`~1.15 GB` / `~0.58 GB`) | Hybrid Gated DeltaNet, 262K native context. Pass `enable_thinking: false` on Tier 1 voice turns! |
| **P1: Flagship Mobile Edge** (`8 to 12 GB` device RAM) | Snapdragon 8 Gen 3/4, Tensor G4/G5, Apple A18+ | `google/gemma-4-E2B-it` (or `Qwen3.5-4B` text-heavy) | Official QAT GGUF / `Q4_K_M` (`3.43 GB` incl. vision/audio; Unsloth `Q3_K_XL` `~2.45 GB`) | Native text + image (70 to 1120 selectable tokens) + USM conformer audio. PLE + Shared KV cache minimizes active runtime RSS. |
| **P2: Developer Laptop / Single-GPU** (`12 to 16 GB` RAM/VRAM) | Mac M2/M3/M4 16GB, RTX 4060/4070 Laptop | **`google/gemma-4-12B-it`** (`QAT` `6.98 GB`) OR **`Qwen/Qwen3.5-9B`** (`Q4_K_M` `~5.8 GB`) | GGUF QAT / `Q4_K_M` (`5.8 to 7.38 GB`) | `Gemma-4-12B` is encoder-free unified multimodal (LiveCodeBench `72.0`). `Qwen3.5-9B` leads sub-10B structured tool calling. |
| **P3: Workstation / Mesh Compute** (`16 to 32+ GB` VRAM / Unified) | RTX 4080/4090/5090, Mac Studio, Linux Server | **`Qwen/Qwen3.6-27B`** (`Q4_K_M` `16.8 GB`) OR **`Qwen/Qwen3.6-35B-A3B`** (`~23 GB`, 3B active) | Unsloth Dynamic GGUF `Q4_K_M` (`16.8 to 23.0 GB`) | `Qwen3.6-27B` hits `77.2%` SWE-bench Verified and restores per-request hybrid thinking (`enable_thinking: true/false`). |

> **Critical Quantization Warning for `Gemma-4-26B-A4B` MoE**: If deploying `google/gemma-4-26B-A4B-it` as an alternative P3 MoE (4B active params), **do not** use plain uniform `Q4_0` GGUF. Because its MoE expert intermediate dimension is `704`, uniform 4-bit block quantization measurably degrades accuracy; use Unsloth's dynamic mixed-precision GGUF (`iq4_xs` / `q4_k_xl`) or `Q8_0`.

---

## 3. `eon-cognition`: Dual Inference Backend and KV-Cache Pre-Cognition

### 3.1 `LocalInferenceBackend` Trait (`src/backend/mod.rs`)

Abstract inference behind a single streaming trait so Android NDK builds use `llama-cpp-2` (`0.1.160`, minimal C/C++ footprint with Vulkan/NEON and direct `mmap` file-descriptor control) while desktop/server builds optionally enable `mistralrs` (`0.8.2`, PagedAttention, FlashAttention-2, UQFF, and native multimodal Gemma 4 / Qwen 3.5/3.6 pipelines):

```rust
use bytes::Bytes;
use tokio::sync::mpsc;
use eon_primitives::EonResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingPolicy {
    /// Force immediate answer after empty/bypassed think block (<120ms Tier 1 voice budget)
    DisabledForLatency,
    /// Bounded reasoning budget (e.g., max 512 thought tokens for tactical DAG synthesis)
    BoundedTokens(u32),
    /// Unrestricted deep reasoning (Tier 2 background code synthesis & vulnerability analysis)
    Unbounded,
}

pub struct GenerationRequest {
    pub session_id: String,
    pub static_prefix_hash: u64,          // Cached system prompt + RepoMap KV prefix
    pub messages: Vec<ChatMessage>,
    pub media_inputs: Vec<MediaAttachment>, // Cropped SoM images (70/280/560 token budget)
    pub thinking: ThinkingPolicy,
    pub json_schema: Option<serde_json::Value>, // Compiled via llguidance when Some
    pub max_tokens: u32,
    pub temperature: f32,
    pub cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Debug, Clone)]
pub enum StreamPiece {
    ThoughtDelta(String),  // Inside <think>...</think>; routed ONLY to HUD/CLI/Journal
    SpokenDelta(String),   // Outside </think>; routed to ClauseChunker -> TTS / DAG parser
    Finished { prompt_tokens: u32, thought_tokens: u32, output_tokens: u32, prefill_ms: u32 },
}

#[async_trait::async_trait]
pub trait LocalInferenceBackend: Send + Sync {
    /// Pre-populates KV cache during live user speech without emitting output tokens.
    async fn prewarm_prefix(&self, session_id: &str, prefix_tokens: &[u32]) -> EonResult<usize>;

    /// Streams categorized deltas with <50us per-token grammar masking when json_schema is set.
    async fn generate_stream(
        &self,
        req: GenerationRequest,
        tx: mpsc::Sender<EonResult<StreamPiece>>,
    ) -> EonResult<()>;

    /// Releases non-pinned pages on entry to DORMANT_MODE.
    fn evict_working_pages(&self) -> EonResult<()>;
}
```

### 3.2 Predictive Pre-Cognition KV-Cache Prefix Sharing (`src/router/pre_cognition.rs`)

To hit `< 120ms` Tactical SLM first-token latency after VAD end-of-utterance:
1. **Slot 0 (Pinned System Prefix)**: At workspace open, `eon-cognition` tokenizes `[Master Ciel System Prompt + Active Operating Mode + Top-Ranked PageRank RepoMap (<1,200 tokens)]` and evaluates those tokens into KV-cache sequence `0`, saving the KV state in RAM (~18 to 45 MB with `Q8_0` KV quantization).
2. **Slot 1 (Live Speech Speculative Prefill)**: Every `160ms` while the user is speaking, Zipformer emits a partial transcript $U_k$. If $|U_k| - |U_{k-1}| \ge 4$ tokens:
   - Copy sequence `0` metadata (`llama_kv_cache_seq_cp(ctx, 0, 1, 0, sys_len)`).
   - Compare the newly tokenized user turn $T(U_k)$ against the previously prefilled tokens $T(U_{k-1})$ to find their longest common token prefix length $L_{\text{lcp}}$.
   - Truncate divergent tail tokens only: `llama_kv_cache_seq_rm(ctx, 1, sys_len + L_lcp, -1)`.
   - Run `llama_decode` on the suffix `T(U_k)[L_lcp..]`.
3. When VAD fires end-of-utterance `250 to 800ms` later, 85% to 95% of the prompt is **already prefilled in KV sequence 1**! Only the final 2 to 5 tokens plus `<|im_start|>assistant\n` need evaluation, cutting time-to-first-token (TTFT) from `~220ms` down to `18 to 35ms`!

### 3.3 Model-Specific Thinking Control & Dual-Stream Splitter (`src/router/stream_splitter.rs`)

- **Template Nuance**:
  - **Qwen 3.5** models generate `<think>...</think>` by default and ignore `/nothink` user-text tags; you MUST render the Jinja/chat template with `enable_thinking = false` (which pre-fills `<think>\n\n</think>\n\n` in the assistant header!) for Tier 1 low-latency conversational replies, and `enable_thinking = true` for DAG compilation and code repair.
  - **Qwen 3.6** and **Gemma 4** support explicit per-request thinking toggles; when `ThinkingPolicy::BoundedTokens(B)` is set, track `thought_token_count` inside the sampler loop and force-inject the token sequence for `\n</think>\n` if `thought_token_count == B` so a local model never spins in an infinite reasoning loop!
- **Chunk-Boundary State Machine**: Because BPE tokens can split or straddle `<think>` and `</think>` across arbitrary character slices, implement a 4-state byte holdback buffer (`PreThink`, `InThink`, `ClosingTagHoldback`, `PostThink`) holding back at most `8` bytes when a partial prefix of `</think>` appears at the end of a token piece.

---

## 4. `llguidance` (`1.9.1`) Grammar-Constrained DAG Synthesis (`src/backend/grammar.rs`)

### 4.1 Discriminated-Union Capability Schema Compilation

A naive JSON schema that permits `"capability": {"type": "string"}` and `"parameters": {"type": "object"}` still lets a 2B to 4B SLM pair `"workspace.read@v1"` with parameters belonging to `"process.exec@v1"`. Instead, `dag_schema.rs` dynamically compiles the active `CapabilityRegistry` into a **strict discriminated union (`oneOf`)**:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["objective_id", "tasks"],
  "additionalProperties": false,
  "properties": {
    "objective_id": { "type": "string", "pattern": "^obj_[0-9A-HJKMNP-TV-Z]{26}$" },
    "tasks": {
      "type": "array",
      "minItems": 1,
      "maxItems": 32,
      "items": {
        "oneOf": [
          {
            "type": "object",
            "required": ["step_key", "task_type", "capability", "operation", "parameters", "dependencies", "timeout_ms", "is_idempotent"],
            "additionalProperties": false,
            "properties": {
              "step_key": { "type": "string", "pattern": "^[a-z][a-z0-9_]{1,31}$" },
              "task_type": { "const": "AtomicExecution" },
              "capability": { "const": "workspace.read@v1" },
              "operation": { "enum": ["read_range", "stat_digest"] },
              "parameters": { "$ref": "#/$defs/workspace_read_params" },
              "dependencies": { "type": "array", "items": { "type": "string" } },
              "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 600000 },
              "is_idempotent": { "const": true }
            }
          }
        ]
      }
    }
  }
}
```

### 4.2 Asynchronous Mask Computation and Safe Cancellation

Per `llguidance 1.9.1` contracts:
1. Build the `TokTrie` **once** when loading the model vocabulary (~3ms startup cost) and share `Arc<ParserFactory>` across requests.
2. Before moving a `llguidance::Matcher` onto the inference worker thread, call `let matcher = matcher.into_cancellable();` and extract `let cancel_handle = matcher.cancellation_handle().unwrap();`.
3. Spawn a lightweight watcher waiting on `req.cancel_token.cancelled()` (triggered by user barge-in or `EPOCH_INVALIDATED`) that invokes `cancel_handle.cancel()`. **Safety Rule**: Always `join()` the inference worker before dropping `matcher`, and treat `StopReason::Cancelled` (when `compute_mask()` returns empty) as an immediate clean abort rather than forcing EOS.
4. **Zero-Latency Overlap**: Call `matcher.compute_mask()` on a CPU worker *simultaneously* while the GPU/NPU executes `llama_decode()` for the current token! By the time logits arrive from the accelerator (`5 to 25ms`), the bitmask is already waiting in L1 cache (`< 40us` compute time), adding **zero microseconds** to the critical path! Also apply `matcher.compute_ff_tokens()` (fast-forward deterministic tokens like `{"capability":"workspace.` boilerplate) directly into the next decode batch to boost structured JSON generation speed by **2.1x to 2.8x**!

---

## 5. Multimodal Vision Grounding: Solving High-DPI and Obfuscated Screens (`eon-vision`)

2025 to 2026 computer-use benchmarks (`ScreenSpot-Pro` at 4K resolution and `OSWorld-G`) proved a critical failure mode: feeding a full 4K (`3840x2160`) or tall mobile screenshot resized down to a VLM's patch grid crushes 14px IDE icons and toolbar buttons into 2x2 pixel blurs, dropping single-pass grounding accuracy below 25%. Meanwhile, top 2026 systems (`UI-Venus-1.5 + ZoomIn` at 74.8%, `MEGA-GUI` at 73.2%) solve this via **Coarse-to-Fine Iterative Crop (`+ZoomIn`)**.

### 5.1 EON's 3-Stage Grounding Cascade (`src/grounding/`)

When Ciel needs to click or inspect a UI element for target description $Q$:

1. **Stage 1: Zero-Cost Structural Tree (< 3ms)**:
   - Query Android `AccessibilityNodeInfo` (`clients/android`), Linux `AT-SPI2` (`atspi` crate), or Windows `UIAutomation` (`windows` crate).
   - If non-empty interactive bounds match $Q$ with confidence $\ge 0.85$ and the window is not a Canvas/WebView/Flutter surface (`io.flutter.view.FlutterView`, `android.webkit.WebView`, `SurfaceView`), dispatch directly to the structural node ID.
2. **Stage 2: Coarse Region Proposal + High-Res `ZoomIn` (< 18ms)**:
   - On desktop monitors (`width > 1920` or `height > 1200`) or dense mobile screens (> 60 detected boxes), divide the screen into overlapping $3 \times 2$ quadrants (or run Gemma 4's low-cost `70-token` coarse image budget to identify the target quadrant `[qx, qy]`), then crop the raw native-resolution framebuffer around that quadrant at **1:1 native pixel scale**!
3. **Stage 3: OmniParser-v2 + Collision-Free Set-of-Mark (`< 28ms`)**:
   - Run `omniparser_yolo.onnx` (`ort 2.0`, FP16/INT8) + `omniparser_ocr.onnx` on the native crop.
   - Merge overlapping OCR text boxes and icon boxes via class-aware IoU (`IoU > 0.65`).
   - **Collision-Free Badge Renderer (`som_renderer.rs`)**: Drawing `[12]` directly on top of a tiny 16x16 icon occludes the icon itself so the VLM cannot see what `[12]` points to! Instead, place the high-contrast `[mark_id]` pill (`#000000` background, `#00FFCC` 1px border, bold monospace white digits) at the **exterior corner** (`top-left` above box, falling back to `top-right`, `bottom-left`, `bottom-right` chosen by minimizing overlap with adjacent `UiBoundingBox` rectangles) with a 2px leader tick touching the box corner.
   - Feed the marked crop to `Gemma-4-E2B` / `Gemma-4-12B` (`280-token` image budget) constrained by `llguidance` to `{"mark_id": <uint>, "confidence": <0..1>}`, then map `mark_id` back through the crop offset transform to exact full-screen physical coordinates $(x_{\text{phys}}, y_{\text{phys}})$.

---

## 6. Hybrid Memory Retrieval (RRF) and `MemoryGovernor`

### 6.1 Reciprocal Rank Fusion over FTS5 and `sqlite-vec` (`src/memory/hybrid_rag.rs`)

Raw BM25 scores in SQLite FTS5 are unbounded negative numbers (`bm25()` returns negative values where smaller is better) whereas cosine similarity lies in `[-1.0, 1.0]`. Adding `0.5 * BM25 + 0.5 * Cosine` directly produces uncalibrated garbage! Implement **Reciprocal Rank Fusion (RRF)** with constant $k = 60$:

$$\text{RRF}(d) = \frac{w_{\text{lex}}}{k + \text{rank}_{\text{FTS5}}(d)} + \frac{w_{\text{vec}}}{k + \text{rank}_{\text{vec0}}(d)} + \beta_{\text{recency}}(d)$$

where $w_{\text{lex}} = 1.0$, $w_{\text{vec}} = 1.2$, and $\beta_{\text{recency}}(d) = 0.15 \cdot e^{-\Delta t / 7\text{d}}$. Store embeddings in `sqlite-vec` using **scalar int8 quantization** (`vec_int8` in `vec0`), shrinking 384-dim vectors from `1,536 bytes` to `384 bytes` (4x smaller cache footprint, SIMD `sdot` accelerated on ARMv8.2+ and AVX2!).

### 6.2 Three-Tier `MemoryGovernor` (`src/memory/governor.rs`)

```rust
pub struct MemoryGovernor {
    tier1_pinned: Vec<PinnedMmap>, // Silero VAD + WeSpeaker + 5 Ciel Audio Bridges (<12 MB, mlock)
    tier2_warm: Vec<SharedMmap>,   // Zipformer 2023-06-26 + Kokoro/PocketTTS + BGE (~260-380 MB)
    tier3_slm: Mutex<Option<SlmMapping>>, // Gemma-4-E2B / Qwen3.5 GGUF mmap
}
```
- **Linux / Android `mlock` Limit**: Unprivileged Android processes often have `RLIMIT_MEMLOCK` set to `64 KB` or `64 MB`. Wrap `libc::mlock` in a graceful check (`getrlimit(RLIMIT_MEMLOCK)`); if `RLIMIT_MEMLOCK < tier1_bytes`, pin Silero VAD (`2.1 MB`) first and fall back to periodic cache-touching for WeSpeaker without panicking!
- **Dormant & OS Trim Eviction**: Expose `EonKernelBridge::on_system_memory_trim(level: u32)` over UniFFI, wired to Android `Application.onTrimMemory(TRIM_MEMORY_RUNNING_LOW | TRIM_MEMORY_BACKGROUND)`. On trim or transition into `DORMANT`:
  1. Flush active KV-cache slot `1` (keep compressed system prefix snapshot on disk).
  2. Call `libc::madvise(ptr, len, libc::MADV_DONTNEED)` on Linux/Android (or `DiscardVirtualMemory` / `VirtualUnlock` on Windows) across the Tier 3 GGUF mapping, dropping resident RSS below `45 MB` in `< 1.5 ms`.
