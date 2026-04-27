use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkyRealism {
    pub horizon_reddening: bool,

    pub horizon_dimming: bool,

    pub moon_illusion: bool,

    pub earthshine: bool,

    pub physical_moon_orbit: bool,

    pub eclipses: bool,
}

impl Default for SkyRealism {
    fn default() -> Self {
        Self::enhanced()
    }
}

impl SkyRealism {
    pub const fn enhanced() -> Self {
        Self {
            horizon_reddening: true,
            horizon_dimming: true,
            moon_illusion: true,
            earthshine: true,
            physical_moon_orbit: false,
            eclipses: true,
        }
    }

    pub const fn retail() -> Self {
        Self {
            horizon_reddening: false,
            horizon_dimming: false,
            moon_illusion: false,
            earthshine: false,
            physical_moon_orbit: false,
            eclipses: false,
        }
    }
}
