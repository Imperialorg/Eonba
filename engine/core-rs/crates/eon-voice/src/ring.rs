//! Wait-free single-producer, single-consumer PCM ring for the audio device callbacks (docs/implementation/04, section 2).
//!
//! The producer runs in the capture callback and the consumer in the render callback or the ASR thread. Neither side
//! takes a lock or allocates. Overflow drops the newest samples and counts them, so a stalled consumer never blocks
//! capture. Flushing for barge-in releases every queued sample in one index update.

use rtrb::{Consumer, Producer, RingBuffer};

pub struct PcmProducer {
    inner: Producer<f32>,
    dropped: u64,
}

pub struct PcmConsumer {
    inner: Consumer<f32>,
}

/// Creates a ring that holds `capacity` samples.
pub fn pcm_ring(capacity: usize) -> (PcmProducer, PcmConsumer) {
    let (producer, consumer) = RingBuffer::new(capacity);
    (
        PcmProducer {
            inner: producer,
            dropped: 0,
        },
        PcmConsumer { inner: consumer },
    )
}

impl PcmProducer {
    /// Writes as many samples as fit and returns how many were written. The rest are dropped and counted.
    pub fn push_slice(&mut self, samples: &[f32]) -> usize {
        let mut written = 0;
        for &sample in samples {
            if self.inner.push(sample).is_err() {
                break;
            }
            written += 1;
        }
        self.dropped += (samples.len() - written) as u64;
        written
    }

    /// Total samples dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl PcmConsumer {
    /// Fills `out` from the front of the ring and returns how many samples were read.
    pub fn pop_slice(&mut self, out: &mut [f32]) -> usize {
        let mut read = 0;
        for slot in out.iter_mut() {
            match self.inner.pop() {
                Ok(sample) => {
                    *slot = sample;
                    read += 1;
                }
                Err(_) => break,
            }
        }
        read
    }

    /// Discards everything queued, in one index update. Used on barge-in so stale TTS audio never plays.
    pub fn flush(&mut self) -> usize {
        let available = self.inner.slots();
        if available == 0 {
            return 0;
        }
        match self.inner.read_chunk(available) {
            Ok(chunk) => {
                chunk.commit_all();
                available
            }
            Err(_) => 0,
        }
    }

    pub fn available(&self) -> usize {
        self.inner.slots()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_arrive_in_order_across_threads() {
        let (mut tx, mut rx) = pcm_ring(1024);
        let total = 200_000usize;
        let producer = std::thread::spawn(move || {
            let mut next = 0usize;
            while next < total {
                let end = (next + 97).min(total);
                let chunk: Vec<f32> = (next..end).map(|i| i as f32).collect();
                let mut offset = 0;
                while offset < chunk.len() {
                    offset += tx.push_slice(&chunk[offset..]);
                    if offset < chunk.len() {
                        std::thread::yield_now();
                    }
                }
                next = end;
            }
        });
        let mut expected = 0usize;
        let mut buf = vec![0.0f32; 128];
        while expected < total {
            let n = rx.pop_slice(&mut buf);
            for &v in &buf[..n] {
                assert_eq!(v, expected as f32);
                expected += 1;
            }
            if n == 0 {
                std::thread::yield_now();
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn overflow_drops_newest_and_counts_them() {
        let (mut tx, mut rx) = pcm_ring(4);
        assert_eq!(tx.push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 4);
        assert_eq!(tx.dropped(), 2);
        let mut out = [0.0f32; 8];
        assert_eq!(rx.pop_slice(&mut out), 4);
        assert_eq!(&out[..4], &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn flush_discards_queued_audio_and_frees_space() {
        let (mut tx, mut rx) = pcm_ring(8);
        tx.push_slice(&[0.5; 8]);
        assert_eq!(rx.flush(), 8);
        assert_eq!(rx.available(), 0);
        assert_eq!(tx.push_slice(&[0.25; 8]), 8);
        let mut out = [0.0f32; 8];
        assert_eq!(rx.pop_slice(&mut out), 8);
        assert!(out.iter().all(|&s| s == 0.25));
    }
}
