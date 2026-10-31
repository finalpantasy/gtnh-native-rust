//! Twenty-Hz clock with bounded catch-up; paused time never becomes deferred simulation work.
use std::time::Duration;
#[derive(Default)]
pub struct Stepper {
    remainder_ns: u64,
}
impl Stepper {
    pub fn advance(&mut self, elapsed: Duration, running: bool) -> u32 {
        if !running {
            self.remainder_ns = 0;
            return 0;
        }
        let accepted = elapsed.as_nanos().min(250_000_000) as u64;
        let total = self.remainder_ns + accepted;
        self.remainder_ns = total % 50_000_000;
        (total / 50_000_000) as u32
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regular_frames_accumulate_exactly_twenty_ticks_per_second() {
        let mut clock = Stepper::default();
        let ticks = (0..100)
            .map(|_| clock.advance(Duration::from_millis(10), true))
            .sum::<u32>();
        assert_eq!(ticks, 20);
    }
    #[test]
    fn stalls_and_pause_never_create_unbounded_or_offline_catchup() {
        let mut clock = Stepper::default();
        assert_eq!(clock.advance(Duration::from_secs(3600), true), 5);
        assert_eq!(clock.advance(Duration::ZERO, true), 0);
        assert_eq!(clock.advance(Duration::from_millis(25), true), 0);
        assert_eq!(clock.advance(Duration::from_secs(3600), false), 0);
        assert_eq!(clock.advance(Duration::from_millis(25), true), 0);
        assert_eq!(clock.advance(Duration::from_millis(25), true), 1);
    }
}
