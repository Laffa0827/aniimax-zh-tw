// Crackle Generator E-mode configuration.
// Electrical demand is modeled per facility as its own Lv.1 base power plus a fixed
// per-level increment. Do not assume every facility starts at the same wattage.

export const ELECTRIC_INCOMPATIBLE = Object.freeze([
    'Farmland',
    'Woodland',
    'Tidewhisper Sandcastle',
    'Dewy House',
    'Nimbus Bed',
    'Starfall Hammock',
    'Floral Windmill',
    'Heat Furnace',
    'Cooling Unit',
    'Sunlamp',
]);

export function electricCompatible(facility) {
    return !ELECTRIC_INCOMPATIBLE.includes(facility);
}

// basePower is the facility's Lv.1 demand; powerPerLevel is the increase for each
// additional facility level. These are explicit per-facility rules so individual bases
// can be corrected later without changing the solver model.
export const ELECTRIC_FACILITY_RULES = Object.freeze({
    'Mine': { basePower: 30, powerPerLevel: 30 },
    'Well': { basePower: 30, powerPerLevel: 30 },

    'Carousel Mill': { basePower: 15, powerPerLevel: 15 },
    'Crafting Table': { basePower: 15, powerPerLevel: 15 },
    'Claw Game Cooker': { basePower: 15, powerPerLevel: 15 },
    'Jukebox Dryer': { basePower: 15, powerPerLevel: 15 },
    'Simmering Pot': { basePower: 15, powerPerLevel: 15 },
    'Phonolfactory Table': { basePower: 15, powerPerLevel: 15 },
    'Bouncy Brew Keg': { basePower: 15, powerPerLevel: 15 },
    'Blazing Stove': { basePower: 15, powerPerLevel: 15 },
    'Pickling Jar': { basePower: 15, powerPerLevel: 15 },
    'Joy Wheel Loom': { basePower: 15, powerPerLevel: 15 },
    'Woodworking Bench': { basePower: 15, powerPerLevel: 15 },
    'Chimney Kiln': { basePower: 15, powerPerLevel: 15 },

    'Dance Pad Polisher': { basePower: 30, powerPerLevel: 15 },
    'Aniipod Maker': { basePower: 30, powerPerLevel: 15 },
});

export function electricRequire(facility, level) {
    if (!electricCompatible(facility) || !ELECTRIC_FACILITY_RULES[facility] || level < 1) return null;
    const rule = ELECTRIC_FACILITY_RULES[facility];
    return rule.basePower + (Number(level) - 1) * rule.powerPerLevel;
}

export const ELECTRIC_GENERATOR = Object.freeze({
    capacity: Object.freeze([600, 800, 1000, 1200, 1500]),
    boostThreshold: Object.freeze([500, 660, 800, 1000, 1200]),
});

export function generatorLevelForRv(rv) {
    rv = Number(rv) || 1;
    if (rv <= 13) return 1;
    if (rv <= 15) return 2;
    if (rv <= 17) return 3;
    if (rv <= 19) return 4;
    return 5;
}


// Verified in-game: the relay-pole hard cap is determined by Homeland/RV level.
// RV 12-13: 6, 14-15: 12, 16-17: 18, 18-19: 24, 20: 30.
export const RELAY_POLE_CAP_BY_RV = Object.freeze({
    12: 6, 13: 6,
    14: 12, 15: 12,
    16: 18, 17: 18,
    18: 24, 19: 24,
    20: 30,
});

export function relayPoleCapForRv(rv) {
    const level = Math.max(1, Math.min(20, Number(rv) || 1));
    if (level < 12) return 0;
    return RELAY_POLE_CAP_BY_RV[level] ?? 30;
}

export function generatorLabel(level) {
    const i = Math.max(1, Math.min(5, Number(level) || 1)) - 1;
    return `Lv.${i + 1} · ${ELECTRIC_GENERATOR.capacity[i]}W · 120% Boost 門檻 ${ELECTRIC_GENERATOR.boostThreshold[i]}W`;
}
