//! Four traffic classes sharing one peer connection (docs/implementation/05, section 2.2).
//!
//! Audio travels as unreliable QUIC datagrams so a late frame is dropped instead of stalling the stream behind it.
//! The other classes use streams. Higher stream priority is transmitted first, so control traffic preempts CRDT deltas,
//! and deltas preempt bulk transfers.

/// Traffic class of a message. Each class has one transport and one scheduling rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficClass {
    /// Microphone and playback frames. Datagrams only, never retransmitted.
    Audio,
    /// Epoch commits, tokens, and handshakes. Highest stream priority.
    Control,
    /// CRDT deltas and keyframes.
    Delta,
    /// Model weights and build blobs. Lowest priority, so it yields to everything else.
    Bulk,
}

impl TrafficClass {
    /// Stream scheduler priority, or `None` for `Audio`, which never uses a stream.
    pub fn stream_priority(self) -> Option<i32> {
        match self {
            TrafficClass::Audio => None,
            TrafficClass::Control => Some(100),
            TrafficClass::Delta => Some(50),
            TrafficClass::Bulk => Some(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TrafficClass;

    #[test]
    fn audio_never_takes_a_stream_and_stream_classes_schedule_control_then_delta_then_bulk() {
        assert_eq!(TrafficClass::Audio.stream_priority(), None);
        let control = TrafficClass::Control.stream_priority().unwrap();
        let delta = TrafficClass::Delta.stream_priority().unwrap();
        let bulk = TrafficClass::Bulk.stream_priority().unwrap();
        assert!(control > delta && delta > bulk);
    }
}
