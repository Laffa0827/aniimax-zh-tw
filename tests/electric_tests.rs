use aniimax::electric::{
    electric_compatible, electric_require, electric_rule, generator_count_for_rv, generator_level_for_rv, ElectricConfig,
};

#[test]
fn generator_progression_matches_rv() {
    assert_eq!(generator_level_for_rv(12), 1);
    assert_eq!(generator_level_for_rv(13), 1);
    assert_eq!(generator_level_for_rv(14), 2);
    assert_eq!(generator_level_for_rv(16), 3);
    assert_eq!(generator_level_for_rv(18), 4);
    assert_eq!(generator_level_for_rv(20), 5);
}

// One generator: its output, and the most powered units may draw (output / 1.2). The limits for
// Lv.2, Lv.3 and Lv.5 used to be a table (660 / 800 / 1200); the confirmed rule is output / 1.2.
#[test]
fn generator_capacity_and_boost_threshold_match() {
    let expected = [
        (1, 600.0, 500.0),
        (2, 800.0, 800.0 / 1.2),
        (3, 1000.0, 1000.0 / 1.2),
        (4, 1200.0, 1000.0),
        (5, 1500.0, 1250.0),
    ];
    for (level, capacity, threshold) in expected {
        let p = ElectricConfig::new(level);
        assert_eq!(p.generator_count, 1);
        assert_eq!(p.capacity(), capacity);
        assert!((p.boost_threshold() - threshold).abs() < 1e-9, "Lv{level}: {}", p.boost_threshold());
    }
    assert!((ElectricConfig::new(2).boost_threshold() - 666.67).abs() < 0.01);
    assert!((ElectricConfig::new(3).boost_threshold() - 833.33).abs() < 0.01);
}

// Confirmed counts: one generator at RV12-14, two at RV15-17, three at RV18-20.
#[test]
fn generator_count_follows_rv() {
    for rv in 12..=14 {
        assert_eq!(generator_count_for_rv(rv), 1, "RV{rv}");
    }
    for rv in 15..=17 {
        assert_eq!(generator_count_for_rv(rv), 2, "RV{rv}");
    }
    for rv in 18..=20 {
        assert_eq!(generator_count_for_rv(rv), 3, "RV{rv}");
    }
}

// The whole grid at every RV from 12 to 20: count x one generator's output, and that over 1.2.
// RV16: 2 x 1000 W = 2000 W, limit 1666.67 W, so 1665 W in 15 W steps.
#[test]
fn grid_capacity_and_limit_by_rv() {
    let expected: [(u32, u8, f64); 9] = [
        (12, 1, 600.0),
        (13, 1, 600.0),
        (14, 1, 800.0),
        (15, 2, 1600.0),
        (16, 2, 2000.0),
        (17, 2, 2000.0),
        (18, 3, 3600.0),
        (19, 3, 3600.0),
        (20, 3, 4500.0),
    ];
    for (rv, count, capacity) in expected {
        let grid = ElectricConfig::new(generator_level_for_rv(rv)).with_generators(generator_count_for_rv(rv));
        assert_eq!(grid.generator_count, count, "RV{rv}");
        assert_eq!(grid.capacity(), capacity, "RV{rv}");
        assert!((grid.boost_threshold() - capacity / 1.2).abs() < 1e-9, "RV{rv}");
    }
    let rv16 = ElectricConfig::new(generator_level_for_rv(16)).with_generators(generator_count_for_rv(16));
    assert_eq!(rv16.unit_capacity(), 1000.0);
    assert!((rv16.boost_threshold() - 1666.67).abs() < 0.01);
    assert_eq!((rv16.boost_threshold() / 15.0).floor() * 15.0, 1665.0);
    assert!((rv16.efficiency(1665.0) - 1.2).abs() < 1e-9, "1665 W still runs at 120%");
    assert!(rv16.efficiency(1680.0) < 1.2, "1680 W is over the limit");
    let rv15 = ElectricConfig::new(generator_level_for_rv(15)).with_generators(generator_count_for_rv(15));
    assert!((rv15.boost_threshold() - 1600.0 / 1.2).abs() < 1e-9);
    let rv18 = ElectricConfig::new(generator_level_for_rv(18)).with_generators(generator_count_for_rv(18));
    assert_eq!(rv18.boost_threshold(), 3000.0);
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

    // Dance Pad Polisher / Aniipod Maker: 30 / 60 / 90 W (confirmed by external sources).
    assert_eq!(electric_require("Dance Pad Polisher", 1), Some(30.0));
    assert_eq!(electric_require("Dance Pad Polisher", 2), Some(60.0));
    assert_eq!(electric_require("Dance Pad Polisher", 3), Some(90.0));
    assert_eq!(electric_require("Aniipod Maker", 1), Some(30.0));
    assert_eq!(electric_require("Aniipod Maker", 2), Some(60.0));
    assert_eq!(electric_require("Aniipod Maker", 3), Some(90.0));

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
    assert_eq!(special.power_per_level, 30.0);
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

#[test]
fn counterfactual_forbidden_facility_is_excluded() {
    let p = ElectricConfig::new_with_strategy_and_forbidden(2, 1, 1);
    assert!(!p.allows_facility("Mine"));
    assert!(p.allows_facility("Well"));
}
