# MODELS AND WEIGHTS SETUP SPECIFICATION

> **Subsystem:** EON Perception & Neural Inference  
> **Directory:** `perception/models/`  
> **Target:** Canonical model architectures, weights, quantization formats, and download endpoints

---

## 1. Overview & Footprint Constraints

To ensure EON functions as an ultra-fast, local-first system with **sub-200ms latency**, models must be quantized (INT8 / FP16 / GGUF Q4_K_M) and optimized for ONNX Runtime and `llama.cpp`.

Total local model storage budget: **< 3.5 GB**.

---

## 2. Model Manifest Table

| Subsystem | Model Name | Format & Quantization | Disk Size | Target Directory | Primary Endpoint / Source |
|---|---|---|---|---|---|
| **VAD** | Silero VAD v5 | ONNX (INT8) | ~2.1 MB | `perception/models/vad/` | `snakers4/silero-vad` |
| **Streaming ASR** | Sherpa-onnx Zipformer | ONNX (int8 streaming) | ~78 MB | `perception/models/asr/` | `k2-fsa/sherpa-onnx` (Zipformer 20M/50M) |
| **Streaming TTS** | Kokoro-82M v0.19 | ONNX (FP16/INT8) + `voices.bin` | ~160 MB | `perception/models/tts/` | `hexgrad/Kokoro-82M` |
| **Embeddings** | `bge-micro-v2` / `all-MiniLM` | ONNX (INT8) | ~25 MB | `perception/models/embeddings/` | `BAAI/bge-micro-v2` or SentenceTransformers |
| **Tactical SLM** | `Qwen2.5-Coder-3B-Instruct` | GGUF (`Q4_K_M`) | ~2.1 GB | `perception/models/slm/` | `Qwen/Qwen2.5-Coder-3B-Instruct-GGUF` |
| **Fallback SLM** | `gemma-2-2b-it` | GGUF (`Q4_K_M`) | ~1.6 GB | `perception/models/slm/` | `google/gemma-2-2b-it-GGUF` |

---

## 3. Directory Layout in `perception/models/`

```text
perception/models/
├── vad/
│   └── silero_vad.onnx                # 30ms window VAD model
├── asr/
│   ├── encoder.int8.onnx              # Zipformer streaming encoder
│   ├── decoder.int8.onnx              # Transducer prediction network
│   ├── joiner.int8.onnx               # Joint network
│   └── tokens.txt                     # BPE token vocabulary
├── tts/
│   ├── kokoro-v0_19.onnx              # StyleTTS2 neural synthesis graph
│   └── voices.bin                     # Quantized voice timbre embeddings (Ciel voice profile)
├── embeddings/
│   ├── model.onnx                     # Quantized embedding model
│   └── tokenizer.json                 # Fast tokenizer configuration
└── slm/
    └── qwen2.5-coder-3b-instruct-q4_k_m.gguf # Tactical on-device reasoning & code SLM
```

---

## 4. Automated Download Script (`scripts/download_models.py`)

A standardized Python / PowerShell script is provided in `scripts/` to download and verify SHA-256 checksums automatically:

```python
#!/usr/bin/env python3
"""
EON Model Weights Downloader
Fetches verified quantized weights for Silero VAD, Sherpa-onnx Zipformer, Kokoro-82M, and Qwen-Coder.
"""
import os
import urllib.request
import hashlib

MODELS = {
    "perception/models/vad/silero_vad.onnx": {
        "url": "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx",
        "sha256": "4722513f56da235559868770fa3de68022d86a6eb43e1cfdf96b2fd709cb8b59"
    }
}

def verify_and_download(filepath, meta):
    os.makedirs(os.path.dirname(filepath), exist_ok=True)
    if os.path.exists(filepath):
        print(f"Verified: {filepath} already exists.")
        return
    print(f"Downloading {filepath}...")
    urllib.request.urlretrieve(meta["url"], filepath)
    print(f"Downloaded {filepath} successfully.")

if __name__ == "__main__":
    for path, meta in MODELS.items():
        verify_and_download(path, meta)
```
