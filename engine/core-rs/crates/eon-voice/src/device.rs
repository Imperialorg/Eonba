//! Device audio capture and playback over cpal (docs/implementation/04, device loop).
//!
//! Capture runs in the host's callback thread: it downmixes to mono and pushes into a `PcmProducer`. Playback pulls
//! from a `PcmConsumer` and fans mono out to every output channel. Neither side locks or allocates after the first
//! callback, because the scratch buffer is reserved up front. An empty ring plays silence and counts an underrun.
//! Overflow and driver errors are counted, never silently dropped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream};
use eon_primitives::{EonError, EonResult};

use crate::barge_in::BargeIn;
use crate::ring::{PcmConsumer, PcmProducer};

const SCRATCH_FRAMES: usize = 16_384;
/// Playback buffer in frames. The backend default makes PulseAudio's ALSA plugin fail `snd_pcm_avail_delay` with I/O error
/// 5 about a third of the time at stream start, so playback asks for an explicit size.
const PLAYBACK_BUFFER_FRAMES: u32 = 4096;

/// Counters shared between the device callbacks and the owner.
#[derive(Debug, Default)]
pub struct DeviceCounters {
    pub callbacks: AtomicU64,
    pub frames: AtomicU64,
    pub underruns: AtomicU64,
    pub driver_errors: AtomicU64,
    pub barge_in_flushes: AtomicU64,
    pub flushed_samples: AtomicU64,
}

fn device_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("audio.{operation}"),
        reason: reason.to_string(),
    }
}

pub struct CaptureHandle {
    _stream: Stream,
    pub sample_rate: u32,
    pub counters: Arc<DeviceCounters>,
}

pub struct PlaybackHandle {
    _stream: Stream,
    pub sample_rate: u32,
    pub counters: Arc<DeviceCounters>,
}

fn build_capture<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut producer: PcmProducer,
    counters: Arc<DeviceCounters>,
) -> EonResult<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels());
    let stream_config = config.config();
    let data_counters = Arc::clone(&counters);
    let error_counters = Arc::clone(&counters);
    let mut mono: Vec<f32> = Vec::with_capacity(SCRATCH_FRAMES);
    device
        .build_input_stream::<T, _, _>(
            stream_config,
            move |data: &[T], _| {
                data_counters.callbacks.fetch_add(1, Ordering::Relaxed);
                mono.clear();
                for frame in data.chunks_exact(channels) {
                    let sum: f32 = frame.iter().map(|s| s.to_sample::<f32>()).sum();
                    mono.push(sum / channels as f32);
                }
                let written = producer.push_slice(&mono);
                data_counters.frames.fetch_add(written as u64, Ordering::Relaxed);
            },
            move |_err| {
                error_counters.driver_errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| device_err("capture.build", e))
}

/// Opens the default input device and streams mono PCM into `producer` until the handle is dropped.
pub fn capture_default_input(producer: PcmProducer) -> EonResult<CaptureHandle> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| device_err("capture.device", "no default input device"))?;
    let config = device
        .default_input_config()
        .map_err(|e| device_err("capture.config", e))?;
    let counters = Arc::new(DeviceCounters::default());
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_capture::<f32>(&device, &config, producer, Arc::clone(&counters))?,
        SampleFormat::I16 => build_capture::<i16>(&device, &config, producer, Arc::clone(&counters))?,
        other => return Err(device_err("capture.format", format!("unsupported sample format {other:?}"))),
    };
    stream.play().map_err(|e| device_err("capture.play", e))?;
    Ok(CaptureHandle {
        sample_rate: config.sample_rate(),
        counters,
        _stream: stream,
    })
}

fn build_playback<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut consumer: PcmConsumer,
    counters: Arc<DeviceCounters>,
    barge: BargeIn,
) -> EonResult<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels());
    let mut stream_config = config.config();
    stream_config.buffer_size = cpal::BufferSize::Fixed(PLAYBACK_BUFFER_FRAMES);
    let data_counters = Arc::clone(&counters);
    let error_counters = Arc::clone(&counters);
    let mut mono: Vec<f32> = vec![0.0; SCRATCH_FRAMES];
    device
        .build_output_stream::<T, _, _>(
            stream_config,
            move |out: &mut [T], _| {
                data_counters.callbacks.fetch_add(1, Ordering::Relaxed);
                let frames = out.len() / channels;
                let mut got = 0;
                if barge.take() {
                    let dropped = consumer.flush();
                    data_counters.barge_in_flushes.fetch_add(1, Ordering::Relaxed);
                    data_counters.flushed_samples.fetch_add(dropped as u64, Ordering::Relaxed);
                } else {
                    let want = frames.min(mono.len());
                    got = consumer.pop_slice(&mut mono[..want]);
                }
                if got < frames && !barge.is_pending() {
                    data_counters.underruns.fetch_add(1, Ordering::Relaxed);
                }
                for (frame_index, frame) in out.chunks_exact_mut(channels).enumerate() {
                    let sample = if frame_index < got { mono[frame_index] } else { 0.0 };
                    let converted: T = sample.to_sample();
                    frame.iter_mut().for_each(|s| *s = converted);
                }
                data_counters.frames.fetch_add(got as u64, Ordering::Relaxed);
            },
            move |_err| {
                error_counters.driver_errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| device_err("playback.build", e))
}

/// Opens the default output device and plays mono PCM pulled from `consumer` until the handle is dropped. A pending
/// `barge` request flushes the ring and silences the next callback.
pub fn play_default_output(consumer: PcmConsumer, barge: BargeIn) -> EonResult<PlaybackHandle> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| device_err("playback.device", "no default output device"))?;
    let config = device
        .default_output_config()
        .map_err(|e| device_err("playback.config", e))?;
    let counters = Arc::new(DeviceCounters::default());
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_playback::<f32>(&device, &config, consumer, Arc::clone(&counters), barge)?,
        SampleFormat::I16 => build_playback::<i16>(&device, &config, consumer, Arc::clone(&counters), barge)?,
        other => return Err(device_err("playback.format", format!("unsupported sample format {other:?}"))),
    };
    stream.play().map_err(|e| device_err("playback.play", e))?;
    Ok(PlaybackHandle {
        sample_rate: config.sample_rate(),
        counters,
        _stream: stream,
    })
}
