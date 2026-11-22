use std::fmt;
use std::ops::{Add, Sub};
use std::time::{Duration, Instant};

/// Monotonic instant independent of system wall-clock adjustments or NTP shifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonotonicInstant(Instant);

impl MonotonicInstant {
    pub fn now() -> Self {
        Self(Instant::now())
    }

    pub fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    pub fn duration_since(&self, earlier: MonotonicInstant) -> Duration {
        self.0.duration_since(earlier.0)
    }
}

impl Default for MonotonicInstant {
    fn default() -> Self {
        Self::now()
    }
}

impl Sub for MonotonicInstant {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        self.duration_since(rhs)
    }
}

impl Add<Duration> for MonotonicInstant {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl Sub<Duration> for MonotonicInstant {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self::Output {
        Self(self.0 - rhs)
    }
}

impl fmt::Display for MonotonicInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MonotonicInstant({:?})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monotonic_time_progression() {
        let t1 = MonotonicInstant::now();
        std::thread::sleep(Duration::from_millis(5));
        let t2 = MonotonicInstant::now();
        assert!(t2 > t1);
        assert!(t2 - t1 >= Duration::from_millis(4));
    }
}

