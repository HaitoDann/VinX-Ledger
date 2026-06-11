use serde::{Deserialize, Serialize};
use std::fmt;

/// Semantic version of the VinX protocol running on-chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl ProtocolVersion {
    pub const GENESIS: Self = Self { major: 1, minor: 0, patch: 0 };

    pub fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self { major, minor, patch }
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

/// An upgrade scheduled to activate at a specific block height.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduledUpgrade {
    pub version: ProtocolVersion,
    /// Block height at which the upgrade activates.
    pub activation_height: u64,
    /// Block height at which the upgrade was announced (for notice-window validation).
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
    /// Minimum blocks between announcement and activation.
    pub fn min_notice_blocks(self) -> u64 {
        use crate::amount::{
            UPGRADE_NOTICE_MAJOR_BLOCKS, UPGRADE_NOTICE_MINOR_BLOCKS, UPGRADE_NOTICE_PATCH_BLOCKS,
        };
        match self {
            UpgradeType::Patch => UPGRADE_NOTICE_PATCH_BLOCKS,
            UpgradeType::Minor => UPGRADE_NOTICE_MINOR_BLOCKS,
            UpgradeType::Major => UPGRADE_NOTICE_MAJOR_BLOCKS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upgrade_type_detection() {
        let v100 = ProtocolVersion::new(1, 0, 0);
        assert_eq!(v100.upgrade_type(&ProtocolVersion::new(1, 0, 1)), UpgradeType::Patch);
        assert_eq!(v100.upgrade_type(&ProtocolVersion::new(1, 1, 0)), UpgradeType::Minor);
        assert_eq!(v100.upgrade_type(&ProtocolVersion::new(2, 0, 0)), UpgradeType::Major);
    }

    #[test]
    fn test_display() {
        assert_eq!(ProtocolVersion::GENESIS.to_string(), "1.0.0");
        assert_eq!(ProtocolVersion::new(2, 3, 14).to_string(), "2.3.14");
    }

    #[test]
    fn test_min_notice_blocks() {
        assert_eq!(UpgradeType::Patch.min_notice_blocks(), 7 * 24 * 360);
        assert_eq!(UpgradeType::Minor.min_notice_blocks(), 30 * 24 * 360);
        assert_eq!(UpgradeType::Major.min_notice_blocks(), 90 * 24 * 360);
    }
}
