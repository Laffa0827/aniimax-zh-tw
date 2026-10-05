//! Crackle Generator E-mode data and helpers.
//!
//! The electrical model is deliberately kept separate from recipe data: facility IDs/names stay
//! unchanged, while this module supplies the decoded electricRequire values and the generator
//! progression used by the planner.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElectricConfig {
    pub enabled: bool,
    /// Crackle Generator level (1..=5).
    pub generator_level: u8,
    /// 0 = combined, 1 = raw materials (Mine/Well), 2 = workstations/processing.
    pub strategy: u8,
}

impl Default for ElectricConfig {
    fn default() -> Self {
        Self { enabled: false, generator_level: 1, strategy: 0 }
    }
}

impl ElectricConfig {
    pub fn disabled() -> Self { Self::default() }

    pub fn new(generator_level: u8) -> Self {
        Self::new_with_strategy(generator_level, 0)
    }

    pub fn new_with_strategy(generator_level: u8, strategy: u8) -> Self {
        Self { enabled: true, generator_level: generator_level.clamp(1, 5), strategy: strategy.min(2) }
    }

    pub fn allows_facility(self, facility: &str) -> bool {
        match self.strategy {
            1 => matches!(facility, "Mine" | "Well"),
            2 => !matches!(facility, "Mine" | "Well"),
            _ => true,
        }
    }

    /// Decoded generator capacity: 600 / 800 / 1000 / 1200 / 1500.
    pub fn capacity(self) -> f64 {
        [600.0, 800.0, 1000.0, 1200.0, 1500.0][self.generator_level.saturating_sub(1).min(4) as usize]
    }

    /// The demand ceiling at which the network still receives the 120% E-mode boost.
    pub fn boost_threshold(self) -> f64 {
        [500.0, 660.0, 800.0, 1000.0, 1200.0][self.generator_level.saturating_sub(1).min(4) as usize]
    }

    /// Actual grid efficiency. Below/equal to the boost threshold the network can run at 120%;
    /// above it, E-mode falls back to 100% until the generator itself is overloaded.
    pub fn efficiency(self, demand: f64) -> f64 {
        if !self.enabled || demand <= 1e-9 {
            return 1.0;
        }
        let ratio = self.capacity() / demand;
        if demand <= self.boost_threshold() {
            ratio.min(1.2)
        } else {
            ratio.min(1.0)
        }
    }
}

/// Whether a facility may be driven by Crackle Generator E-mode.
///
/// The user has confirmed that the following facilities are NOT E-mode compatible. Every other
/// facility is treated as E-mode compatible. This is deliberately separate from electric_require:
/// compatibility is a game rule, while electric_require contains only power values we have data for.
pub fn electric_compatible(facility: &str) -> bool {
    !matches!(facility,
        "Farmland"
        | "Woodland"
        | "Tidewhisper Sandcastle"
        | "Dewy House"
        | "Nimbus Bed"
        | "Starfall Hammock"
        | "Floral Windmill"
        | "Heat Furnace"
        | "Cooling Unit"
        | "Sunlamp"
    )
}

/// Per-facility power model.
/// The level passed to `electric_require` must be the actual owned facility level,
/// not the production recipe's minimum unlock level.
///
/// `base_power` is the facility's own Lv.1 starting demand. `power_per_level` is the fixed
/// increase for each additional facility level. This deliberately does not assume that every
/// facility starts at the same wattage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElectricFacilityRule {
    pub base_power: f64,
    pub power_per_level: f64,
}

impl ElectricFacilityRule {
    pub const fn new(base_power: f64, power_per_level: f64) -> Self {
        Self { base_power, power_per_level }
    }

    pub fn require(self, level: u32) -> f64 {
        self.base_power + self.power_per_level * level.saturating_sub(1) as f64
    }
}

/// Electric facility data model.
///
/// The current model is intentionally expressed as per-facility base + per-level increment, so
/// future in-game verification only needs to change a facility's own base value. The model is
/// deliberately ready to accept a different base for any facility; it does not impose one global
/// starting wattage on all processors.
pub fn electric_rule(facility: &str) -> Option<ElectricFacilityRule> {
    match facility {
        // Gathering facilities: confirmed Lv4/Lv5/Lv6 Mine and Lv4/Lv5 Well progression.
        "Mine" | "Well" => Some(ElectricFacilityRule::new(30.0, 30.0)),

        // Ordinary processing facilities with confirmed 15W-per-level progression.
        "Carousel Mill"
        | "Crafting Table"
        | "Claw Game Cooker"
        | "Jukebox Dryer"
        | "Simmering Pot"
        | "Phonolfactory Table"
        | "Bouncy Brew Keg"
        | "Blazing Stove"
        | "Pickling Jar"
        | "Joy Wheel Loom"
        | "Woodworking Bench"
        | "Chimney Kiln" => Some(ElectricFacilityRule::new(15.0, 15.0)),

        // These two special facilities start at 30W, but follow the common +15W per-level increment.
        // Mine/Well are the only current facilities with the +30W-per-level progression.
        "Dance Pad Polisher" | "Aniipod Maker" => Some(ElectricFacilityRule::new(30.0, 15.0)),
        _ => None,
    }
}

pub fn electric_require(facility: &str, level: u32) -> Option<f64> {
    if level == 0 || !electric_compatible(facility) { return None; }
    electric_rule(facility).map(|rule| rule.require(level))
}

/// RV progression of the Crackle Generator. RV12-13 Lv1, RV14-15 Lv2, ... RV20 Lv5.
pub fn generator_level_for_rv(rv: u32) -> u8 {
    match rv {
        0..=13 => 1,
        14..=15 => 2,
        16..=17 => 3,
        18..=19 => 4,
        _ => 5,
    }
}
