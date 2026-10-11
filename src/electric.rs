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
    /// How many Crackle Generators feed the shared grid (see [`generator_count_for_rv`]).
    pub generator_count: u8,
    /// 0 = combined, 1 = raw materials (Mine/Well), 2 = workstations/processing.
    pub strategy: u8,
    /// Facility code to exclude from E-mode during counterfactual trade-off analysis. 0 = none.
    pub forbidden_facility: u8,
    /// At least this many powered facility units: the best an earlier solve found (see
    /// `Goal::MostPowered` in `exact.rs`). 0 = no floor.
    pub min_powered: u32,
    /// At least this many watts drawn by powered units: the best an earlier solve found with
    /// `min_powered` kept (see `Goal::MostPowerDraw`). 0 = no floor.
    pub min_draw: u32,
    /// Whether the Aniipod Maker may be powered. Off unless the player turns it on.
    pub aniipod_maker: bool,
    /// Whether the Dance Pad Polisher may be powered. Off unless the player turns it on.
    pub dance_pad_polisher: bool,
    /// Manual allocation: exactly this many powered Mine units (0 = the Mines get no power).
    /// `None` = the planner decides, as it does for every other facility.
    pub mine_units: Option<u32>,
    /// Manual allocation for the Wells, as `mine_units`.
    pub well_units: Option<u32>,
}

impl Default for ElectricConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            generator_level: 1,
            generator_count: 1,
            strategy: 0,
            forbidden_facility: 0,
            min_powered: 0,
            min_draw: 0,
            aniipod_maker: false,
            dance_pad_polisher: false,
            mine_units: None,
            well_units: None,
        }
    }
}

impl ElectricConfig {
    pub fn disabled() -> Self { Self::default() }

    pub fn new(generator_level: u8) -> Self {
        Self::new_with_strategy(generator_level, 0)
    }

    pub fn new_with_strategy(generator_level: u8, strategy: u8) -> Self {
        Self::new_with_strategy_and_forbidden(generator_level, strategy, 0)
    }

    pub fn new_with_strategy_and_forbidden(generator_level: u8, strategy: u8, forbidden_facility: u8) -> Self {
        Self {
            enabled: true,
            generator_level: generator_level.clamp(1, 5),
            generator_count: 1,
            strategy: strategy.min(2),
            forbidden_facility,
            min_powered: 0,
            min_draw: 0,
            aniipod_maker: false,
            dance_pad_polisher: false,
            mine_units: None,
            well_units: None,
        }
    }

    /// The same grid, with the Aniipod Maker and the Dance Pad Polisher allowed E-mode or not.
    pub fn with_special_facilities(self, aniipod_maker: bool, dance_pad_polisher: bool) -> Self {
        Self { aniipod_maker, dance_pad_polisher, ..self }
    }

    /// The same grid fed by `count` generators (at least one).
    pub fn with_generators(self, count: u8) -> Self {
        Self { generator_count: count.max(1), ..self }
    }

    /// The same grid, keeping at least `min_powered` powered units drawing at least `min_draw` W.
    pub fn with_floors(self, min_powered: u32, min_draw: u32) -> Self {
        Self { min_powered, min_draw, ..self }
    }

    /// The same grid with the player's own Mine and Well allocation (manual mode): exactly that
    /// many powered units of each, `None` leaving it to the planner.
    pub fn with_manual_units(self, mine_units: Option<u32>, well_units: Option<u32>) -> Self {
        Self { mine_units, well_units, ..self }
    }

    /// The powered units the player fixed for `facility`, if any (only the Mine and the Well).
    /// A facility left out of E-mode (`forbidden_facility`) has none fixed.
    pub fn manual_units(self, facility: &str) -> Option<u32> {
        if self.forbidden_facility != 0 && electric_facility_code(facility) == self.forbidden_facility {
            return None;
        }
        match facility {
            "Mine" => self.mine_units,
            "Well" => self.well_units,
            _ => None,
        }
    }

    pub fn allows_facility(self, facility: &str) -> bool {
        if self.forbidden_facility != 0 && electric_facility_code(facility) == self.forbidden_facility {
            return false;
        }
        // Each of these two only when the player has turned its own switch on.
        if (facility == "Aniipod Maker" && !self.aniipod_maker) || (facility == "Dance Pad Polisher" && !self.dance_pad_polisher) {
            return false;
        }
        match self.strategy {
            1 => matches!(facility, "Mine" | "Well"),
            2 => !matches!(facility, "Mine" | "Well"),
            _ => true,
        }
    }


    /// One generator's output at its level: 600 / 800 / 1000 / 1200 / 1500 W.
    pub fn unit_capacity(self) -> f64 {
        GENERATOR_UNIT_CAPACITY[self.generator_level.saturating_sub(1).min(4) as usize]
    }

    /// The whole grid's output: every generator's together.
    pub fn capacity(self) -> f64 {
        self.unit_capacity() * self.generator_count.max(1) as f64
    }

    /// The most the powered units may draw: the grid's output over 1.2, so E-mode runs at its
    /// fixed 120%. (Every draw is a multiple of 15 W, so e.g. 2000 W allows 1665 W in practice.)
    pub fn boost_threshold(self) -> f64 {
        self.capacity() / E_MODE_EFFICIENCY
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

/// Stable internal codes used only by the UI's counterfactual E-mode analysis.
pub fn electric_facility_code(facility: &str) -> u8 {
    match facility {
        "Mine" => 1,
        "Well" => 2,
        "Carousel Mill" => 3,
        "Crafting Table" => 4,
        "Claw Game Cooker" => 5,
        "Jukebox Dryer" => 6,
        "Simmering Pot" => 7,
        "Phonolfactory Table" => 8,
        "Bouncy Brew Keg" => 9,
        "Blazing Stove" => 10,
        "Pickling Jar" => 11,
        "Joy Wheel Loom" => 12,
        "Woodworking Bench" => 13,
        "Chimney Kiln" => 14,
        "Dance Pad Polisher" => 15,
        "Aniipod Maker" => 16,
        _ => 0,
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

        // These two special facilities draw 30 / 60 / 90 W at Lv.1-3 (external sources agree).
        "Dance Pad Polisher" | "Aniipod Maker" => Some(ElectricFacilityRule::new(30.0, 30.0)),
        _ => None,
    }
}

pub fn electric_require(facility: &str, level: u32) -> Option<f64> {
    if level == 0 || !electric_compatible(facility) { return None; }
    electric_rule(facility).map(|rule| rule.require(level))
}

/// The building that staffs the shared grid, as named in a plan's staffing rows.
pub const GENERATOR_BUILDING: &str = "Crackle Generator";

/// The Aniimo ability the Crackle Generator's worker needs.
pub const GENERATOR_ABILITY: &str = "Lightning";

/// Whether one kind of roster Aniimo can staff the Crackle Generator: at least one of it, with
/// the Lightning ability at any level.
pub fn can_staff_generator(aniimo: &crate::models::RosterAniimo) -> bool {
    aniimo.count > 0 && aniimo.level(GENERATOR_ABILITY) > 0
}

/// Whether anyone on `crew` can staff the Crackle Generator. The current model needs one
/// full-time Lightning worker for the whole grid, however many generators it has.
pub fn crew_can_staff_generator(crew: &crate::models::Crew) -> bool {
    crew.members.iter().any(can_staff_generator)
}

/// The error a roster plan with E-mode on gets when nobody on the roster has Lightning.
pub fn missing_generator_staff_error() -> String {
    format!(
        "E-mode needs a {GENERATOR_ABILITY} Aniimo to staff the {GENERATOR_BUILDING}, and no Aniimo on this roster has {GENERATOR_ABILITY}."
    )
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

/// How many Crackle Generators the homeland has at an RV level: one at RV12-14, two at RV15-17,
/// three from RV18. (Below RV12 E-mode is off; the count is kept at one.)
pub fn generator_count_for_rv(rv: u32) -> u8 {
    match rv {
        0..=14 => 1,
        15..=17 => 2,
        _ => 3,
    }
}

/// One Crackle Generator's output by its level (Lv.1..=Lv.5), in watts.
pub const GENERATOR_UNIT_CAPACITY: [f64; 5] = [600.0, 800.0, 1000.0, 1200.0, 1500.0];

/// E-mode's fixed efficiency: powered units run at 120%, and the grid may only draw up to its
/// output divided by this.
pub const E_MODE_EFFICIENCY: f64 = 1.2;
