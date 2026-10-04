//! Color-mode selection for CAMF-derived DNG rendering metadata.

use crate::Error;
use std::{fmt, str::FromStr};

/// SPP color mode to embed without changing the linear camera samples.
/// Availability depends on the selected mode's calibration in the X3F.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SppMode {
    /// Standard color and tone.
    Standard,
    /// Neutral color and reduced contrast.
    Neutral,
    /// Vivid color and increased contrast.
    Vivid,
    /// Portrait color and tone.
    Portrait,
    /// Landscape color and tone.
    Landscape,
    /// Foveon Classic Blue color and tone.
    FcBlue,
}

impl SppMode {
    /// Mode name used by CAMF property lists and the CLI selector.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Neutral => "Neutral",
            Self::Vivid => "Vivid",
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
            Self::FcBlue => "FCBlue",
        }
    }
}

impl fmt::Display for SppMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SppMode {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Standard" => Ok(Self::Standard),
            "Neutral" => Ok(Self::Neutral),
            "Vivid" => Ok(Self::Vivid),
            "Portrait" => Ok(Self::Portrait),
            "Landscape" => Ok(Self::Landscape),
            "FCBlue" => Ok(Self::FcBlue),
            _ => Err(Error::InvalidData(format!(
                "unsupported SPP color mode: {value}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_round_trip() {
        for mode in [
            SppMode::Standard,
            SppMode::Neutral,
            SppMode::Vivid,
            SppMode::Portrait,
            SppMode::Landscape,
            SppMode::FcBlue,
        ] {
            assert_eq!(mode.as_str().parse::<SppMode>().unwrap(), mode);
            assert_eq!(mode.to_string(), mode.as_str());
        }
        for name in [
            "",
            "standard",
            "Auto",
            "Monochrome",
            "Fov Classic Blue",
            "unknown",
        ] {
            assert!(name.parse::<SppMode>().is_err());
        }
        assert!(crate::ProcessOptions::default().dng_spp_mode.is_none());
    }
}
