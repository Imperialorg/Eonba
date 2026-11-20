//! Opt-in, local PP-OCRv4 for **already authorized** JPEG pixels. Horizontal
//! text regions in raster coordinates, not accessibility selectors, widget
//! classifications or action targets. No capture, model downloads or subprocesses.

mod detection;

use std::io::Cursor;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits, Rgb, RgbImage,
    imageops,
};
use ort::session::{RunOptions, Session};
use ort::value::Tensor;
use sha2::{Digest, Sha256};

use crate::{EonError, EonResult};

const MAX_JPEG: usize = 8 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_TIME: Duration = Duration::from_secs(10);
const MAX_CANDIDATES: usize = 64;

// RECORD hashes in the PyPI-authenticated rapidocr-onnxruntime 1.4.4 wheel.
const DET_SHA256: &str = "d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9";
const REC_SHA256: &str = "48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b";

/// Half-open bounds in the stored JPEG's raster (no EXIF orientation transform).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBounds {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    /// Untrusted visible text. Not executable and not evidence of clickability.
    pub text: String,
    /// Mean greedy CTC probability of the emitted (nonblank) characters.
    pub confidence: f32,
    pub bounds: PixelBounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrPage {
    pub width: u32,
    pub height: u32,
    /// Top-to-bottom rows, left-to-right within each row. May be empty.
    pub lines: Vec<OcrLine>,
}

fn failed(reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "PaddleOCR".into(),
        reason: reason.to_string(),
    }
}

fn limited(resource: &str, requested: u64, available: u64) -> EonError {
    EonError::ResourceExhausted {
        resource: resource.into(),
        requested,
        available,
    }
}

fn deadline(budget: Duration) -> EonResult<Instant> {
    if budget.is_zero() || budget > MAX_TIME {
        return Err(EonError::ValidationError(
            "OCR budget must be in (0, 10s]".into(),
        ));
    }
    Ok(Instant::now() + budget)
}

fn check_time(end: Instant) -> EonResult<()> {
    if Instant::now() >= end {
        Err(failed("frame deadline exceeded"))
    } else {
        Ok(())
    }
}

fn decode(bytes: &[u8], expected: Option<(u32, u32)>) -> EonResult<RgbImage> {
    if bytes.len() > MAX_JPEG {
        return Err(limited(
            "OCR JPEG bytes",
            bytes.len() as u64,
            MAX_JPEG as u64,
        ));
    }
    if !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return Err(EonError::ValidationError(
            "expected one complete JPEG".into(),
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(failed)?;
    let (w, h) = decoder.dimensions();
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_PIXELS {
        return Err(limited("OCR pixels", pixels, MAX_PIXELS));
    }
    if w == 0 || h == 0 || expected.is_some_and(|dims| dims != (w, h)) {
        return Err(EonError::ValidationError(
            "invalid or mismatched frame dimensions".into(),
        ));
    }
    Ok(DynamicImage::from_decoder(decoder)
        .map_err(failed)?
        .into_rgb8())
}

/// Paddle v4 consumes BGR NCHW, normalized to [-1, 1]; the recognizer
/// expects right padding with zero **after** normalization.
fn input(
    image: &impl GenericImageView<Pixel = Rgb<u8>>,
    w: u32,
    h: u32,
    stride: u32,
) -> EonResult<Tensor<f32>> {
    let scaled = imageops::resize(image, w, h, imageops::FilterType::Triangle);
    let plane = (h * stride) as usize;
    let mut data = vec![0.0; 3 * plane];
    for (x, y, rgb) in scaled.enumerate_pixels() {
        let off = (y * stride + x) as usize;
        for ch in 0..3 {
            data[ch * plane + off] = f32::from(rgb[2 - ch]) / 127.5 - 1.0;
        }
    }
    Tensor::from_array(([1usize, 3, h as usize, stride as usize], data)).map_err(failed)
}

pub struct PaddleOcr {
    detector: Session,
    recognizer: Session,
    alphabet: Vec<String>,
}

impl PaddleOcr {
    /// Verify both graphs **before** passing either to the native runtime. The
    /// character dictionary is metadata of the pinned recognizer, not a local
    /// config, vocabulary file, plugin or network dependency.
    pub fn from_models(detector: &[u8], recognizer: &[u8]) -> EonResult<Self> {
        for (bytes, len, digest) in [
            (detector, 4_745_517, DET_SHA256),
            (recognizer, 10_857_958, REC_SHA256),
        ] {
            if bytes.len() != len || format!("{:x}", Sha256::digest(bytes)) != digest {
                return Err(EonError::ValidationError(
                    "PP-OCRv4 model digest mismatch".into(),
                ));
            }
        }
        let env = ort::init()
            .with_name("eon-vision")
            .with_telemetry(false)
            .build()
            .map_err(failed)?;
        let load = |graph| {
            Session::builder(&env)
                .map_err(failed)?
                .with_intra_threads(1)
                .map_err(failed)?
                .with_inter_threads(1)
                .map_err(failed)?
                // Caller-controlled crop sizes vary between frames. Don't keep
                // per-shape allocation plans or a CPU arena at its high-water mark.
                .with_memory_pattern(false)
                .map_err(failed)?
                .with_execution_providers([ort::ep::CPU::default()
                    .with_arena_allocator(false)
                    .build()])
                .map_err(failed)?
                .commit_from_memory(graph)
                .map_err(failed)
        };
        let detector = load(detector)?;
        let recognizer = load(recognizer)?;
        let dictionary = recognizer
            .metadata()
            .map_err(failed)?
            .custom("character")
            .ok_or_else(|| failed("missing PP-OCRv4 character metadata"))?;
        // Index zero is the CTC blank; space follows the embedded dictionary.
        let mut alphabet = vec![String::new()];
        alphabet.extend(dictionary.lines().map(str::to_owned));
        alphabet.push(" ".into());
        Ok(Self {
            detector,
            recognizer,
            alphabet,
        })
    }

    /// Ground one caller-supplied still. JPEG bytes are never interpreted as a
    /// filename. `budget` covers this frame, including all its recognizer runs.
    /// ORT termination is cooperative (at kernel boundaries), not a process kill.
    pub fn recognize_jpeg(&mut self, jpeg: &[u8], budget: Duration) -> EonResult<OcrPage> {
        self.recognize(jpeg, None, deadline(budget)?)
    }

    /// Ground only the sealed descriptor already selected by its capture owner.
    /// Does not query history, select a newer frame or hold a producer lock.
    #[cfg(target_os = "linux")]
    pub fn recognize_selected(
        &mut self,
        frame: &crate::VideoKeyframe<crate::shared::SharedJpeg>,
        budget: Duration,
    ) -> EonResult<OcrPage> {
        let end = deadline(budget)?;
        let jpeg = frame.jpeg_bytes.read_limited(MAX_JPEG)?;
        self.recognize(&jpeg, Some((frame.width, frame.height)), end)
    }

    fn recognize(
        &mut self,
        bytes: &[u8],
        expected: Option<(u32, u32)>,
        end: Instant,
    ) -> EonResult<OcrPage> {
        check_time(end)?;
        let image = decode(bytes, expected)?;
        check_time(end)?;
        let options = RunOptions::new().map_err(failed)?;
        std::thread::scope(|scope| {
            let (done, rx) = channel::<()>();
            let opts = &options;
            std::thread::Builder::new()
                .name("eon-ocr-deadline".into())
                .spawn_scoped(scope, move || {
                    if rx.recv_timeout(end.saturating_duration_since(Instant::now()))
                        == Err(RecvTimeoutError::Timeout)
                    {
                        let _ = opts.terminate();
                    }
                })
                .map_err(failed)?;
            let result = self.infer(&image, &options, end);
            drop(done); // wake and join even on inference errors
            check_time(end)?;
            result
        })
    }

    fn infer(&mut self, image: &RgbImage, opts: &RunOptions, end: Instant) -> EonResult<OcrPage> {
        let (width, height) = image.dimensions();
        let scale = (960.0 / width.max(height) as f32).min(1.0);
        let aligned = |side| (((side as f32 * scale / 32.0).round() as u32).max(1)) * 32;
        let (dw, dh) = (aligned(width), aligned(height));
        let regions = {
            let tensor = input(image, dw, dh, dw)?;
            check_time(end)?;
            let out = self
                .detector
                .run_with_options(ort::inputs![tensor], opts)
                .map_err(failed)?;
            let (shape, scores) = out[0].try_extract_tensor::<f32>().map_err(failed)?;
            if **shape != [1, 1, i64::from(dh), i64::from(dw)] {
                return Err(failed("unexpected text probability map dimensions"));
            }
            detection::regions(scores, dw, dh, width, height)?
        };
        let mut lines = Vec::new();
        for bounds in regions {
            check_time(end)?;
            let (w, h) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
            let crop = image.view(bounds.left, bounds.top, w, h);
            let rw = (48 * w).div_ceil(h).clamp(1, 960);
            let tensor = input(&*crop, rw, 48, rw.max(320))?;
            let out = self
                .recognizer
                .run_with_options(ort::inputs![tensor], opts)
                .map_err(failed)?;
            let (shape, logits) = out[0].try_extract_tensor::<f32>().map_err(failed)?;
            if shape.len() != 3 || shape[0] != 1 || shape[2] != self.alphabet.len() as i64 {
                return Err(failed(
                    "recognizer shape disagrees with character dictionary",
                ));
            }
            if let Some((text, confidence)) = ctc(logits, &self.alphabet) {
                lines.push(OcrLine {
                    text,
                    confidence,
                    bounds,
                });
            }
        }
        Ok(OcrPage {
            width,
            height,
            lines,
        })
    }
}

fn ctc(scores: &[f32], alphabet: &[String]) -> Option<(String, f32)> {
    let mut text = String::new();
    let (mut prev, mut count, mut total) = (0, 0u32, 0.0);
    for step in scores.chunks_exact(alphabet.len()) {
        let (index, &p) = step.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
        if index != 0 && prev != index {
            text.push_str(&alphabet[index]);
            total += p;
            count += 1;
        }
        prev = index;
    }
    let confidence = total / count.max(1) as f32;
    let text = text.trim();
    (!text.is_empty() && (0.6..=1.0).contains(&confidence)).then(|| (text.to_owned(), confidence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_oversized_pixels_from_the_jpeg_header_before_decompression() {
        let mut bytes = include_bytes!("../tests/fixtures/settings.jpg").to_vec();
        let sof = bytes.windows(2).position(|m| m == [0xff, 0xc0]).unwrap();
        // SOF0 length, precision, then two big-endian u16 raster dimensions.
        bytes[sof + 5..sof + 9].copy_from_slice(&[0x0c, 0x00, 0x0c, 0x00]);
        assert!(matches!(
            decode(&bytes, None),
            Err(EonError::ResourceExhausted { .. })
        ));
        bytes[sof + 5..sof + 9].copy_from_slice(&[0x00, 0x08, 0xff, 0xff]);
        assert!(decode(&bytes, None).is_err());
    }

    #[test]
    fn non_image_input_and_excess_bytes_are_not_processed() {
        assert!(decode(b"file:///etc/hosts", None).is_err());
        assert!(decode(&[0xff, 0xd8, 0xff, 0xd9], None).is_err());
        assert!(matches!(
            decode(&vec![0; MAX_JPEG + 1], None),
            Err(EonError::ResourceExhausted { .. })
        ));
        assert!(deadline(Duration::ZERO).is_err());
        assert!(deadline(MAX_TIME + Duration::from_millis(1)).is_err());
    }

    #[test]
    fn ctc_collapses_repeats_but_not_across_blank_and_drops_low_confidence() {
        let alphabet = ["", "A", "B"].map(String::from);
        assert_eq!(
            ctc(&[0., 1., 0., 0., 1., 0., 1., 0., 0., 0., 1., 0.], &alphabet),
            Some(("AA".into(), 1.0))
        );
        assert!(ctc(&[0.3, 0.5, 0.2], &alphabet).is_none());
    }
}
