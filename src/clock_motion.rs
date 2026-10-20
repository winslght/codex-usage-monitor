//! Presentation-only retention and monotonic interpolation. Never changes the Monitor cache.
use crate::clock_window::Snapshot;
use std::time::{Duration, Instant, SystemTime};
const TRANSITION: Duration = Duration::from_millis(700);
#[derive(Default)]
pub struct Motion {
    provider: String,
    last: Option<Snapshot>,
    from: Option<Snapshot>,
    started: Option<Instant>,
    version: Option<SystemTime>,
    target: Option<(f64, Option<SystemTime>, Option<u64>)>,
    waiting: bool,
}
pub struct Frame {
    pub snapshot: Snapshot,
    pub animating: bool,
}
fn mix_time(a: Option<SystemTime>, b: Option<SystemTime>, t: f64) -> Option<SystemTime> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if let Ok(d) = b.duration_since(a) {
                a.checked_add(d.mul_f64(t))
            } else {
                a.checked_sub(a.duration_since(b).ok()?.mul_f64(t))
            }
        }
        _ => b,
    }
}
impl Motion {
    pub fn frame(&mut self, raw: &Snapshot, wall: SystemTime, clock: Instant) -> Frame {
        if self.provider != raw.provider_name {
            *self = Self {
                provider: raw.provider_name.clone(),
                ..Default::default()
            };
        }
        let valid = raw.available
            && raw.remaining_percent.is_finite()
            && raw.ahead_of_time.is_some()
            && raw.reset_at.is_some()
            && raw.window_seconds.is_some_and(|seconds| seconds > 0);
        if raw.refreshing || !valid {
            self.waiting = true;
            self.started = None;
            self.from = None;
            if let Some(last) = &self.last {
                let mut cached = last.clone();
                cached.refreshing = raw.refreshing;
                cached.refresh_state = raw.refresh_state.clone();
                cached.status = raw.status.clone();
                cached.chinese = raw.chinese;
                cached.refresh_minutes = raw.refresh_minutes;
                cached.weekly = raw.weekly.clone();
                cached.pace_cached = true;
                cached.pace_reason = if raw.chinese {
                    "区域保留上次有效结果，等待新数据".into()
                } else {
                    "Showing last valid pace; awaiting new data".into()
                };
                return Frame {
                    snapshot: cached,
                    animating: false,
                };
            }
            return Frame {
                snapshot: raw.clone(),
                animating: false,
            };
        }
        let target = (raw.remaining_percent, raw.reset_at, raw.window_seconds);
        let changed = self.target != Some(target) || self.version != raw.updated_at || self.waiting;
        if changed {
            self.from = self.last.clone();
            self.started = self.from.as_ref().map(|_| clock);
            self.target = Some(target);
            self.version = raw.updated_at;
            self.waiting = false;
        }
        let elapsed = self
            .started
            .map(|start| clock.saturating_duration_since(start));
        let t = elapsed
            .map(|d| (d.as_secs_f64() / TRANSITION.as_secs_f64()).clamp(0.0, 1.0))
            .unwrap_or(1.0);
        let ease = t * t * (3.0 - 2.0 * t);
        let mut snapshot = raw.clone();
        snapshot.pace_at = Some(wall);
        snapshot.pace_cached = false;
        if let Some(from) = &self.from {
            if t < 1.0 {
                snapshot.remaining_percent = from.remaining_percent
                    + (raw.remaining_percent - from.remaining_percent) * ease;
                snapshot.reset_at = mix_time(from.reset_at, raw.reset_at, ease);
                if let (Some(a), Some(b)) = (from.window_seconds, raw.window_seconds) {
                    snapshot.window_seconds =
                        Some((a as f64 + (b as f64 - a as f64) * ease).round().max(1.0) as u64);
                }
                snapshot.pace_at = mix_time(from.pace_at, Some(wall), ease);
            }
        }
        if t >= 1.0 {
            self.started = None;
            self.from = None;
        }
        self.last = Some(snapshot.clone());
        Frame {
            snapshot,
            animating: t < 1.0,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn data(w: SystemTime, r: f64) -> Snapshot {
        Snapshot {
            provider_name: "Codex".into(),
            available: true,
            remaining_percent: r,
            ahead_of_time: Some(true),
            reset_at: Some(w + Duration::from_secs(8100)),
            window_seconds: Some(18000),
            updated_at: Some(w),
            ..Default::default()
        }
    }
    #[test]
    fn refreshing_retains_geometry_and_new_success_eases_to_target() {
        let w = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let clock = Instant::now();
        let mut m = Motion::default();
        let initial = m.frame(&data(w, 63.0), w, clock).snapshot;
        let pending = Snapshot {
            refreshing: true,
            ahead_of_time: None,
            status: "refresh in progress".into(),
            ..data(w, 0.0)
        };
        let held = m.frame(
            &pending,
            w + Duration::from_secs(30),
            clock + Duration::from_secs(30),
        );
        assert_eq!(held.snapshot.remaining_percent, 63.0);
        assert_eq!(held.snapshot.pace_at, initial.pace_at);
        assert!(held.snapshot.ahead_of_time.is_some());
        assert!(!held.animating);
        let newer = Snapshot {
            updated_at: Some(w + Duration::from_secs(31)),
            ..data(w, 20.0)
        };
        let start = clock + Duration::from_secs(31);
        let first = m.frame(&newer, w + Duration::from_secs(31), start);
        assert!(first.animating);
        assert_eq!(first.snapshot.remaining_percent, 63.0);
        let mid = m.frame(
            &newer,
            w + Duration::from_secs(31),
            start + Duration::from_millis(350),
        );
        assert!((mid.snapshot.remaining_percent - 41.5).abs() < 1e-6);
        let end = m.frame(
            &newer,
            w + Duration::from_secs(32),
            start + Duration::from_millis(710),
        );
        assert!(!end.animating);
        assert_eq!(end.snapshot.remaining_percent, 20.0);
    }
    #[test]
    fn provider_switch_and_first_load_do_not_reuse_other_accounts_geometry() {
        let w = SystemTime::now();
        let t = Instant::now();
        let mut m = Motion::default();
        let _ = m.frame(&data(w, 63.0), w, t);
        let other = Snapshot {
            provider_name: "Claude Code".into(),
            refreshing: true,
            available: false,
            ahead_of_time: None,
            ..data(w, 0.0)
        };
        let f = m.frame(&other, w, t);
        assert!(!f.snapshot.available);
        assert!(f.snapshot.ahead_of_time.is_none());
        assert!(!f.animating);
    }
    #[test]
    fn failed_refresh_preserves_explicitly_cached_geometry() {
        let w = SystemTime::now();
        let t = Instant::now();
        let mut m = Motion::default();
        let _ = m.frame(&data(w, 63.0), w, t);
        let error = Snapshot {
            ahead_of_time: None,
            status: "cached data; refresh failed".into(),
            ..data(w, 63.0)
        };
        let f = m.frame(&error, w, t);
        assert!(f.snapshot.ahead_of_time.is_some());
        assert!(f.snapshot.pace_cached);
        assert!(f.snapshot.status.contains("failed"));
        assert_eq!(f.snapshot.remaining_percent, 63.0);
    }
    #[test]
    fn stale_and_reset_gap_keep_frozen_pace_until_new_window_arrives() {
        let w = SystemTime::now();
        let t = Instant::now();
        let mut m = Motion::default();
        let first = m.frame(&data(w, 63.0), w, t).snapshot;
        let stale = Snapshot {
            ahead_of_time: None,
            reset_at: Some(w - Duration::from_secs(1)),
            status: "window reset; waiting".into(),
            ..data(w, 63.0)
        };
        let later = w + Duration::from_secs(9000);
        let held = m.frame(&stale, later, t + Duration::from_secs(9000));
        assert!(held.snapshot.pace_cached);
        assert_eq!(held.snapshot.pace_at, first.pace_at);
        assert_eq!(held.snapshot.reset_at, first.reset_at);
        let start = t + Duration::from_secs(9001);
        let fresh = data(later, 98.0);
        assert!(m.frame(&fresh, later, start).animating);
        let mid = m.frame(&fresh, later, start + Duration::from_millis(350));
        assert!(mid.animating);
        assert!(mid.snapshot.remaining_percent > 63.0 && mid.snapshot.remaining_percent < 98.0);
        assert!(!mid.snapshot.pace_cached);
        assert_eq!(
            m.frame(&fresh, later, start + Duration::from_secs(1))
                .snapshot
                .remaining_percent,
            98.0
        );
    }
}
