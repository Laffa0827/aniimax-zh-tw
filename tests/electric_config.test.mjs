import assert from 'node:assert/strict';
import test from 'node:test';

import {
    electricRequire, generatorCountForRv, generatorLabel, generatorLevelForRv, generatorUnitCapacity, gridCapacity, gridPowerLimit,
    normalizeElectricStrategy, ELECTRIC_STRATEGY, ELECTRIC_STRATEGY_LABEL,
    normalizeElectricAllocation, normalizeManualUnits, manualUnitCap, electricAllocationInput, migrateElectricAllocation,
    MANUAL_BEST, normalizeManualChoice,
} from '../web/electric-config.js';

// The same table as `grid_capacity_and_limit_by_rv` in tests/electric_tests.rs: the page's settings
// hint must agree with the planner's grid (src/electric.rs).
const GRID_BY_RV = [
    [12, 1, 1, 600], [13, 1, 1, 600], [14, 2, 1, 800],
    [15, 2, 2, 1600], [16, 3, 2, 2000], [17, 3, 2, 2000],
    [18, 4, 3, 3600], [19, 4, 3, 3600], [20, 5, 3, 4500],
];

test('test_grid_matches_the_planner_at_every_rv', () => {
    for (const [rv, level, count, capacity] of GRID_BY_RV) {
        assert.equal(generatorLevelForRv(rv), level, `RV${rv} level`);
        assert.equal(generatorCountForRv(rv), count, `RV${rv} count`);
        assert.equal(gridCapacity(level, count), capacity, `RV${rv} capacity`);
        assert.ok(Math.abs(gridPowerLimit(level, count) - capacity / 1.2) < 1e-9, `RV${rv} limit`);
    }
});

test('test_rv16_grid_and_its_15w_limit', () => {
    const level = generatorLevelForRv(16);
    const count = generatorCountForRv(16);
    assert.equal(generatorUnitCapacity(level), 1000);
    assert.equal(gridCapacity(level, count), 2000);
    assert.ok(Math.abs(gridPowerLimit(level, count) - 1666.67) < 0.01);
    assert.equal(Math.floor(gridPowerLimit(level, count) / 15) * 15, 1665);
    assert.match(generatorLabel(level, count), /Lv\.3 × 2 台 · 單台 1000W · 總容量 2000W · 耗電上限 1666\.67W/);
});

test('test_single_generator_limits_are_output_over_1_2', () => {
    assert.ok(Math.abs(gridPowerLimit(2, 1) - 666.67) < 0.01);
    assert.ok(Math.abs(gridPowerLimit(3, 1) - 833.33) < 0.01);
    assert.equal(gridPowerLimit(5, 1), 1250);
});

test('test_special_facilities_draw_30_60_90', () => {
    for (const facility of ['Dance Pad Polisher', 'Aniipod Maker']) {
        assert.deepEqual([1, 2, 3].map(level => electricRequire(facility, level)), [30, 60, 90], facility);
    }
    assert.equal(electricRequire('Woodworking Bench', 4), 60);
    assert.equal(electricRequire('Chimney Kiln', 4), 60);
});

// The strategy is always combined: whatever an older saved setup or share link carries.
test('test_old_strategy_values_become_combined', () => {
    assert.equal(ELECTRIC_STRATEGY, 'combined');
    assert.equal(ELECTRIC_STRATEGY_LABEL, '綜合');
    for (const saved of ['raw', 'workstations', 'processing', 'combined', '', undefined, null, 'anything']) {
        assert.equal(normalizeElectricStrategy(saved), 'combined', String(saved));
    }
});

// The Mine/Well allocation defaults to auto: no fixed counts reach the planner, whatever counts the
// page still holds. Manual sends each ticked facility's count, and null for one left unticked.
test('test_allocation_defaults_to_auto', () => {
    for (const saved of [undefined, null, '', 'auto', 'raw', 'Manual']) {
        assert.equal(normalizeElectricAllocation(saved), 'auto', String(saved));
        assert.deepEqual(electricAllocationInput(saved, '3', '2'),
            { electric_allocation: 'auto', electric_mine_units: null, electric_well_units: null, electric_mine_best: false, electric_well_best: false }, String(saved));
    }
    assert.deepEqual(electricAllocationInput('manual', '3', null),
        { electric_allocation: 'manual', electric_mine_units: 3, electric_well_units: null, electric_mine_best: false, electric_well_best: false });
    assert.deepEqual(electricAllocationInput('manual', '0', 'x'),
        { electric_allocation: 'manual', electric_mine_units: null, electric_well_units: null, electric_mine_best: false, electric_well_best: false });
});

// A manual count goes up to what's owned, and to what fits the grid's limit on its own: RV16's grid
// (1666.67 W) holds all of eight Lv.5 Mines (150 W) and three Lv.4 Wells (120 W) but only eleven
// of twelve Lv.5 Mines; one Lv.1 generator (500 W) holds four Lv.4 Mines. None owned, none offered.
test('test_manual_caps_follow_rv_and_owned_units', () => {
    const [level, count] = [generatorLevelForRv(16), generatorCountForRv(16)];
    assert.equal(manualUnitCap('Mine', [{ count: 8, level: 5 }], level, count), 8);
    assert.equal(manualUnitCap('Well', [{ count: 3, level: 4 }], level, count), 3);
    assert.equal(manualUnitCap('Mine', [{ count: 12, level: 5 }], level, count), 11);
    assert.equal(manualUnitCap('Mine', [{ count: 6, level: 4 }], 1, 1), 4);
    // Mixed tiers draw at the highest owned level, as the planner prices them.
    assert.equal(manualUnitCap('Mine', [{ count: 4, level: 2 }, { count: 4, level: 5 }], 1, 1), 3);
    assert.equal(manualUnitCap('Mine', [{ count: 0, level: 5 }], level, count), 0);
    assert.equal(manualUnitCap('Mine', [], level, count), 0);
});

// Saves from before the choice existed come back as auto; unclear values are made safe, and a
// valid manual setup is kept as it was.
test('test_saved_allocation_migrates_safely', () => {
    const old = migrateElectricAllocation({ 'electric-enabled': true });
    assert.equal(old['electric-allocation-auto'], true);
    assert.equal(old['electric-allocation-manual'], false);
    assert.equal(old['electric-enabled'], true);
    const odd = migrateElectricAllocation({
        'electric-allocation-manual': 'yes', 'electric-mine-allowed': 'on', 'electric-mine-units': '2.5', 'electric-well-units': '-1',
    });
    assert.equal(odd['electric-allocation-auto'], true);
    assert.equal(odd['electric-mine-allowed'], false);
    assert.ok(!('electric-mine-units' in odd) && !('electric-well-units' in odd));
    const kept = migrateElectricAllocation({
        'electric-allocation-auto': false, 'electric-allocation-manual': true, 'electric-mine-allowed': true, 'electric-mine-units': '3',
    });
    assert.deepEqual(kept, {
        'electric-allocation-auto': false, 'electric-allocation-manual': true, 'electric-mine-allowed': true, 'electric-mine-units': '3',
    });
    assert.equal(normalizeManualUnits('3'), 3);
    assert.equal(normalizeManualUnits(0), null);
});

// Manual mode's "最佳" (best): the facility is allowed power and the planner decides how many, so
// no count goes with it; off sends nothing at all (none powered); a count is sent as it is. Each
// facility is chosen on its own.
test('test_best_choice_reaches_the_planner_as_best', () => {
    const input = (mine, well) => electricAllocationInput('manual', mine, well);
    assert.deepEqual(input(MANUAL_BEST, null),
        { electric_allocation: 'manual', electric_mine_units: null, electric_well_units: null, electric_mine_best: true, electric_well_best: false });
    assert.deepEqual(input(null, 'best'),
        { electric_allocation: 'manual', electric_mine_units: null, electric_well_units: null, electric_mine_best: false, electric_well_best: true });
    assert.deepEqual(input('best', 'best'),
        { electric_allocation: 'manual', electric_mine_units: null, electric_well_units: null, electric_mine_best: true, electric_well_best: true });
    assert.deepEqual(input('3', 'best'),
        { electric_allocation: 'manual', electric_mine_units: 3, electric_well_units: null, electric_mine_best: false, electric_well_best: true });
    assert.deepEqual(input('best', '2'),
        { electric_allocation: 'manual', electric_mine_units: null, electric_well_units: 2, electric_mine_best: true, electric_well_best: false });
    // Auto mode fixes nothing, "best" or not.
    assert.deepEqual(electricAllocationInput('auto', 'best', '2'),
        { electric_allocation: 'auto', electric_mine_units: null, electric_well_units: null, electric_mine_best: false, electric_well_best: false });
    // Only the exact choice counts as best.
    for (const odd of ['Best', '最佳', 'optimal', '']) assert.equal(normalizeManualChoice(odd), null, odd);
    assert.equal(normalizeManualChoice('best'), 'best');
    assert.equal(normalizeManualChoice('4'), 4);
});

// A saved "best" comes back as it was; saves from before it existed keep their counts.
test('test_saved_best_choice_migrates', () => {
    const saved = migrateElectricAllocation({
        'electric-allocation-manual': true, 'electric-mine-allowed': true, 'electric-mine-units': 'best',
        'electric-well-allowed': true, 'electric-well-units': '2',
    });
    assert.equal(saved['electric-mine-units'], 'best');
    assert.equal(saved['electric-well-units'], '2');
    assert.equal(saved['electric-allocation-manual'], true);
    const odd = migrateElectricAllocation({ 'electric-allocation-manual': true, 'electric-mine-units': 'Best', 'electric-well-units': 'optimal' });
    assert.ok(!('electric-mine-units' in odd) && !('electric-well-units' in odd));
});
