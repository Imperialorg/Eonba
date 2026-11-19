//! Hardware-keyed model profiles P0 to P3 (docs/implementation/02, section 2.1).
//!
//! A profile is chosen from the device's memory. The lock key matches the profile name in
//! `scripts/models.lock.json`, so the Rust selection and the downloader cannot disagree silently.

use std::fmt;
use std::str::FromStr;

use eon_primitives::{EonError, EonResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelProfile {
    /// Mid-range Android with up to about 6 GB of RAM.
    P0,
    /// Flagship mobile edge with 8 to 12 GB of RAM.
    P1,
    /// Developer laptop or single GPU with 12 to 16 GB.
    P2,
    /// Workstation or mesh compute with 16 GB or more of VRAM or unified memory.
    P3,
}

impl ModelProfile {
    pub const ALL: [ModelProfile; 4] = [Self::P0, Self::P1, Self::P2, Self::P3];

    /// Profile key in `scripts/models.lock.json`.
    pub fn lock_key(self) -> &'static str {
        match self {
            Self::P0 => "p0_mobile_ultralight",
            Self::P1 => "p1_mobile_flagship",
            Self::P2 => "p2_workstation_16g",
            Self::P3 => "p3_workstation_heavy",
        }
    }

    /// Smallest device memory, in GB, on which this profile is selected.
    pub fn min_device_memory_gb(self) -> f64 {
        match self {
            Self::P0 => 0.0,
            Self::P1 => 8.0,
            Self::P2 => 12.0,
            Self::P3 => 16.0,
        }
    }

    /// Picks the heaviest profile whose minimum the device meets.
    ///
    /// Boundaries are inclusive at the lower bound, so exactly 12 GB selects P2. The 6 to 8 GB gap
    /// falls to P0, the lighter choice. Negative, NaN, and infinite values are rejected.
    pub fn select_for_memory_gb(memory_gb: f64) -> EonResult<Self> {
        if !memory_gb.is_finite() || memory_gb < 0.0 {
            return Err(EonError::ValidationError(format!(
                "device memory must be a finite, non-negative GB value, got {memory_gb}"
            )));
        }
        Ok(Self::ALL
            .iter()
            .rev()
            .copied()
            .find(|profile| memory_gb >= profile.min_device_memory_gb())
            .unwrap_or(Self::P0))
    }
}

impl fmt::Display for ModelProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let short = match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
        };
        f.write_str(short)
    }
}

impl FromStr for ModelProfile {
    type Err = EonError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|profile| profile.to_string() == s)
            .ok_or_else(|| EonError::ValidationError(format!("unknown model profile '{s}', expected p0 to p3")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = include_str!("../../../../../scripts/models.lock.json");

    fn select(gb: f64) -> ModelProfile {
        ModelProfile::select_for_memory_gb(gb).unwrap()
    }

    #[test]
    fn selection_follows_the_spec_bands() {
        assert_eq!(select(4.0), ModelProfile::P0);
        assert_eq!(select(6.0), ModelProfile::P0);
        assert_eq!(select(7.9), ModelProfile::P0);
        assert_eq!(select(8.0), ModelProfile::P1);
        assert_eq!(select(11.99), ModelProfile::P1);
        assert_eq!(select(12.0), ModelProfile::P2);
        assert_eq!(select(15.99), ModelProfile::P2);
        assert_eq!(select(16.0), ModelProfile::P3);
        assert_eq!(select(128.0), ModelProfile::P3);
    }

    #[test]
    fn selection_is_monotone_in_memory() {
        let mut previous = ModelProfile::P0;
        for tenth in 0..=400 {
            let current = select(tenth as f64 / 10.0);
            assert!(current >= previous, "profile went down at {} GB", tenth as f64 / 10.0);
            previous = current;
        }
    }

    #[test]
    fn invalid_memory_values_are_rejected() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(ModelProfile::select_for_memory_gb(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn short_names_round_trip_and_unknown_names_fail() {
        for profile in ModelProfile::ALL {
            assert_eq!(profile.to_string().parse::<ModelProfile>().unwrap(), profile);
        }
        assert!("P2".parse::<ModelProfile>().is_err(), "names are lowercase");
        assert!("p4".parse::<ModelProfile>().is_err());
        assert!("".parse::<ModelProfile>().is_err());
    }

    #[test]
    fn minimum_memory_increases_with_profile_weight() {
        let minimums: Vec<f64> = ModelProfile::ALL.iter().map(|p| p.min_device_memory_gb()).collect();
        assert!(minimums.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn every_lock_key_exists_in_the_downloader_lockfile() {
        for profile in ModelProfile::ALL {
            let quoted = format!("\"{}\":", profile.lock_key());
            assert!(LOCK.contains(&quoted), "scripts/models.lock.json has no profile {}", profile.lock_key());
        }
    }
}
