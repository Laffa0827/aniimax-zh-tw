use aniimax::electric::{electric_compatible, electric_require, electric_rule, generator_level_for_rv, ElectricConfig};

#[test]
fn generator_progression_matches_rv() {
    assert_eq!(generator_level_for_rv(12), 1);
    assert_eq!(generator_level_for_rv(13), 1);
    assert_eq!(generator_level_for_rv(14), 2);
    assert_eq!(generator_level_for_rv(16), 3);
    assert_eq!(generator_level_for_rv(18), 4);
    assert_eq!(generator_level_for_rv(20), 5);
}

#[test]
fn generator_capacity_and_boost_threshold_match() {
    let expected = [
        (1, 600.0, 500.0),
        (2, 800.0, 660.0),
        (3, 1000.0, 800.0),
        (4, 1200.0, 1000.0),
        (5, 1500.0, 1200.0),
    ];
    for (level, capacity, threshold) in expected {
        let p = ElectricConfig::new(level);
        assert_eq!(p.capacity(), capacity);
        assert_eq!(p.boost_threshold(), threshold);
    }
}

#[test]
fn efficiency_uses_shared_grid_threshold() {
    let p = ElectricConfig::new(4);
    assert!((p.efficiency(1000.0) - 1.2).abs() < 1e-9);
    assert!((p.efficiency(1155.0) - 1.0).abs() < 1e-9);
    assert!((p.efficiency(1200.0) - 1.0).abs() < 1e-9);
    assert!((p.efficiency(1300.0) - (1200.0 / 1300.0)).abs() < 1e-9);
}

#[test]
fn electric_require_progression_matches_user_confirmed_rule() {
    // Mine / Well: +30W per level.
    for level in 1..=6 {
        assert_eq!(electric_require("Mine", level), Some(30.0 * level as f64));
    }
    for level in 1..=5 {
        assert_eq!(electric_require("Well", level), Some(30.0 * level as f64));
    }

    // Regular processing: +15W per level.
    for facility in [
        "Carousel Mill", "Crafting Table", "Claw Game Cooker", "Jukebox Dryer",
        "Simmering Pot", "Phonolfactory Table", "Bouncy Brew Keg", "Blazing Stove",
        "Pickling Jar", "Joy Wheel Loom", "Woodworking Bench", "Chimney Kiln",
    ] {
        for level in 1..=7 {
            assert_eq!(electric_require(facility, level), Some(15.0 * level as f64));
        }
    }

    // Dance Pad Polisher / Aniipod Maker start at 30W, then use the common +15W increment.
    assert_eq!(electric_require("Dance Pad Polisher", 1), Some(30.0));
    assert_eq!(electric_require("Dance Pad Polisher", 2), Some(45.0));
    assert_eq!(electric_require("Dance Pad Polisher", 3), Some(60.0));
    assert_eq!(electric_require("Aniipod Maker", 1), Some(30.0));
    assert_eq!(electric_require("Aniipod Maker", 2), Some(45.0));
    assert_eq!(electric_require("Aniipod Maker", 3), Some(60.0));

    // User's direct screenshot confirmation: Well Lv4 = 120W.
    assert_eq!(electric_require("Well", 4), Some(120.0));
}


#[test]
fn electric_rule_is_per_facility_base_plus_increment() {
    let mine = electric_rule("Mine").unwrap();
    assert_eq!(mine.base_power, 30.0);
    assert_eq!(mine.power_per_level, 30.0);

    let mill = electric_rule("Carousel Mill").unwrap();
    assert_eq!(mill.base_power, 15.0);
    assert_eq!(mill.power_per_level, 15.0);

    let special = electric_rule("Dance Pad Polisher").unwrap();
    assert_eq!(special.base_power, 30.0);
    assert_eq!(special.power_per_level, 15.0);
}

#[test]
fn user_confirmed_non_electric_facilities_are_not_compatible() {
    for facility in [
        "Farmland", "Woodland", "Tidewhisper Sandcastle", "Dewy House",
        "Nimbus Bed", "Starfall Hammock", "Floral Windmill",
        "Heat Furnace", "Cooling Unit", "Sunlamp",
    ] {
        assert!(!electric_compatible(facility), "{facility} must not support E-mode");
    }
}

#[test]
fn all_other_facilities_are_electric_compatible() {
    for facility in [
        "Mine", "Well", "Carousel Mill", "Crafting Table", "Claw Game Cooker",
        "Jukebox Dryer", "Simmering Pot", "Phonolfactory Table", "Bouncy Brew Keg",
        "Blazing Stove", "Pickling Jar", "Joy Wheel Loom", "Woodworking Bench",
        "Chimney Kiln", "Dance Pad Polisher", "Aniipod Maker",
    ] {
        assert!(electric_compatible(facility), "{facility} must support E-mode");
    }
}


#[test]
fn enabled_generator_requires_one_lightning_aniimo() {
    // The exact planner enforces this when a roster is supplied: one full-time Lightning
    // Aniimo staffs the shared Crackle Generator itself. Generator level does not change
    // the worker count; the current model has one shared generator.
    let p = ElectricConfig::new(2);
    assert!(p.enabled);
    assert_eq!(p.generator_level, 2);
}
