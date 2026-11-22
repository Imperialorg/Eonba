//! Opt-in synthetic Class 0 capture on a dedicated, already authenticated session.
//! Neither role opens an endpoint, selects a path, or reconnects. There are no
//! timestamps on the wire: operator marks use the receiver clock and optional
//! decode-receipt round trips use only the sender clock.
use std::{fmt, sync::mpsc, time::Instant};

use super::{Arrival, AudioPcmChunk, Duration, ReliableFrame, WanEndpoint};
use tokio::time::{sleep_until, timeout};

const PERIOD: Duration = Duration::from_millis(20);
const CONTROL_WAIT: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_millis(500);
const READY: &[u8] = b"eon/continuity/1/ready";
const READY_COPIES: &[u8] = b"eon/continuity/1/ready-bound-copies";
const FINISH: &[u8] = b"eon/continuity/1/queued";
const DRAINED: &[u8] = b"eon/continuity/1/drained";
const TIMED_FINISH: &[u8] = b"eon/continuity/2/queued";
const TIMED_DRAINED: &[u8] = b"eon/continuity/2/unique";
// A short, fixed PCM receipt, distinguishable from a forward probe. Never
// reflect arbitrary bytes, timestamps, or more than one receipt per sequence.
const DECODED: [u8; 2] = *b"ok";
const SILENCE: [u8; 640] = [0; 640];

#[derive(Clone, Copy)]
pub struct CapturePlan {
    seconds: u16,
    copies: bool,
    timing: bool,
}

impl CapturePlan {
    pub fn new(seconds: u16) -> Option<Self> {
        (1..=120).contains(&seconds).then_some(Self {
            seconds,
            copies: false,
            timing: false,
        })
    }

    /// Both callers must separately attach a bound standby before this capture.
    /// This option never opens a transport or authorizes a different peer.
    pub fn with_redundant_media(self) -> Self {
        Self {
            copies: true,
            ..self
        }
    }

    /// Both operators opt into primary-only return traffic after first decode.
    pub fn with_decode_receipts(self) -> Self {
        Self {
            timing: true,
            ..self
        }
    }

    fn ready(self) -> ReliableFrame {
        let tag: &[u8] = match (self.timing, self.copies) {
            (false, false) => READY,
            (false, true) => READY_COPIES,
            (true, false) => b"eon/continuity/2/ready",
            (true, true) => b"eon/continuity/2/ready-bound-copies",
        };
        control(tag, self.limit())
    }

    fn timed_start(self) -> ReliableFrame {
        let tag: &[u8] = if self.copies {
            b"eon/continuity/2/start-bound-copies"
        } else {
            b"eon/continuity/2/start"
        };
        control(tag, self.limit())
    }

    fn finish_tag(self) -> &'static [u8] {
        if self.timing { TIMED_FINISH } else { FINISH }
    }

    /// Agreed probe interval, excluding rendezvous and the final drain.
    pub fn span(self) -> Duration {
        Duration::from_secs(self.seconds.into())
    }

    fn limit(self) -> u32 {
        u32::from(self.seconds) * 50
    }
}

// Fixed out-of-band agreed chunks, never interpreted as application commands.
fn control(tag: &[u8], count: u32) -> ReliableFrame {
    ReliableFrame::Bulk {
        index: count,
        bytes: tag.to_vec(),
    }
}

/// Queue consecutive sequences at least 20 ms apart, never catch up in bursts.
/// Consumes the endpoint: another capture requires fresh provisioning/consent.
pub async fn transmit(ep: WanEndpoint, plan: CapturePlan) -> Result<u32, &'static str> {
    transmit_observed(ep, plan).await.result()
}

/// Sender-local timing evidence for this endpoint only, including partial runs.
pub struct Transmission {
    status: Result<(), &'static str>,
    limit: u32,
    queued: u32,
    // Indexed by sequence - 1. Only allocated for an agreed timed run.
    samples: Vec<(Instant, Option<Duration>)>,
    peer_unique: Option<u32>,
}

impl Transmission {
    pub fn result(&self) -> Result<u32, &'static str> {
        self.status.map(|()| self.queued)
    }

    fn record_echo(&mut self, pcm: &AudioPcmChunk, via: Arrival, now: Instant) {
        if via == Arrival::Primary
            && pcm.timestamp_ms == 0
            && pcm.pcm_data == DECODED
            && let Ok(seq) = usize::try_from(pcm.sequence_number)
            && let Some((sent, bound)) = seq.checked_sub(1).and_then(|i| self.samples.get_mut(i))
            && bound.is_none()
        {
            *bound = now.checked_duration_since(*sent);
        }
    }

    fn acknowledged(&self) -> u32 {
        self.samples.iter().filter(|(_, rtt)| rtt.is_some()).count() as u32
    }

    // Never await an echo on the send/finish critical path. Poll the original
    // deadline or reliable exchange first; don't cancel a partially read stream.
    async fn observe_until<T>(&mut self, ep: &WanEndpoint, until: impl Future<Output = T>) -> T {
        if self.samples.is_empty() {
            return until.await;
        }
        tokio::pin!(until);
        loop {
            tokio::select! {
                biased;
                done = &mut until => return done,
                echo = ep.recv_audio_observed() => {
                    let Ok((pcm, via)) = echo else { return until.await };
                    self.record_echo(&pcm, via, Instant::now());
                }
            }
        }
    }
}

impl fmt::Display for Transmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "eon_continuity_timing_v1 clock=sender_monotonic")?;
        // Round up so the printed upper bound never understates the RTT.
        let upper_us = |rtt: Option<Duration>| {
            rtt.map_or_else(
                || "unavailable".into(),
                |d| d.as_nanos().div_ceil(1000).to_string(),
            )
        };
        let acknowledged = self.acknowledged();
        let mut max = None;
        for (i, (_, rtt)) in self.samples.iter().enumerate() {
            max = max.max(*rtt);
            writeln!(
                f,
                "seq={} decode_receipt_rtt_upper_us={}",
                i + 1,
                upper_us(*rtt)
            )?;
        }
        let timing = if acknowledged == 0 {
            "unavailable"
        } else {
            "observed"
        };
        writeln!(
            f,
            "status={} timing={timing} limit={} queued={} skipped={} acknowledged={acknowledged} unacknowledged={} peer_missing={} decoded_without_receipt={} max_decode_receipt_rtt_upper_us={}",
            self.status.err().unwrap_or("complete"),
            self.limit,
            self.queued,
            self.limit - self.queued,
            self.queued - acknowledged,
            value(self.peer_unique.map(|n| self.queued - n)),
            value(self.peer_unique.map(|n| n - acknowledged)),
            upper_us(max),
        )
    }
}

/// Like `transmit`, retaining partial sender observations even on a disconnect.
pub async fn transmit_observed(ep: WanEndpoint, plan: CapturePlan) -> Transmission {
    let mut out = Transmission {
        status: Ok(()),
        limit: plan.limit(),
        queued: 0,
        samples: Vec::new(),
        peer_unique: None,
    };
    out.status = async {
        if plan.copies && ep.media.is_none() {
            return Err("missing_bound_standby");
        }
        let ready = timeout(CONTROL_WAIT, ep.recv())
            .await
            .map_err(|_| "ready_timeout")?
            .map_err(|_| "ready_failed")?;
        if ready != plan.ready() {
            return Err("invalid_ready_or_plan");
        }
        if plan.timing {
            timeout(CONTROL_WAIT, ep.send(&plan.timed_start()))
                .await
                .map_err(|_| "start_timeout")?
                .map_err(|_| "start_failed")?;
        }
        let mut frame = AudioPcmChunk {
            pcm_data: SILENCE.to_vec(),
            ..Default::default()
        };
        let stop = Instant::now() + plan.span();
        while out.queued < plan.limit() && Instant::now() < stop {
            frame.sequence_number = i64::from(out.queued + 1);
            let start = plan.timing.then(Instant::now); // before encoding or enqueueing either copy
            if plan.copies {
                ep.send_redundant_audio(&frame).map(|_| ())
            } else {
                ep.send_audio(&frame)
            }
            .map_err(|_| "audio_queue_failed")?;
            out.queued += 1;
            if let Some(start) = start {
                out.samples.push((start, None));
            }
            out.observe_until(&ep, sleep_until((Instant::now() + PERIOD).min(stop).into()))
                .await;
        }
        let queued = out.queued;
        let finish = async {
            timeout(CONTROL_WAIT, ep.send(&control(plan.finish_tag(), queued)))
                .await
                .map_err(|_| "finish_timeout")?
                .map_err(|_| "finish_failed")?;
            timeout(CONTROL_WAIT + DRAIN, ep.recv())
                .await
                .map_err(|_| "drain_timeout")?
                .map_err(|_| "drain_failed")
        };
        let unique = match out.observe_until(&ep, finish).await? {
            ReliableFrame::Bulk { index, bytes }
                if plan.timing && bytes == TIMED_DRAINED && index <= queued =>
            {
                Some(index)
            }
            frame if !plan.timing && frame == control(DRAINED, queued) => None,
            _ => return Err("invalid_drain_receipt"),
        };
        // The receiver closes after its reliable DRAINED receipt. A late echo
        // is not that close and must not race the reliable receipt off the wire.
        let _ = out
            .observe_until(&ep, timeout(CONTROL_WAIT, ep.conn.closed()))
            .await;
        if unique.is_some_and(|n| out.acknowledged() > n) {
            return Err("inconsistent_decode_count");
        }
        out.peer_unique = unique;
        Ok(())
    }
    .await;
    let _ = timeout(CONTROL_WAIT, ep.close()).await;
    out
}

pub struct Capture {
    status: &'static str,
    limit: u32,
    timing: bool,
    queued: Option<u32>,
    // First PCM decode of each unique sequence, in local arrival order.
    receipts: Vec<(u32, Duration, Arrival)>,
    duplicates: u32,
    marks: Vec<Duration>,
}

impl Capture {
    fn phase(&self, at: Duration) -> &'static str {
        match self.marks.as_slice() {
            [begin, ..] if at < *begin => "before",
            [_, end] if at >= *end => "after",
            [_, _] => "during",
            _ => "unclassified",
        }
    }

    /// Increasing sequences decoded on opposite sides of both marks on this
    /// completed session. Does not imply gapless delivery or a route change.
    pub fn bracketed(&self) -> bool {
        let highest = |phase| {
            self.receipts
                .iter()
                .filter(|(_, at, _)| self.phase(*at) == phase)
                .map(|(seq, _, _)| seq)
                .max()
        };
        self.status == "complete"
            && self.marks.len() == 2
            && highest("before")
                .zip(highest("after"))
                .is_some_and(|(pre, post)| pre < post)
    }
}

impl fmt::Display for Capture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "eon_continuity_v1 clock=receiver_monotonic")?;
        for (name, t) in ["begin", "end"].iter().zip(&self.marks) {
            writeln!(f, "mark={name} rx_us={}", t.as_micros())?;
        }
        let mut last = None;
        let mut max_gap = None;
        let mut marked_gap = None;
        for &(seq, at, via) in &self.receipts {
            let gap = last.map(|prev: Duration| (at - prev).as_micros());
            max_gap = max_gap.max(gap);
            if let ([begin, end], Some(prev)) = (self.marks.as_slice(), last)
                && prev < *end
                && at >= *begin
            {
                marked_gap = marked_gap.max(gap);
            }
            writeln!(
                f,
                "seq={seq} rx_us={} gap_us={} phase={}{}",
                at.as_micros(),
                value(gap),
                self.phase(at),
                match via {
                    Arrival::Primary => "",
                    Arrival::PrimaryCopy => " via=primary_copy",
                    Arrival::StandbyCopy => " via=standby_copy",
                },
            )?;
            last = Some(at);
        }
        let unique = self.receipts.len();
        writeln!(
            f,
            "status={} bracketed={} limit={} queued={} unique={unique} missing={} duplicates={} max_gap_us={} max_mark_gap_us={} skipped={} timing={}",
            self.status,
            self.bracketed(),
            self.limit,
            value(self.queued),
            value(self.queued.and_then(|n| (n as usize).checked_sub(unique))),
            self.duplicates,
            value(max_gap),
            value(marked_gap),
            value(self.queued.map(|n| self.limit - n)),
            if self.timing {
                "see_sender"
            } else {
                "unavailable"
            },
        )
    }
}

fn value<T: ToString>(v: Option<T>) -> String {
    v.map_or_else(|| "unknown".into(), |n| n.to_string())
}

async fn send_ready(ep: &WanEndpoint, plan: CapturePlan) -> Result<(), &'static str> {
    if plan.copies && ep.media.is_none() {
        return Err("missing_bound_standby");
    }
    timeout(CONTROL_WAIT, ep.send(&plan.ready()))
        .await
        .map_err(|_| "ready_timeout")?
        .map_err(|_| "ready_failed")
}

/// Call `arm` at the first valid decode. It must return immediately; its source
/// timestamps up to two local operator events (begin, end) using `std::time::Instant`.
/// Retains partial receipts on disconnect/error, never appends a second session.
pub async fn receive(
    ep: WanEndpoint,
    plan: CapturePlan,
    arm: impl FnOnce() -> mpsc::Receiver<Instant>,
) -> Capture {
    let origin = Instant::now();
    let limit = plan.limit();
    let mut out = Capture {
        status: "timeout",
        limit,
        timing: false,
        queued: None,
        receipts: Vec::new(),
        duplicates: 0,
        marks: Vec::new(),
    };
    if plan.timing {
        // The transport ACK for READY only means the frame decoded. Require a
        // role-specific START on this very primary before arming or echoing.
        let negotiated = async {
            send_ready(&ep, plan).await?;
            let start = timeout(CONTROL_WAIT, ep.recv())
                .await
                .map_err(|_| "start_timeout")?
                .map_err(|_| "start_failed")?;
            (start == plan.timed_start())
                .then_some(())
                .ok_or("invalid_start_or_plan")
        }
        .await;
        if let Err(status) = negotiated {
            out.status = status;
            let _ = timeout(CONTROL_WAIT, ep.close()).await;
            return out;
        }
        out.timing = true;
    }
    let mut seen = vec![false; limit as usize + 1];
    let mut echo = plan.timing.then(|| AudioPcmChunk {
        pcm_data: DECODED.to_vec(),
        ..Default::default()
    });
    let mut arm = Some(arm);
    let mut marks = None;
    let mut deadline = Instant::now() + plan.span() + CONTROL_WAIT * 2;
    // In v1 keep reading audio while READY waits for its transport receipt.
    let finish = async {
        if !plan.timing {
            send_ready(&ep, plan).await?;
        }
        match ep.recv().await {
            Ok(ReliableFrame::Bulk { index, bytes })
                if bytes == plan.finish_tag() && index <= limit =>
            {
                Ok(index)
            }
            Ok(_) => Err("invalid_finish"),
            Err(_) => Err("finish_failed"),
        }
    };
    {
        tokio::pin!(finish);
        out.status = loop {
            tokio::select! {
                biased;
                _ = sleep_until(deadline.into()) => break if out.queued.is_some() { "complete" } else { "timeout" },
                result = &mut finish, if out.queued.is_none() => match result {
                    Ok(n) if seen[n as usize + 1..].contains(&true) => break "inconsistent_finish",
                    Ok(_) if origin.elapsed() < plan.span() => break "early_finish",
                    Ok(n) => {
                        out.queued = Some(n);
                        deadline = Instant::now() + DRAIN;
                    }
                    Err(status) => break status,
                },
                sample = ep.recv_audio_observed() => {
                    let at = origin.elapsed();
                    let Ok((pcm, via)) = sample else { break "audio_receive_failed" };
                    let Ok(seq) = u32::try_from(pcm.sequence_number) else { break "invalid_probe" };
                    if !(1..=limit).contains(&seq)
                        || pcm.timestamp_ms != 0 || pcm.pcm_data != SILENCE
                        || plan.copies != (via != Arrival::Primary)
                    {
                        break "invalid_probe";
                    }
                    if out.queued.is_some_and(|n| seq > n) {
                        out.queued = None;
                        break "inconsistent_finish";
                    }
                    if std::mem::replace(&mut seen[seq as usize], true) {
                        out.duplicates = out.duplicates.saturating_add(1);
                    } else {
                        out.receipts.push((seq, at, via));
                        if let Some(arm) = arm.take() { marks = Some(arm()); }
                        if let Some(echo) = &mut echo {
                            // Best effort on the primary even for a standby-first
                            // decode. Failure to queue a return is not forward loss.
                            echo.sequence_number = pcm.sequence_number;
                            let _ = ep.send_audio(echo);
                        }
                    }
                }
            }
        };
    }
    let stopped = Instant::now();
    if let Some(rx) = marks {
        for t in rx.try_iter().take(2) {
            if !(origin..=stopped).contains(&t) {
                break;
            }
            let at = t.duration_since(origin);
            if out.marks.last().is_some_and(|begin| at <= *begin) {
                break;
            }
            out.marks.push(at);
        }
    }
    if out.status == "complete"
        && let Some(n) = out.queued
    {
        let drained = if plan.timing {
            control(TIMED_DRAINED, out.receipts.len() as u32)
        } else {
            control(DRAINED, n)
        };
        if !matches!(timeout(CONTROL_WAIT, ep.send(&drained)).await, Ok(Ok(()))) {
            out.status = "drain_receipt_failed";
        }
    }
    let _ = timeout(CONTROL_WAIT, ep.close()).await;
    out
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    #[test]
    fn iroh_and_qos_tests_continuity_bounds_only_first_valid_return_for_queued_sequence() {
        let start = Instant::now();
        let mut out = Transmission {
            status: Ok(()),
            limit: 50,
            queued: 3,
            samples: vec![(start, None); 3],
            peer_unique: Some(2),
        };
        let mut pcm = AudioPcmChunk {
            sequence_number: 1,
            pcm_data: DECODED.to_vec(),
            ..Default::default()
        };
        let at = start + Duration::from_nanos(5_000_001);
        for seq in [-1, 0, 4, i64::MAX] {
            pcm.sequence_number = seq;
            out.record_echo(&pcm, Arrival::Primary, at);
        }
        pcm.sequence_number = 2;
        out.record_echo(&pcm, Arrival::StandbyCopy, at);
        pcm.timestamp_ms = 1;
        out.record_echo(&pcm, Arrival::Primary, at);
        pcm.timestamp_ms = 0;
        pcm.pcm_data = SILENCE.to_vec();
        out.record_echo(&pcm, Arrival::Primary, at);
        assert!(out.to_string().contains("timing=unavailable"));

        pcm.pcm_data = DECODED.to_vec();
        out.record_echo(&pcm, Arrival::Primary, at);
        out.record_echo(&pcm, Arrival::Primary, at + PERIOD);
        let report = out.to_string();
        assert!(report.contains("seq=2 decode_receipt_rtt_upper_us=5001\n"));
        assert!(report.contains("seq=1 decode_receipt_rtt_upper_us=unavailable\n"));
        assert!(report.contains(
            "skipped=47 acknowledged=1 unacknowledged=2 peer_missing=1 decoded_without_receipt=1"
        ));
    }
}
