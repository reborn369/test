//! What to put in the "push early" box, measured rather than guessed.
//!
//! A scheduled fire is aimed with the local clock, but the chain opens on its
//! own: the mint succeeds in the first block whose timestamp reaches the start
//! time. Two things sit between "our clock says now" and "our bytes are at the
//! sequencer" — how far the clock is off, and how long the packet is in flight.
//!
//! Both are measurable, and the flight is not the larger one. A machine whose
//! clock is 800ms slow fires eight blocks late on a 100ms chain and nothing in
//! the log says so, which is the failure this measurement exists to expose.

use crate::rpc::RpcClient;
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

/// Seconds between the NTP epoch (1900) and the Unix epoch (1970).
const NTP_UNIX_DELTA: u64 = 2_208_988_800;
/// Public time sources; all are asked and the fastest answer wins.
const TIME_SERVERS: [&str; 3] = [
    "time.google.com:123",
    "pool.ntp.org:123",
    "time.cloudflare.com:123",
];
/// Round trips sampled against the chain RPC.
const RTT_SAMPLES: usize = 12;
/// How far past the open the first knock is aimed.
///
/// Aiming early looks safer and is not: a knock refused a moment before the
/// open waits a whole interval for the next one, so the miss becomes the
/// interval rather than the aim. Landing a few milliseconds late instead means
/// the first knock is taken as it arrives. The push loop's fine interval is
/// what covers the case where this measurement was off in the wrong direction.
const AIM_LATE_MS: i64 = 5;
/// The push window cannot start earlier than the engine's own ceiling.
const LEAD_CEILING_MS: i64 = 3_000;

/// How long a run may spend asking what the time really is.
///
/// The check happens while the mint is being prepared, where seconds are free,
/// but it must never become the reason a run misses: past this the answer is
/// dropped and the machine's own clock is used, with the log saying so.
const CLOCK_SYNC_BUDGET: Duration = Duration::from_secs(4);

/// Do not run the check when the fire is closer than this.
pub const CLOCK_SYNC_MIN_LEAD_MS: i64 = 5_000;

/// Readings further out than this are reported but not applied.
///
/// NTP answers in UTC, so a timezone cannot produce one. An hour of genuine
/// drift is possible but far rarer than a malformed reply, and silently moving
/// a countdown by an hour is worse than leaving it alone and saying so.
const CLOCK_APPLY_CEILING_MS: i64 = 3_600_000;

/// Below this the clock is in step and the log says so in one quiet line.
const CLOCK_NOTABLE_MS: i64 = 25;

/// What the last check found. Applied to every later reading of the clock.
static CLOCK_OFFSET_MS: AtomicI64 = AtomicI64::new(0);
static CLOCK_CHECKED: AtomicBool = AtomicBool::new(false);

/// The clock a run fires by: this machine's, plus what the time servers said it
/// was out by.
///
/// A run aims at a wall-clock instant, so a machine whose clock is a second slow
/// fires a second late - and its own log still shows a countdown reaching zero
/// exactly on the mark, which is why the fault is so hard to see from inside.
/// Correcting here fixes every decision that depends on when now is.
pub fn true_now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis() + CLOCK_OFFSET_MS.load(Ordering::Relaxed)
}

/// Whole seconds of true time, floored - the units phase start times arrive in.
pub fn true_now_secs() -> i64 {
    true_now_ms().div_euclid(1_000)
}

/// The correction currently in force, if a check has ever succeeded.
pub fn clock_offset_applied_ms() -> Option<i64> {
    CLOCK_CHECKED
        .load(Ordering::Relaxed)
        .then(|| CLOCK_OFFSET_MS.load(Ordering::Relaxed))
}

/// What the measurement found, in the order it matters.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FireLagReport {
    /// Fastest round trip seen to the chain RPC.
    pub rtt_min_ms: u64,
    /// Typical round trip; a wide gap to the minimum means a noisy line.
    pub rtt_median_ms: u64,
    /// Half the fastest round trip — the packet's flight to the sequencer.
    pub one_way_ms: u64,
    /// How far the local clock is behind true time; negative means ahead.
    /// `None` when no time server could be reached.
    pub clock_offset_ms: Option<i64>,
    /// Which time server answered, or why none did.
    pub clock_source: String,
    /// What to put in the "push early" box.
    pub suggested_lead_ms: u64,
    /// One line saying where the number came from.
    pub summary: String,
}

/// The lead that puts our bytes at the sequencer when the chain opens.
///
/// Kept apart from the measuring so the arithmetic can be tested without a
/// network: it is the part that decides whether a run is early or late.
pub fn suggest_lead_ms(one_way_ms: u64, clock_offset_ms: Option<i64>) -> u64 {
    // A clock that runs fast already fires early, so its offset subtracts. No
    // reading at all is treated as a clock that is right — the only honest
    // assumption — but the flight still has to be covered.
    let offset = clock_offset_ms.unwrap_or(0);
    let raw = offset + one_way_ms as i64 - AIM_LATE_MS;
    let clamped = raw.clamp(0, LEAD_CEILING_MS);
    // Round up to a tidy step; the box is a hint, not a stopwatch.
    ((clamped + 9) / 10 * 10) as u64
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Ask one time server for the offset, with the network delay divided out.
///
/// The four-timestamp exchange is why this is worth a UDP socket rather than an
/// HTTP `Date` header: the header is whole seconds and carries the request's own
/// latency inside it, which is the very thing being measured.
async fn ntp_offset_ms(server: &str) -> Result<(i64, i64)> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("could not open a UDP socket for the time query")?;
    socket
        .connect(server)
        .await
        .with_context(|| format!("could not reach {server}"))?;

    let mut packet = [0u8; 48];
    packet[0] = 0x1b; // leap 0, version 3, mode 3 (client)

    let t1 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    socket.send(&packet).await.context("time request failed")?;

    let mut buf = [0u8; 48];
    tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buf))
        .await
        .context("time server did not answer in time")?
        .context("time reply failed")?;
    let t4 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let stamp_at = |offset: usize| -> i64 {
        let secs = u32::from_be_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as u64;
        let frac = u32::from_be_bytes([
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ]) as u64;
        let unix_secs = secs.saturating_sub(NTP_UNIX_DELTA) as i64;
        unix_secs * 1000 + ((frac * 1000) >> 32) as i64
    };
    let t2 = stamp_at(32); // server received
    let t3 = stamp_at(40); // server replied
    if t2 <= 0 || t3 <= 0 {
        anyhow::bail!("time server returned an empty timestamp");
    }
    Ok((((t2 - t1) + (t3 - t4)) / 2, ((t4 - t1) - (t3 - t2)).max(0)))
}

/// Ask every time server at once and keep the answer that travelled best.
///
/// Concurrent rather than one after another: three servers behind a blocked UDP
/// port would otherwise cost three timeouts in a row, and this runs inside a
/// run's preparation.
async fn best_offset() -> (Option<(i64, i64, String)>, Option<String>) {
    let mut jobs = tokio::task::JoinSet::new();
    for server in TIME_SERVERS {
        jobs.spawn(async move { (server, ntp_offset_ms(server).await) });
    }
    let mut best: Option<(i64, i64, String)> = None;
    let mut err: Option<String> = None;
    while let Some(joined) = jobs.join_next().await {
        match joined {
            Ok((server, Ok((offset, delay)))) => {
                if best.as_ref().map(|(_, d, _)| delay < *d).unwrap_or(true) {
                    best = Some((offset, delay, server.trim_end_matches(":123").to_string()));
                }
            }
            Ok((_, Err(e))) => err = Some(e.to_string()),
            Err(e) => err = Some(e.to_string()),
        }
    }
    (best, err)
}

/// Decide what to do with a reading, and what to say about it.
///
/// Kept apart from the network because this is the part that can move a run's
/// fire time by a second, and it has to be testable without a time server.
fn clock_verdict(
    best: Option<(i64, i64, String)>,
    err: Option<String>,
    carried: Option<i64>,
) -> (Option<i64>, String) {
    match best {
        Some((offset, _, host)) if offset.abs() > CLOCK_APPLY_CEILING_MS => (
            None,
            format!(
                "Clock: {host} puts this machine {offset}ms out, too far to be drift - leaving the clock alone"
            ),
        ),
        Some((offset, delay, host)) if offset.abs() >= CLOCK_NOTABLE_MS => (
            Some(offset),
            format!(
                "Clock: this machine is {}ms {} true time ({host}, +/-{delay}ms) - the countdown is corrected for it",
                offset.abs(),
                if offset > 0 { "behind" } else { "ahead of" }
            ),
        ),
        Some((offset, _, host)) => (
            Some(offset),
            format!("Clock: in step with true time ({host}, {offset}ms out)"),
        ),
        None => {
            let why = err.map(|e| format!(" ({e})")).unwrap_or_default();
            match carried {
                Some(prev) if prev != 0 => (
                    None,
                    format!(
                        "Clock: no time server answered{why} - keeping the {prev}ms correction from the last check"
                    ),
                ),
                _ => (
                    None,
                    format!(
                        "Clock: no time server answered{why} - the countdown follows this machine's own clock"
                    ),
                ),
            }
        }
    }
}

/// Learn how far this machine's clock is out and correct every later reading.
///
/// Returns the line for the operator log. Runs once per mint, during
/// preparation; the answer holds for the rest of the process.
pub async fn sync_clock() -> String {
    let (best, err) = match tokio::time::timeout(CLOCK_SYNC_BUDGET, best_offset()).await {
        Ok(pair) => pair,
        Err(_) => (
            None,
            Some(format!(
                "nothing answered within {}s",
                CLOCK_SYNC_BUDGET.as_secs()
            )),
        ),
    };
    let (apply, line) = clock_verdict(best, err, clock_offset_applied_ms());
    if let Some(offset) = apply {
        CLOCK_OFFSET_MS.store(offset, Ordering::Relaxed);
        CLOCK_CHECKED.store(true, Ordering::Relaxed);
    }
    line
}

/// Measure both halves of the lead and say what to put in the box.
pub async fn measure_fire_lag(rpc: &RpcClient) -> Result<FireLagReport> {
    // 1. Flight. The minimum is the honest figure: anything above it is
    //    queueing, which one well-timed packet will not meet.
    let mut rtts: Vec<u64> = Vec::with_capacity(RTT_SAMPLES);
    let mut last_err: Option<String> = None;
    for _ in 0..RTT_SAMPLES {
        let started = Instant::now();
        match rpc.chain_id().await {
            Ok(_) => rtts.push(started.elapsed().as_millis() as u64),
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    if rtts.is_empty() {
        anyhow::bail!(
            "the chain RPC did not answer{}",
            last_err.map(|e| format!(": {e}")).unwrap_or_default()
        );
    }
    rtts.sort_unstable();
    let rtt_min_ms = rtts[0];
    let rtt_median_ms = percentile(&rtts, 0.5);
    let one_way_ms = rtt_min_ms / 2;

    // 2. The clock. Whichever server answers with the smallest delay is the one
    //    to believe, so all of them are asked at once and the best kept.
    let (best, clock_err) = best_offset().await;

    let (clock_offset_ms, clock_source) = match &best {
        Some((offset, delay, host)) => (Some(*offset), format!("{host} (±{delay}ms)")),
        None => (
            None,
            format!(
                "no time server answered{} — assuming the clock is right",
                clock_err.map(|e| format!(" ({e})")).unwrap_or_default()
            ),
        ),
    };

    let suggested_lead_ms = suggest_lead_ms(one_way_ms, clock_offset_ms);
    let summary = match clock_offset_ms {
        Some(off) if off.abs() >= 50 => format!(
            "flight {one_way_ms}ms + clock {} {}ms → push early {suggested_lead_ms}ms",
            if off > 0 { "behind by" } else { "ahead by" },
            off.abs()
        ),
        Some(_) => {
            format!("flight {one_way_ms}ms, clock in step → push early {suggested_lead_ms}ms")
        }
        None => format!("flight {one_way_ms}ms, clock unknown → push early {suggested_lead_ms}ms"),
    };

    Ok(FireLagReport {
        rtt_min_ms,
        rtt_median_ms,
        one_way_ms,
        clock_offset_ms,
        clock_source,
        suggested_lead_ms,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_clock_dominates_the_suggestion() {
        // The case this exists for: 88ms of flight is the small half when the
        // machine is 811ms behind.
        assert_eq!(suggest_lead_ms(88, Some(811)), 900);
    }

    #[test]
    fn a_fast_clock_subtracts_because_it_already_fires_early() {
        assert_eq!(suggest_lead_ms(90, Some(-60)), 30);
    }

    #[test]
    fn an_unreadable_clock_still_covers_the_flight() {
        assert_eq!(suggest_lead_ms(90, None), 90);
    }

    #[test]
    fn the_aim_lands_just_after_the_open_not_before_it() {
        // miss = clock + flight - lead, so a lead this much short of the sum is
        // a knock that arrives once the gate is already open and is taken on the
        // spot, instead of being refused and waiting out an interval.
        let (clock, flight) = (200i64, 40u64);
        let lead = suggest_lead_ms(flight, Some(clock)) as i64;
        let miss = clock + flight as i64 - lead;
        assert!(
            (0..=10).contains(&miss),
            "expected to land just after the open, landed {miss}ms off"
        );
    }

    #[test]
    fn the_suggestion_never_leaves_the_window_the_engine_accepts() {
        // Past three seconds the push loop would run before the transactions are
        // signed, so a wild clock reading must not produce a number the engine
        // will silently cap.
        assert_eq!(suggest_lead_ms(50, Some(60_000)), 3_000);
        assert_eq!(suggest_lead_ms(0, Some(-60_000)), 0);
    }

    #[test]
    fn a_slow_clock_is_corrected_and_named() {
        let (apply, line) = clock_verdict(Some((811, 12, "time.google.com".into())), None, None);
        assert_eq!(
            apply,
            Some(811),
            "the whole point is that this gets applied"
        );
        assert!(line.contains("811ms behind"), "{line}");
    }

    #[test]
    fn a_clock_in_step_is_applied_but_reported_quietly() {
        let (apply, line) = clock_verdict(Some((3, 5, "time.google.com".into())), None, None);
        assert_eq!(apply, Some(3));
        assert!(line.contains("in step"), "{line}");
    }

    #[test]
    fn an_absurd_reading_is_refused_rather_than_jumping_the_countdown() {
        let (apply, line) = clock_verdict(Some((7_200_000, 9, "pool.ntp.org".into())), None, None);
        assert_eq!(apply, None);
        assert!(line.contains("too far to be drift"), "{line}");
    }

    #[test]
    fn a_silent_network_keeps_what_an_earlier_check_found() {
        let (apply, line) = clock_verdict(None, Some("blocked".into()), Some(811));
        assert_eq!(apply, None, "a failed check must not undo a good one");
        assert!(line.contains("keeping the 811ms"), "{line}");
    }

    #[test]
    fn a_silent_network_with_nothing_learned_admits_it() {
        let (apply, line) = clock_verdict(None, Some("blocked".into()), None);
        assert_eq!(apply, None);
        assert!(line.contains("this machine's own clock"), "{line}");
    }

    #[test]
    fn the_correction_moves_the_clock_the_run_reads() {
        let previous = CLOCK_OFFSET_MS.swap(1_500, Ordering::Relaxed);
        let corrected = true_now_ms() - chrono::Utc::now().timestamp_millis();
        CLOCK_OFFSET_MS.store(previous, Ordering::Relaxed);
        assert!(
            (1_400..=1_600).contains(&corrected),
            "expected the stored offset to show up in the clock, saw {corrected}ms"
        );
    }

    #[test]
    fn the_median_is_read_off_the_sorted_samples() {
        assert_eq!(percentile(&[10, 20, 30, 40, 50], 0.5), 30);
        assert_eq!(percentile(&[], 0.5), 0);
    }
}
