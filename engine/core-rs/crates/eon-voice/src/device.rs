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
use std::sync::Mutex;
use std::time::{Duration, Instant};
use eon_primitives::{EonError, EonResult};

use crate::barge_in::BargeIn;
use crate::ring::{PcmConsumer, PcmProducer};

const SCRATCH_FRAMES: usize = 16_384;
/// Playback buffer in frames. The backend default makes PulseAudio's ALSA plugin fail `snd_pcm_avail_delay` with I/O error
/// 5 about a third of the time at stream start, so playback asks for an explicit size.
const PLAYBACK_BUFFER_FRAMES: u32 = 4096;
/// Attempts to start playback when the driver fails before its first callback. The consumer is kept across attempts.
const STARTUP_ATTEMPTS: u32 = 3;
const STARTUP_WAIT: Duration = Duration::from_secs(3);
/// Callbacks after which a stream counts as healthy. A driver error before that is a startup failure.
const HEALTHY_CALLBACKS: u64 = 10;

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
    /// Stream rebuilds needed because the driver failed before the first callback.
    pub startup_retries: u32,
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
    consumer: Arc<Mutex<PcmConsumer>>,
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
                // Never block the audio thread. A contended consumer plays silence for this callback.
                if let Ok(mut consumer) = consumer.try_lock() {
                    if barge.take() {
                        let dropped = consumer.flush();
                        data_counters.barge_in_flushes.fetch_add(1, Ordering::Relaxed);
                        data_counters.flushed_samples.fetch_add(dropped as u64, Ordering::Relaxed);
                    } else {
                        let want = frames.min(mono.len());
                        got = consumer.pop_slice(&mut mono[..want]);
                    }
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
/// `barge` request flushes the ring and silences the next callback. A driver error before the first callback is a
/// startup failure: the stream is rebuilt, up to `STARTUP_ATTEMPTS` times, and each rebuild is counted in
/// `startup_retries`. An error after `HEALTHY_CALLBACKS` callbacks is a steady-state error and stays in `driver_errors`.
pub fn play_default_output(consumer: PcmConsumer, barge: BargeIn) -> EonResult<PlaybackHandle> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| device_err("playback.device", "no default output device"))?;
    let config = device
        .default_output_config()
        .map_err(|e| device_err("playback.config", e))?;
    let shared = Arc::new(Mutex::new(consumer));
    let mut startup_retries = 0;
    loop {
        let counters = Arc::new(DeviceCounters::default());
        let stream = match config.sample_format() {
            SampleFormat::F32 => build_playback::<f32>(&device, &config, Arc::clone(&shared), Arc::clone(&counters), barge.clone())?,
            SampleFormat::I16 => build_playback::<i16>(&device, &config, Arc::clone(&shared), Arc::clone(&counters), barge.clone())?,
            other => return Err(device_err("playback.format", format!("unsupported sample format {other:?}"))),
        };
        stream.play().map_err(|e| device_err("playback.play", e))?;
        let started = Instant::now();
        while started.elapsed() < STARTUP_WAIT
            && counters.driver_errors.load(Ordering::Relaxed) == 0
            && counters.callbacks.load(Ordering::Relaxed) < HEALTHY_CALLBACKS
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let failed_at_startup = counters.driver_errors.load(Ordering::Relaxed) > 0;
        if failed_at_startup && startup_retries + 1 < STARTUP_ATTEMPTS {
            drop(stream);
            startup_retries += 1;
            continue;
        }
        return Ok(PlaybackHandle {
            sample_rate: config.sample_rate(),
            counters,
            startup_retries,
            _stream: stream,
        });
    }
}
