//! Helpers shared by the voice e2e tests.

use std::path::Path;

/// Reads a 16 kHz mono 16-bit PCM WAV into floats in [-1, 1]. Any other format is an error, not a silent resample.
pub fn read_pcm16_mono_16k(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("fixture must exist");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..pos + 8 + size];
        if id == b"fmt " {
            let audio_format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes([body[14], body[15]]);
            format_ok = audio_format == 1 && channels == 1 && rate == 16_000 && bits == 16;
        }
        if id == b"data" {
            assert!(format_ok, "fixture must be 16 kHz mono PCM16");
            return body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                .collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}
