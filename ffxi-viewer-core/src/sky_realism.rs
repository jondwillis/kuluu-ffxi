//! Realism toggles for sun/moon rendering.
//!
//! Each field gates one self-contained effect in
//! [`crate::sun_moon::sun_moon_system`] or the moon shader. Defaults
//! lean toward "more realistic than retail" because retail's sky was
//! a stylized 2002 fake (sun/moon antipodal regardless of phase, etc.
//! — see `ffxi-viewer-core/src/sun_moon.rs` docs for the trade).
//!
//! Operators flip features at runtime via `/sky <feature> [on|off|toggle]`
//! (parsed in `ffxi-client/src/view_native/slash_commands.rs::parse_sky`).
//! Persistence to disk lives with [`crate::graphics_settings::GraphicsSettings`]
//! as a future hook — for now the resource resets on launch.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// User-toggleable sky realism features. All booleans, all
/// independent, all hot-swappable.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkyRealism {
    /// Sun and moon redden as they near the horizon (Rayleigh
    /// scattering through thick atmosphere). Sun has always done
    /// this; moon previously stayed icy-blue at the horizon, which
    /// reads wrong against a sunset sky.
    pub horizon_reddening: bool,
    /// Both discs dim near the horizon, not just redden. Atmospheric
    /// extinction. Cheap multiplier on disc intensity.
    pub horizon_dimming: bool,
    /// "Moon illusion": the perceptual trick where the moon *looks*
    /// larger near the horizon than at zenith. We fake it by scaling
    /// the disc up to ~1.3× when altitude < 15°.
    pub moon_illusion: bool,
    /// Earthshine — the unlit portion of a crescent moon catches
    /// faint Vana'diel-bounced light. Brightest at thin crescent,
    /// vanishes at full. Already a flat 0.06 floor in the shader;
    /// with this on we make it phase-dependent and slightly tinted.
    pub earthshine: bool,
    /// **Stage 2 (not wired yet)** — derive the moon's *sky position*
    /// from the LSB phase lookup so geometry agrees with the disc's
    /// lit fraction. When on, a full moon will be visible during the
    /// day (in the sun direction), a half moon rises at noon, etc.
    /// Departs from retail's antipodal sun-moon arrangement.
    pub physical_moon_orbit: bool,
    /// **Stage 2 (not wired yet)** — surface solar/lunar eclipse
    /// events when `physical_moon_orbit` puts the discs in
    /// occulting/anti-aligned configurations. No-op when orbit is
    /// off (eclipses are impossible if the moon never visits the
    /// sun's hemisphere).
    pub eclipses: bool,
}

impl Default for SkyRealism {
    fn default() -> Self {
        Self::enhanced()
    }
}

impl SkyRealism {
    /// Our default, modern look — every enhancement on except the
    /// physical moon orbit (which departs visibly from retail's
    /// antipodal sun/moon and is opt-in via `/sky realmoon`).
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

    /// Retail-faithful look — every post-2002 embellishment off. The
    /// moon stays a fixed-size icy disc antipodal to the sun, with no
    /// horizon reddening/dimming, no earthshine, no eclipse events. The
    /// stylized 2002 sky the original client shipped.
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

/// Named features, used by the `/sky` slash command and persistence.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyFeature {
    HorizonReddening,
    HorizonDimming,
    MoonIllusion,
    Earthshine,
    PhysicalMoonOrbit,
    Eclipses,
}

impl SkyFeature {
    /// Lowercase token the operator types after `/sky`.
    pub const ALL: &'static [(&'static str, SkyFeature)] = &[
        ("reddening", SkyFeature::HorizonReddening),
        ("dimming", SkyFeature::HorizonDimming),
        ("illusion", SkyFeature::MoonIllusion),
        ("earthshine", SkyFeature::Earthshine),
        ("realmoon", SkyFeature::PhysicalMoonOrbit),
        ("eclipses", SkyFeature::Eclipses),
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(s))
            .map(|(_, v)| *v)
    }

    pub fn label(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, v)| *v == self)
            .map(|(k, _)| *k)
            .unwrap_or("?")
    }

    /// Get the current value from a `SkyRealism` resource.
    pub fn get(self, sky: &SkyRealism) -> bool {
        match self {
            Self::HorizonReddening => sky.horizon_reddening,
            Self::HorizonDimming => sky.horizon_dimming,
            Self::MoonIllusion => sky.moon_illusion,
            Self::Earthshine => sky.earthshine,
            Self::PhysicalMoonOrbit => sky.physical_moon_orbit,
            Self::Eclipses => sky.eclipses,
        }
    }

    /// Mutate the feature on a `SkyRealism` resource.
    pub fn set(self, sky: &mut SkyRealism, value: bool) {
        match self {
            Self::HorizonReddening => sky.horizon_reddening = value,
            Self::HorizonDimming => sky.horizon_dimming = value,
            Self::MoonIllusion => sky.moon_illusion = value,
            Self::Earthshine => sky.earthshine = value,
            Self::PhysicalMoonOrbit => sky.physical_moon_orbit = value,
            Self::Eclipses => sky.eclipses = value,
        }
    }
}
