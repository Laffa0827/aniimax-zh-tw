import assert from 'node:assert/strict';
import test from 'node:test';

import { simpleSetup, SEASON } from '../web/facility-config.js';

// RV16 has two Nimbus Beds (confirmed by the player), at Lv.3, where Petals unlock; the bed count
// doesn't change anything else.
test('test_rv16_has_two_nimbus_beds_at_level_3', () => {
    assert.deepEqual(simpleSetup(15).facilities['Nimbus Bed'], [{ count: 1, level: 2 }]);
    assert.deepEqual(simpleSetup(16).facilities['Nimbus Bed'], [{ count: 2, level: 3 }]);
    assert.deepEqual(simpleSetup(17).facilities['Nimbus Bed'], [{ count: 2, level: 3 }]);
    assert.deepEqual(simpleSetup(18).facilities['Nimbus Bed'], [{ count: 2, level: 3 }]);
});

// While the Harvest Moon Festival is on, its two processing candidates are weighed without a
// Recipe Notes tick; the other two Recipe Notes recipes still need theirs.
test('test_festival_candidates_are_not_locked_behind_recipe_notes', () => {
    assert.deepEqual(SEASON.candidates.map(r => r.name).sort(), ['harvest_platter', 'umbral_sweet_and_spicy_sauce']);
    const notes = SEASON.recipeNotes.map(r => r.name);
    for (const candidate of SEASON.candidates) assert.ok(!notes.includes(candidate.name), candidate.name);
    assert.deepEqual(notes.sort(), ['umbral_hot_pot', 'umbral_pickle']);
});
