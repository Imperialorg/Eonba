# ADR-0002: Sub-200ms Duplex Voice Pipeline (Streaming STT & TTS)

## Status
Accepted

## Context
Standard voice assistant architectures rely on full-utterance batching:
1. User stops speaking.
2. VAD waits 500ms - 1000ms to confirm silence.
3. Audio WAV is sent to cloud ASR.
4. LLM generates full text response.
5. Full text response is sent to cloud TTS.
6. Audio file is buffered and played.
Total latency: 2,500ms - 6,000ms. This latency destroys the illusion of an omniscient, present companion. Furthermore, when the user speaks while the assistant is speaking, typical systems cannot cleanly barge in without delay.

## Decision
1. **Streaming On-Device ASR**: Deploy `sherpa-onnx` with the int8 streaming Zipformer model, emitting partial tokens every 60ms.
2. **Silero VAD v5**: Evaluate 30ms audio windows on CPU (< 1ms execution). Lower end-of-utterance trailing silence to 250ms dynamically.
3. **Clause Chunking & Token-to-Audio Streaming**: Instead of waiting for full LLM completion, the streaming token output is split on punctuation clauses (e.g. "Affirmative.", "Report: Found 3 issues."). The first clause is immediately dispatched to the TTS engine.
4. **Kokoro-82M Streaming Neural TTS**: Deploy Kokoro-82M ONNX FP16/INT8 with RTF < 0.15, producing initial audio buffers in < 50ms.
5. **Hardware Barge-In**: Silero VAD monitors incoming microphone audio with AEC. If user speech is detected during assistant playback, the audio driver buffer is flushed in < 5ms, cancelling the remaining speech task instantly.

## Consequences
- Time-to-First-Audio (TTFA) drops to **< 200ms** (P99 < 300ms), exceeding human conversational response times.
- Natural, full-duplex conversations with instantaneous interruption capability.
- 100% offline local voice processing protects user privacy and eliminates cloud API latency.

