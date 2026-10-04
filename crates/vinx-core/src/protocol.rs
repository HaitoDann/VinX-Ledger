use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The protocol version this build of the software implements (ADR 0086). A node whose
/// chain has activated a later version stops instead of applying rules it does not know.
/// Bump it with every consensus-rule change, gated by `WorldState::protocol_at_least`.
pub const NODE_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Semantic version of the VinX protocol running on-chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl ProtocolVersion {
    pub const GENESIS: Self = Self {
        major: 1,
        minor: 0,
        patch: 0,
    };

    /// Compact form carried in block headers: `major·10⁶ + minor·10³ + patch`.
    pub const fn as_u32(&self) -> u32 {
        self.major as u32 * 1_000_000 + self.minor as u32 * 1_000 + self.patch as u32
    }

    pub const fn from_u32(v: u32) -> Self {
        Self {
            major: (v / 1_000_000) as u16,
            minor: (v / 1_000 % 1_000) as u16,
            patch: (v % 1_000) as u16,
        }
    }

    pub fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Returns the upgrade type implied by moving from `self` to `next`.
    pub fn upgrade_type(&self, next: &ProtocolVersion) -> UpgradeType {
        if next.major != self.major {
            UpgradeType::Major
        } else if next.minor != self.minor {
            UpgradeType::Minor
        } else {
            UpgradeType::Patch
        }
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for ProtocolVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.splitn(3, '.').collect();
        if parts.len() != 3 {
            return Err(format!("expected X.Y.Z, got '{}'", s));
        }
        let major = parts[0]
            .parse::<u16>()
            .map_err(|_| format!("invalid major: {}", parts[0]))?;
        let minor = parts[1]
            .parse::<u16>()
            .map_err(|_| format!("invalid minor: {}", parts[1]))?;
        let patch = parts[2]
            .parse::<u16>()
            .map_err(|_| format!("invalid patch: {}", parts[2]))?;
        Ok(ProtocolVersion {
            major,
            minor,
            patch,
        })
    }
}

/// An upgrade scheduled to activate at a specific wall-clock time (ADR 0006).
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ScheduledUpgrade {
    pub version: ProtocolVersion,
    /// Unix timestamp (seconds) at which the upgrade activates.
    pub activation_ts: u64,
    /// Unix timestamp (seconds) at which the upgrade was announced (for
    /// notice-window validation).
    pub announced_at: u64,
}

/// Minimum notice window category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeType {
    Patch,
    Minor,
    Major,
}

impl UpgradeType {
    /// Minimum real seconds between announcement and activation (ADR 0006).
    pub fn min_notice_secs(self) -> u64 {
        use crate::amount::{
            UPGRADE_NOTICE_MAJOR_SECS, UPGRADE_NOTICE_MINOR_SECS, UPGRADE_NOTICE_PATCH_SECS,
        };
        match self {
            UpgradeType::Patch => UPGRADE_NOTICE_PATCH_SECS,
            UpgradeType::Minor => UPGRADE_NOTICE_MINOR_SECS,
            UpgradeType::Major => UPGRADE_NOTICE_MAJOR_SECS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upgrade_type_detection() {
        let v100 = ProtocolVersion::new(1, 0, 0);
        assert_eq!(
            v100.upgrade_type(&ProtocolVersion::new(1, 0, 1)),
            UpgradeType::Patch
        );
        assert_eq!(
            v100.upgrade_type(&ProtocolVersion::new(1, 1, 0)),
            UpgradeType::Minor
        );
        assert_eq!(
            v100.upgrade_type(&ProtocolVersion::new(2, 0, 0)),
            UpgradeType::Major
        );
    }

    #[test]
    fn test_display() {
        assert_eq!(ProtocolVersion::GENESIS.to_string(), "1.0.0");
        assert_eq!(ProtocolVersion::new(2, 3, 14).to_string(), "2.3.14");
    }

    #[test]
    fn test_from_str() {
        use std::str::FromStr;
        assert_eq!(
            ProtocolVersion::from_str("1.0.0").unwrap(),
            ProtocolVersion::GENESIS
        );
        assert_eq!(
            ProtocolVersion::from_str("2.3.14").unwrap(),
            ProtocolVersion::new(2, 3, 14)
        );
        assert!(ProtocolVersion::from_str("bad").is_err());
        assert!(ProtocolVersion::from_str("1.2").is_err());
    }

    #[test]
    fn test_min_notice_secs() {
        assert_eq!(UpgradeType::Patch.min_notice_secs(), 7 * 24 * 3600);
        assert_eq!(UpgradeType::Minor.min_notice_secs(), 30 * 24 * 3600);
        assert_eq!(UpgradeType::Major.min_notice_secs(), 90 * 24 * 3600);
    }
}
