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

// E-mode's strategy is always combined (Mine, Well and processing facilities alike), as in
// `src/wasm.rs` (`ELECTRIC_STRATEGY`). Older saved setups and share links may still carry `raw`,
// `workstations` or anything else; they all mean this one now.
export const ELECTRIC_STRATEGY = 'combined';
export const ELECTRIC_STRATEGY_LABEL = '綜合';

export function normalizeElectricStrategy(_saved) {
    return ELECTRIC_STRATEGY;
}

// How E-mode's power is shared out, as `JsPlanInput::electric_allocation` in `src/wasm.rs` reads it:
// 'auto' (the default) leaves every facility to the planner; 'manual' keeps the player's own Mine
// and Well counts exactly. A save from before this choice existed, or anything else, means 'auto'.
export const ELECTRIC_ALLOCATION_AUTO = 'auto';
export const ELECTRIC_ALLOCATION_MANUAL = 'manual';
export const MANUAL_ELECTRIC_FACILITIES = Object.freeze(['Mine', 'Well']);

export function normalizeElectricAllocation(saved) {
    return saved === ELECTRIC_ALLOCATION_MANUAL ? ELECTRIC_ALLOCATION_MANUAL : ELECTRIC_ALLOCATION_AUTO;
}

// A saved manual count read back: a whole number of at least 1, or null.
export function normalizeManualUnits(saved) {
    const units = Number(saved);
    return Number.isInteger(units) && units >= 1 ? units : null;
}

// A manual facility's "最佳" (best) choice: allowed power, and the planner decides how many.
export const MANUAL_BEST = 'best';

// A saved manual choice read back: MANUAL_BEST, a whole number of at least 1, or null.
export function normalizeManualChoice(saved) {
    return saved === MANUAL_BEST ? MANUAL_BEST : normalizeManualUnits(saved);
}

// The most powered units manual mode offers for `facility` (Mine or Well), from its owned tiers
// (`[{ count, level }]`): every owned unit, as far as the grid's limit holds them on their own. A
// powered unit draws at the facility's highest owned level, as the planner prices it.
export function manualUnitCap(facility, tiers, generatorLevel, generatorCount) {
    const owned = (tiers || []).filter(t => Number(t.count) > 0);
    const count = owned.reduce((sum, t) => sum + Number(t.count), 0);
    if (count === 0) return 0;
    const watts = electricRequire(facility, Math.max(...owned.map(t => Number(t.level) || 1)));
    if (!watts) return 0;
    return Math.min(count, Math.floor(gridPowerLimit(generatorLevel, generatorCount) / watts + 1e-9));
}

// A saved setup's (or share link's) allocation fields, as the page's inputs keep them, made safe in
// place: a save from before the choice existed, or anything unclear, is auto (the default); the
// switches are true or false; a saved choice is kept only as "best" or a whole number of at least 1.
export function migrateElectricAllocation(data) {
    if (!data || typeof data !== 'object') return data;
    const manual = data['electric-allocation-manual'] === true;
    data['electric-allocation-manual'] = manual;
    data['electric-allocation-auto'] = !manual;
    for (const key of ['mine', 'well']) {
        const allowed = `electric-${key}-allowed`;
        if (allowed in data) data[allowed] = data[allowed] === true;
        const units = `electric-${key}-units`;
        if (units in data && normalizeManualChoice(data[units]) === null) delete data[units];
    }
    return data;
}

// The planner's input for the allocation, as `JsPlanInput` in src/wasm.rs reads it. In manual mode
// each facility's choice: a count, kept exactly; MANUAL_BEST, left to the planner
// (`electric_*_best`); or null when it isn't allowed power (the planner then powers none of it).
// In auto mode, nothing fixed at all.
export function electricAllocationInput(mode, mineChoice, wellChoice) {
    const manual = normalizeElectricAllocation(mode) === ELECTRIC_ALLOCATION_MANUAL;
    const [mine, well] = [mineChoice, wellChoice].map(c => manual ? normalizeManualChoice(c) : null);
    return {
        electric_allocation: manual ? ELECTRIC_ALLOCATION_MANUAL : ELECTRIC_ALLOCATION_AUTO,
        electric_mine_units: mine === MANUAL_BEST ? null : mine,
        electric_well_units: well === MANUAL_BEST ? null : well,
        electric_mine_best: mine === MANUAL_BEST,
        electric_well_best: well === MANUAL_BEST,
    };
}

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

    'Dance Pad Polisher': { basePower: 30, powerPerLevel: 30 },
    'Aniipod Maker': { basePower: 30, powerPerLevel: 30 },
});

export function electricRequire(facility, level) {
    if (!electricCompatible(facility) || !ELECTRIC_FACILITY_RULES[facility] || level < 1) return null;
    const rule = ELECTRIC_FACILITY_RULES[facility];
    return rule.basePower + (Number(level) - 1) * rule.powerPerLevel;
}

// The same rules as `src/electric.rs` (`GENERATOR_UNIT_CAPACITY`, `generator_count_for_rv`,
// `E_MODE_EFFICIENCY`): one generator's output by level, how many generators an RV level has, and
// E-mode's fixed 120%. The grid's output is count x unit capacity; the powered units may draw up to
// that over 1.2. The plan's own summary comes from the Rust side; these are for the settings hint.
export const ELECTRIC_GENERATOR = Object.freeze({
    unitCapacity: Object.freeze([600, 800, 1000, 1200, 1500]),
    efficiency: 1.2,
});

export function generatorLevelForRv(rv) {
    rv = Number(rv) || 1;
    if (rv <= 13) return 1;
    if (rv <= 15) return 2;
    if (rv <= 17) return 3;
    if (rv <= 19) return 4;
    return 5;
}

// One generator at RV12-14, two at RV15-17, three from RV18.
export function generatorCountForRv(rv) {
    rv = Number(rv) || 1;
    if (rv <= 14) return 1;
    if (rv <= 17) return 2;
    return 3;
}

export function generatorUnitCapacity(level) {
    return ELECTRIC_GENERATOR.unitCapacity[Math.max(1, Math.min(5, Number(level) || 1)) - 1];
}

export function gridCapacity(level, count) {
    return generatorUnitCapacity(level) * Math.max(1, Number(count) || 1);
}

export function gridPowerLimit(level, count) {
    return gridCapacity(level, count) / ELECTRIC_GENERATOR.efficiency;
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

export function generatorLabel(level, count = 1) {
    const lv = Math.max(1, Math.min(5, Number(level) || 1));
    const n = Math.max(1, Number(count) || 1);
    const limit = Math.round(gridPowerLimit(lv, n) * 100) / 100;
    return `Lv.${lv} × ${n} 台 · 單台 ${generatorUnitCapacity(lv)}W · 總容量 ${gridCapacity(lv, n)}W · 耗電上限 ${limit}W（E-MODE 120%）`;
}
