//! Local input meter. The driver callback exports one peak, never a buffer of PCM.

use std::fmt;
use std::time::{Duration, Instant};

const HISTORY: usize = 48;
const STALL: Duration = Duration::from_secs(3);

trait Input {
    /// `None` means no callback, distinct from a callback reporting silence.
    fn take_peak(&self) -> Result<Option<u8>, &'static str>;
}

#[derive(Default)]
pub(super) struct Meter {
    input: Option<Box<dyn Input>>,
    last_callback: Option<Instant>,
    message: Option<&'static str>,
    history: Vec<u64>,
}

impl fmt::Debug for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Meter")
            .field("active", &self.active())
            .finish()
    }
}

impl Meter {
    pub(super) fn active(&self) -> bool {
        self.input.is_some()
    }

    pub(super) fn stop(&mut self) {
        *self = Self::default();
    }

    fn toggle_with(
        &mut self,
        now: Instant,
        open: impl FnOnce() -> Result<Box<dyn Input>, &'static str>,
    ) {
        let was_active = self.active();
        self.stop();
        if !was_active {
            match open() {
                Ok(input) => {
                    self.input = Some(input);
                    self.last_callback = Some(now);
                }
                Err(message) => self.message = Some(message),
            }
        }
    }

    pub(super) fn toggle(&mut self) {
        #[cfg(feature = "audio-input")]
        self.toggle_with(Instant::now(), device::open);
        #[cfg(not(feature = "audio-input"))]
        self.toggle_with(Instant::now(), || {
            Err("Audio input support is not built in.")
        });
    }

    pub(super) fn poll(&mut self, now: Instant) {
        let Some(input) = &self.input else { return };
        let error = match input.take_peak() {
            Ok(Some(peak)) => {
                self.last_callback = Some(now);
                if self.history.len() == HISTORY {
                    self.history.remove(0);
                }
                self.history.push(u64::from(peak));
                return;
            }
            Ok(None)
                if self
                    .last_callback
                    .is_some_and(|last| now.duration_since(last) >= STALL) =>
            {
                "Input stopped delivering samples."
            }
            Ok(None) => return,
            Err(message) => message,
        };
        self.stop();
        self.message = Some(error);
    }

    pub(super) fn status(&self) -> String {
        if self.active() {
            let level = self.history.last().map_or_else(
                || "waiting for samples".into(),
                |peak| format!("peak {peak}%"),
            );
            format!("Input ON: {level}. Press m to stop or leave this view.")
        } else {
            let why = self.message.unwrap_or("Input OFF.");
            if cfg!(feature = "audio-input") {
                format!("{why} Press m to meter the default microphone locally.")
            } else {
                format!("{why} Rebuild with --features audio-input to meter a microphone.")
            }
        }
    }

    pub(super) fn peaks(&self) -> &[u64] {
        &self.history
    }
}

#[cfg(feature = "audio-input")]
mod device {
    use super::Input;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, Stream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    #[derive(Default)]
    struct Callback {
        // 0 = no samples since the last poll; 1..=101 = observed 0..=100%.
        level: AtomicU32,
        failed: AtomicBool,
    }

    struct DeviceInput {
        _stream: Stream,
        callback: Arc<Callback>,
    }

    impl Input for DeviceInput {
        fn take_peak(&self) -> Result<Option<u8>, &'static str> {
            if self.callback.failed.load(Ordering::Acquire) {
                return Err("The input device reported an error.");
            }
            Ok(self
                .callback
                .level
                .swap(0, Ordering::AcqRel)
                .checked_sub(1)
                .map(|p| p as u8))
        }
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        callback: &Arc<Callback>,
    ) -> Result<Stream, &'static str>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let (data, error) = (callback.clone(), callback.clone());
        device
            .build_input_stream::<T, _, _>(
                config,
                move |samples: &[T], _| {
                    if !samples.is_empty() {
                        let peak = samples
                            .iter()
                            .fold(0.0_f32, |p, s| p.max(s.to_sample::<f32>().abs()));
                        data.level.fetch_max(
                            1 + (peak.min(1.0) * 100.0).round() as u32,
                            Ordering::Release,
                        );
                    }
                },
                move |_| error.failed.store(true, Ordering::Release),
                None,
            )
            .map_err(|_| "Cannot open the default microphone.")
    }

    pub(super) fn open() -> Result<Box<dyn Input>, &'static str> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("No default microphone.")?;
        let config = device
            .default_input_config()
            .map_err(|_| "No available input configuration.")?;
        let callback = Arc::new(Callback::default());
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config.config(), &callback),
            SampleFormat::I16 => build::<i16>(&device, config.config(), &callback),
            SampleFormat::I32 => build::<i32>(&device, config.config(), &callback),
            SampleFormat::U16 => build::<u16>(&device, config.config(), &callback),
            _ => Err("Unsupported microphone sample format."),
        }?;
        stream
            .play()
            .map_err(|_| "Cannot start the default microphone.")?;
        Ok(Box::new(DeviceInput {
            _stream: stream,
            callback,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Counter(Rc<Cell<bool>>);

    impl Input for Counter {
        fn take_peak(&self) -> Result<Option<u8>, &'static str> {
            Ok(self.0.get().then_some(0))
        }
    }

    #[test]
    fn silence_is_live_but_stalled_input_closes_and_erases_its_history() {
        let now = Instant::now();
        let alive = Rc::new(Cell::new(true));
        let mut meter = Meter::default();
        meter.toggle_with(now, || Ok(Box::new(Counter(alive.clone()))));
        for _ in 0..(2 * HISTORY) {
            meter.poll(now);
        }
        assert_eq!(meter.peaks(), &[0; HISTORY]);
        assert!(meter.status().contains("peak 0%"));
        alive.set(false);
        meter.poll(now + STALL);
        assert!(!meter.active() && meter.peaks().is_empty());
        assert!(meter.status().contains("stopped delivering samples"));
        assert_eq!(Rc::strong_count(&alive), 1);
    }

    #[cfg(feature = "audio-input")]
    #[test]
    #[ignore = "requires an explicitly configured virtual default input playing a 50% peak tone"]
    fn opt_in_virtual_input_reports_half_scale_then_releases_device() {
        assert_eq!(
            std::env::var("EON_TEST_VIRTUAL_MIC").as_deref(),
            Ok("1"),
            "set EON_TEST_VIRTUAL_MIC=1 only in a dedicated virtual audio session"
        );
        let mut meter = Meter::default();
        meter.toggle();
        assert!(meter.active(), "{}", meter.status());
        let start = Instant::now();
        while !meter.peaks().iter().any(|p| (48..=52).contains(p)) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "{}",
                meter.status()
            );
            std::thread::sleep(Duration::from_millis(25));
            meter.poll(Instant::now());
        }
        meter.toggle();
        assert!(!meter.active() && meter.peaks().is_empty());
    }

    #[test]
    fn navigation_completion_and_quit_never_leave_the_input_running() {
        use super::super::{Page, WatchState};
        use crate::render_watch_snapshot;
        let mut state = WatchState::default();
        for key in "/a".chars() {
            state.on_key(key);
        }
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.page, Page::Audio);
        assert!(
            render_watch_snapshot(&state, 90, 12)
                .unwrap()
                .contains("Input OFF.")
        );

        let retained = Rc::new(Cell::new(true));
        for exit in [KeyCode::Esc, KeyCode::Char('q')] {
            state.open(Page::Audio);
            state
                .meter
                .toggle_with(Instant::now(), || Ok(Box::new(Counter(retained.clone()))));
            state.meter.poll(Instant::now());
            assert!(
                render_watch_snapshot(&state, 90, 12)
                    .unwrap()
                    .contains("Input ON: peak 0%")
            );
            state.handle_key(press(exit));
            assert_eq!(Rc::strong_count(&retained), 1);
            assert!(state.meter.peaks().is_empty());
        }
    }
}
