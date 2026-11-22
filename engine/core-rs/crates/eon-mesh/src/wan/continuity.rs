//! Opt-in synthetic Class 0 capture on a dedicated, already authenticated session.
//! Neither role opens an endpoint, selects a path, or reconnects. There are no
//! sender timestamps on the wire: receipts and operator marks use the receiver clock.
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
const SILENCE: [u8; 640] = [0; 640];

#[derive(Clone, Copy)]
pub struct CapturePlan {
    seconds: u16,
    copies: bool,
}

impl CapturePlan {
    pub fn new(seconds: u16) -> Option<Self> {
        (1..=120).contains(&seconds).then_some(Self {
            seconds,
            copies: false,
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

    fn ready(self) -> ReliableFrame {
        control(if self.copies { READY_COPIES } else { READY }, self.limit())
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
    let result = async {
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
        let mut frame = AudioPcmChunk {
            pcm_data: SILENCE.to_vec(),
            ..Default::default()
        };
        let stop = Instant::now() + plan.span();
        let mut queued = 0;
        while queued < plan.limit() && Instant::now() < stop {
            frame.sequence_number = i64::from(queued + 1);
            if plan.copies {
                ep.send_redundant_audio(&frame).map(|_| ())
            } else {
                ep.send_audio(&frame)
            }
            .map_err(|_| "audio_queue_failed")?;
            queued += 1;
            sleep_until((Instant::now() + PERIOD).min(stop).into()).await;
        }
        timeout(CONTROL_WAIT, ep.send(&control(FINISH, queued)))
            .await
            .map_err(|_| "finish_timeout")?
            .map_err(|_| "finish_failed")?;
        let ack = timeout(CONTROL_WAIT + DRAIN, ep.recv())
            .await
            .map_err(|_| "drain_timeout")?
            .map_err(|_| "drain_failed")?;
        if ack != control(DRAINED, queued) {
            return Err("invalid_drain_receipt");
        }
        // Let the reliable receipt for DRAINED reach the receiver before close.
        // The receiver sends no PCM and closes upon receiving that receipt.
        let _ = timeout(CONTROL_WAIT, ep.recv_audio()).await;
        Ok(queued)
    }
    .await;
    let _ = timeout(CONTROL_WAIT, ep.close()).await;
    result
}

pub struct Capture {
    status: &'static str,
    limit: u32,
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
            "status={} bracketed={} limit={} queued={} unique={unique} missing={} duplicates={} max_gap_us={} max_mark_gap_us={}",
            self.status,
            self.bracketed(),
            self.limit,
            value(self.queued),
            value(self.queued.and_then(|n| (n as usize).checked_sub(unique))),
            self.duplicates,
            value(max_gap),
            value(marked_gap)
        )
    }
}

fn value<T: ToString>(v: Option<T>) -> String {
    v.map_or_else(|| "unknown".into(), |n| n.to_string())
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
        queued: None,
        receipts: Vec::new(),
        duplicates: 0,
        marks: Vec::new(),
    };
    let mut seen = vec![false; limit as usize + 1];
    let mut arm = Some(arm);
    let mut marks = None;
    let mut deadline = origin + plan.span() + CONTROL_WAIT * 2;
    // Keep reading audio while the READY stream waits for its decode receipt.
    let finish = async {
        if plan.copies && ep.media.is_none() {
            return Err("missing_bound_standby");
        }
        timeout(CONTROL_WAIT, ep.send(&plan.ready()))
            .await
            .map_err(|_| "ready_timeout")?
            .map_err(|_| "ready_failed")?;
        match ep.recv().await {
            Ok(ReliableFrame::Bulk { index, bytes }) if bytes == FINISH && index <= limit => {
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
        && !matches!(
            timeout(CONTROL_WAIT, ep.send(&control(DRAINED, n))).await,
            Ok(Ok(()))
        )
    {
        out.status = "drain_receipt_failed";
    }
    let _ = timeout(CONTROL_WAIT, ep.close()).await;
    out
}
