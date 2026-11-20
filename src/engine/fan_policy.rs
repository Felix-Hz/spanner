//! "When hot" full-speed fans: a hysteresis rule run by the recorder on every sample.
//! Separate on and off thresholds plus a minimum on-time keep the fans from flapping.

use std::time::{Duration, Instant};

pub const CPU_ON_C: u8 = 90;
pub const CPU_OFF_C: u8 = 80;
pub const GPU_ON_C: u8 = 83;
pub const GPU_OFF_C: u8 = 73;
pub const MIN_ON: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct FullSpeedPolicy {
    switched_on_at: Option<Instant>,
}

impl FullSpeedPolicy {
    /// Desired full-speed state given current temperatures (`None`: sensor off, e.g. dGPU asleep)
    /// and whether full speed is on right now.
    pub fn decide(&mut self, cpu_c: Option<u8>, gpu_c: Option<u8>, on_now: bool, now: Instant) -> bool {
        let hot = cpu_c.is_some_and(|t| t >= CPU_ON_C) || gpu_c.is_some_and(|t| t >= GPU_ON_C);
        let cool = cpu_c.is_none_or(|t| t < CPU_OFF_C) && gpu_c.is_none_or(|t| t < GPU_OFF_C);
        let held = self.switched_on_at.is_some_and(|t| now.duration_since(t) < MIN_ON);
        let want = if on_now { !(cool && !held) } else { hot };
        match (on_now, want) {
            (false, true) => self.switched_on_at = Some(now),
            (true, false) => self.switched_on_at = None,
            // Turned on by something else (MSI Center, the app): start the hold from now.
            (true, true) if self.switched_on_at.is_none() => self.switched_on_at = Some(now),
            _ => {}
        }
        want
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_on_when_either_chip_is_hot() {
        let t = Instant::now();
        assert!(FullSpeedPolicy::default().decide(Some(90), None, false, t));
        assert!(FullSpeedPolicy::default().decide(Some(60), Some(83), false, t));
        assert!(!FullSpeedPolicy::default().decide(Some(89), Some(82), false, t));
    }

    #[test]
    fn stays_on_between_thresholds_and_for_the_minimum_time() {
        let mut p = FullSpeedPolicy::default();
        let t0 = Instant::now();
        assert!(p.decide(Some(92), None, false, t0));
        assert!(p.decide(Some(70), None, true, t0 + Duration::from_secs(10)), "held for MIN_ON");
        assert!(p.decide(Some(85), None, true, t0 + Duration::from_secs(40)), "between thresholds");
        assert!(!p.decide(Some(79), Some(72), true, t0 + Duration::from_secs(45)), "both cool");
    }

    #[test]
    fn a_hot_gpu_keeps_it_on() {
        let mut p = FullSpeedPolicy::default();
        let t0 = Instant::now();
        assert!(p.decide(Some(95), Some(80), false, t0));
        assert!(p.decide(Some(70), Some(75), true, t0 + Duration::from_secs(60)));
    }
}
