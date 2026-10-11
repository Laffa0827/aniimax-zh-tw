//! Phase-0 test harness for the Crackle Generator E-mode model and its independent re-check.
//!
//! `solve_exact` always builds its model without electricity (`exact.rs`, `solve_exact` passes
//! `None` to `build_model`), so these tests reach the electric model the way the web page does:
//! `write_lp_with_power` writes the model as CPLEX LP text, a small parser here hands it to
//! `microlp` (playing HiGHS's role from `web/worker.js`), and `plan_from_values_with_power` /
//! `check_plan_with_power` turn the answer into a plan and re-check it. The WASM tests go one step
//! further and call `aniimax::wasm::exact_problem` / `exact_plan` with the page's JSON.
//!
//! TEST FAKE DATA: every facility count, facility level, generator level and roster below is
//! made up to exercise the model. None of it is game data. Wattages, recipes and timings come from
//! the repository's own `electric_rule` and `data/*.csv`, not from these tests.
//!
//! Tests marked `#[ignore = "KNOWN BUG ..."]` assert the behaviour expected AFTER the fix and
//! currently fail; run them with `cargo test --test electric_exact_tests -- --ignored` to
//! reproduce each bug. Do not flip their assertions to match today's output.

use aniimax::data::{load_aniimo_requirements, load_all_data, load_grower_steps};
use aniimax::electric::{electric_require, ElectricConfig};
use aniimax::exact::{
    check_plan_with_power, plan_from_values_with_power, solve_exact, write_lp_with_power, ExactPlan, Goal, LpProblem,
};
use aniimax::models::{Crew, FacilityCounts, ModuleLevels, ProductionItem, RosterAniimo};
use microlp::{ComparisonOp, OptimizationDirection, Problem};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

fn load_items() -> Option<Vec<ProductionItem>> {
    let data_dir = Path::new("data");
    data_dir.exists().then(|| load_all_data(data_dir).expect("Failed to load data"))
}

/// Solves `write_lp_with_power`'s CPLEX LP text with microlp: `(values in x0.. order, objective)`.
/// `Err` when the text holds a constraint the model can never meet (an empty left-hand side with
/// an unmet right-hand side) or the solver finds no solution.
fn solve_lp(problem: &LpProblem) -> Result<(Vec<f64>, f64), String> {
    let n = problem.variables;
    let mut objective = vec![0.0; n];
    let mut bounds = vec![(0.0, f64::INFINITY); n];
    let mut integer = vec![false; n];
    let mut constraints: Vec<(Vec<(usize, f64)>, ComparisonOp, f64)> = Vec::new();

    // `+ 1.5 x3 - 2 x7` -> [(3, 1.5), (7, -2.0)]
    let parse_terms = |text: &str| -> Vec<(usize, f64)> {
        let tokens: Vec<&str> = text.split_whitespace().collect();
        tokens
            .chunks(3)
            .map(|t| {
                let sign = if t[0] == "-" { -1.0 } else { 1.0 };
                let coeff: f64 = t[1].parse().expect("coefficient");
                let var: usize = t[2].trim_start_matches('x').parse().expect("variable");
                (var, sign * coeff)
            })
            .collect()
    };

    let mut section = "";
    for line in problem.lp.lines() {
        let trimmed = line.trim();
        match trimmed {
            "Maximize" | "Subject To" | "Bounds" | "General" | "End" => {
                section = trimmed;
                continue;
            }
            "" => continue,
            _ => {}
        }
        match section {
            "Maximize" => {
                let body = trimmed.strip_prefix("obj:").expect("objective line");
                for (v, c) in parse_terms(body) {
                    objective[v] += c;
                }
            }
            "Subject To" => {
                let (_, body) = trimmed.split_once(':').expect("named constraint");
                let (lhs, op, rhs) = if let Some((l, r)) = body.split_once("<=") {
                    (l, ComparisonOp::Le, r)
                } else if let Some((l, r)) = body.split_once(">=") {
                    (l, ComparisonOp::Ge, r)
                } else {
                    let (l, r) = body.split_once('=').expect("comparison");
                    (l, ComparisonOp::Eq, r)
                };
                let rhs: f64 = rhs.trim().parse().expect("rhs");
                let terms = parse_terms(lhs);
                if terms.is_empty() {
                    let holds = match op {
                        ComparisonOp::Le => 0.0 <= rhs,
                        ComparisonOp::Ge => 0.0 >= rhs,
                        ComparisonOp::Eq => rhs == 0.0,
                    };
                    if !holds {
                        return Err(format!("empty constraint `{trimmed}` can never hold"));
                    }
                    continue;
                }
                constraints.push((terms, op, rhs));
            }
            "Bounds" => {
                if let Some((lo, rest)) = trimmed.split_once("<=") {
                    let (var, hi) = rest.split_once("<=").expect("double bound");
                    let v: usize = var.trim().trim_start_matches('x').parse().expect("variable");
                    bounds[v] = (lo.trim().parse().expect("lo"), hi.trim().parse().expect("hi"));
                } else {
                    let (var, lo) = trimmed.split_once(">=").expect("lower bound");
                    let v: usize = var.trim().trim_start_matches('x').parse().expect("variable");
                    bounds[v] = (lo.trim().parse().expect("lo"), f64::INFINITY);
                }
            }
            "General" => {
                let v: usize = trimmed.trim_start_matches('x').parse().expect("integer variable");
                integer[v] = true;
            }
            _ => {}
        }
    }

    let mut lp = Problem::new(OptimizationDirection::Maximize);
    lp.set_time_limit(Duration::from_secs(60));
    let vars: Vec<microlp::Variable> = (0..n)
        .map(|v| {
            let (lo, hi) = bounds[v];
            if integer[v] {
                let hi = if hi.is_finite() { hi as i32 } else { i32::MAX / 2 };
                lp.add_integer_var(objective[v], (lo as i32, hi))
            } else {
                lp.add_var(objective[v], (lo, hi))
            }
        })
        .collect();
    for (terms, op, rhs) in &constraints {
        let expr: Vec<(microlp::Variable, f64)> = terms.iter().map(|&(v, c)| (vars[v], c)).collect();
        lp.add_constraint(&expr, *op, *rhs);
    }
    let solution = lp.solve().map_err(|e| format!("microlp: {e:?}"))?;
    Ok((vars.iter().map(|v| solution.var_value(*v)).collect(), solution.objective()))
}

/// The exact planner's coin plan for `counts` with `electric`, built the way the page builds it.
fn plan_with_power(
    items: &[ProductionItem],
    counts: &FacilityCounts,
    electric: Option<&ElectricConfig>,
) -> Result<ExactPlan, String> {
    let modules = ModuleLevels::default();
    let goal = Goal::Earn { floors: &[] };
    let problem = write_lp_with_power(items, "coins", counts, &modules, goal, electric);
    let (values, objective) = solve_lp(&problem)?;
    plan_from_values_with_power(items, "coins", counts, &modules, goal, &values, true, objective, electric)
        .ok_or_else(|| "plan_from_values_with_power returned None".to_string())
}

fn check(plan: &ExactPlan, items: &[ProductionItem], counts: &FacilityCounts, electric: Option<&ElectricConfig>) -> Result<f64, String> {
    check_plan_with_power(plan, items, "coins", counts, &ModuleLevels::default(), None, electric)
}

/// Watts the plan's powered units draw, by the same rule the model uses (owned top level).
fn demand(plan: &ExactPlan, items: &[ProductionItem], counts: &FacilityCounts) -> f64 {
    plan.electric_units
        .iter()
        .map(|(name, &units)| {
            let item = items.iter().find(|i| &i.name == name).expect("powered recipe exists");
            units as f64 * electric_require(&item.facility, counts.get_level(&item.facility)).expect("has a power rule")
        })
        .sum()
}

fn production_time(items: &[ProductionItem], name: &str) -> f64 {
    items.iter().find(|i| i.name == name).expect("recipe exists").production_time
}

/// A hand-made plan: recipe -> (rate, units), plus powered units per recipe. Nothing is sold, so
/// the re-check has no balance to trip over; it only earns the seed costs (none for Mine/Well).
fn manual_plan(rows: &[(&str, f64, u32)], powered: &[(&str, u32)]) -> ExactPlan {
    ExactPlan {
        rate_per_second: 0.0,
        objective: 0.0,
        upper_bound: 0.0,
        proven_optimal: false,
        nodes: 0,
        recipe_rates: rows.iter().map(|(n, r, _)| (n.to_string(), *r)).collect(),
        units: rows.iter().map(|(n, _, u)| (n.to_string(), *u)).collect(),
        electric_units: powered.iter().map(|(n, u)| (n.to_string(), *u)).collect(),
        sold: BTreeMap::new(),
        environment: Vec::new(),
        pairs: Vec::new(),
        pace: None,
        staffing: Vec::new(),
    }
}

/// TEST FAKE DATA roster builder: `(count, [(ability, level)])`, with the game's environment
/// buildings exactly as `web/app.js` (`ENVIRONMENT_BUILDING_ABILITY`) sends them, and no
/// personality bonuses. Mirrors `crew_of` in `tests/exact_tests.rs`.
fn crew_of(members: &[(u32, &[(&str, u32)])]) -> Crew {
    Crew {
        members: members
            .iter()
            .map(|(count, abilities)| RosterAniimo {
                count: *count,
                abilities: abilities.iter().map(|(a, l)| (a.to_string(), *l)).collect(),
                personalities: Vec::new(),
            })
            .collect(),
        residents: ["Tidewhisper Sandcastle", "Dewy House", "Nimbus Bed", "Starfall Hammock", "Floral Windmill"]
            .into_iter()
            .map(String::from)
            .collect(),
        environment: [("Heat Furnace", "Fire"), ("Cooling Unit", "Ice"), ("Sunlamp", "Light")]
            .into_iter()
            .map(|(b, a)| (b.to_string(), a.to_string()))
            .collect(),
        personalities: Default::default(),
    }
}

/// The data's items reworked for `crew`, and `counts` planning with it.
fn with_crew(counts: &FacilityCounts, crew: Crew) -> Option<(Vec<ProductionItem>, FacilityCounts)> {
    let items = load_items()?;
    let requirements = load_aniimo_requirements(Path::new("data")).unwrap();
    let steps = load_grower_steps(Path::new("data")).unwrap();
    let items = aniimax::models::crew_variants(items, &crew, &requirements, &steps);
    let mut counts = counts.clone();
    counts.set_crew(crew);
    Some((items, counts))
}

// ---------------------------------------------------------------------------------------------
// 1. Harness and electricity-off regression
// ---------------------------------------------------------------------------------------------

// The LP-text path with no electricity must find the same plan value as the native solver, or the
// harness itself is wrong. Both scenarios are TEST FAKE DATA.
#[test]
fn harness_without_power_matches_solve_exact() {
    let Some(items) = load_items() else { return };
    let modules = ModuleLevels::default();
    for counts in [FacilityCounts::only(&[("Farmland", 6, 2)]), FacilityCounts::only(&[("Mine", 2, 4)])] {
        let native = solve_exact(&items, "coins", &counts, &modules, Goal::Earn { floors: &[] }, Some(Duration::from_secs(60)), None)
            .expect("native plan");
        let via_lp = plan_with_power(&items, &counts, None).expect("LP-path plan");
        assert!(
            (native.rate_per_second - via_lp.rate_per_second).abs() < 1e-6,
            "native {} vs LP path {}",
            native.rate_per_second,
            via_lp.rate_per_second
        );
        assert!(via_lp.electric_units.is_empty());
        check(&via_lp, &items, &counts, None).expect("passes its re-check");
    }
}

// A disabled ElectricConfig must behave exactly like no electricity at all.
#[test]
fn disabled_power_matches_no_power() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 2, 4)]);
    let disabled = ElectricConfig::disabled();
    let off = plan_with_power(&items, &counts, None).expect("plan");
    let disabled_plan = plan_with_power(&items, &counts, Some(&disabled)).expect("plan");
    assert!((off.rate_per_second - disabled_plan.rate_per_second).abs() < 1e-9);
    assert!(disabled_plan.electric_units.is_empty());
    check(&disabled_plan, &items, &counts, Some(&disabled)).expect("passes its re-check");
}

// ---------------------------------------------------------------------------------------------
// 2. Electricity on: model -> plan -> independent re-check (no roster)
// ---------------------------------------------------------------------------------------------

// One Lv.4 Mine (TEST FAKE DATA) on a Lv.2 generator: the plan powers it and still passes the
// re-check. Rule-agnostic bounds only: E-mode never lowers output and never adds more than +20%;
// the exact gain depends on the E-mode timing rule still to be confirmed in game (P2).
#[test]
fn powered_mine_plan_passes_recheck() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 1, 4)]);
    let power = ElectricConfig::new(2);
    let off = plan_with_power(&items, &counts, None).expect("plan without power");
    let on = plan_with_power(&items, &counts, Some(&power)).expect("plan with power");
    assert!(!on.electric_units.is_empty(), "the Mine should be powered: {:?}", on.units);
    assert!(on.rate_per_second > off.rate_per_second, "on {} vs off {}", on.rate_per_second, off.rate_per_second);
    assert!(on.rate_per_second <= off.rate_per_second * 1.2 + 1e-9, "more than +20%: {} vs {}", on.rate_per_second, off.rate_per_second);
    assert!(demand(&on, &items, &counts) <= power.capacity());
    check(&on, &items, &counts, Some(&power)).expect("model plan passes its re-check");
}

// Six Lv.4 Mines (120W each by electric_rule, 720W) on a Lv.2 generator (800W capacity): the
// model's plan must stay within capacity and pass the re-check. Which units get powered depends
// on the 120% threshold rule (P4, still to be confirmed), so this only checks rule-agnostic facts.
#[test]
fn powered_plan_respects_capacity_and_passes_recheck() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 6, 4)]);
    let power = ElectricConfig::new(2);
    let plan = plan_with_power(&items, &counts, Some(&power)).expect("plan");
    assert!(!plan.electric_units.is_empty());
    assert!(demand(&plan, &items, &counts) <= power.capacity());
    check(&plan, &items, &counts, Some(&power)).expect("model plan passes its re-check");
}

// Every powered unit must belong to a recipe that actually runs (P5 / R-14). Solver-dependent:
// nothing in the model forbids idle powered units, so this may start failing with another solver.
#[test]
fn powered_units_belong_to_running_recipes() {
    let Some(items) = load_items() else { return };
    for (counts, level) in [
        (FacilityCounts::only(&[("Mine", 1, 4)]), 2),
        (FacilityCounts::only(&[("Mine", 6, 4)]), 2),
        (FacilityCounts::only(&[("Mine", 3, 4), ("Well", 2, 4)]), 5),
    ] {
        let power = ElectricConfig::new(level);
        let plan = plan_with_power(&items, &counts, Some(&power)).expect("plan");
        for (name, &powered) in &plan.electric_units {
            assert!(plan.recipe_rates.get(name).copied().unwrap_or(0.0) > 1e-9, "{name} powered but idle");
            assert!(powered <= plan.units.get(name).copied().unwrap_or(0), "{name}: {powered} powered > units");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Hand-made plans through the independent re-check
// ---------------------------------------------------------------------------------------------

// Legal: one Lv.4 Mine, powered, running rock at exactly 1.2 units of time. Lv.2 generator.
#[test]
fn recheck_accepts_powered_unit_at_its_limit() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 1, 4)]);
    let power = ElectricConfig::new(2);
    let t = production_time(&items, "rock");
    let plan = manual_plan(&[("rock", 1.2 / t, 1)], &[("rock", 1)]);
    check(&plan, &items, &counts, Some(&power)).expect("1.2 units of work on one powered unit is legal");
}

// Illegal: the same unit asked for a little more than 1.2 units of work.
#[test]
fn recheck_rejects_powered_unit_over_its_limit() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 1, 4)]);
    let power = ElectricConfig::new(2);
    let t = production_time(&items, "rock");
    let plan = manual_plan(&[("rock", 1.21 / t, 1)], &[("rock", 1)]);
    assert!(check(&plan, &items, &counts, Some(&power)).is_err());
}

// Illegal: seven Lv.4 Mines powered = 840W, over a Lv.2 generator's 800W capacity. (All seven
// kept running at 120%, 8.4 units of work, so only the capacity is wrong.)
#[test]
fn recheck_rejects_demand_over_capacity() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 7, 4)]);
    let power = ElectricConfig::new(2);
    let t = production_time(&items, "rock");
    let plan = manual_plan(&[("rock", 8.4 / t, 7)], &[("rock", 7)]);
    let err = check(&plan, &items, &counts, Some(&power)).expect_err("840W > 800W must be rejected");
    assert!(err.contains("exceeds generator capacity"), "{err}");
}

// Illegal: more powered units than units on the recipe.
#[test]
fn recheck_rejects_more_powered_than_units() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 2, 4)]);
    let power = ElectricConfig::new(2);
    let t = production_time(&items, "rock");
    let plan = manual_plan(&[("rock", 1.0 / t, 1)], &[("rock", 2)]);
    assert!(check(&plan, &items, &counts, Some(&power)).is_err());
}

// Illegal: powered units while electricity is off.
#[test]
fn recheck_rejects_power_while_disabled() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 1, 4)]);
    let t = production_time(&items, "rock");
    let plan = manual_plan(&[("rock", 1.0 / t, 1)], &[("rock", 1)]);
    assert!(check(&plan, &items, &counts, None).is_err());
    assert!(check(&plan, &items, &counts, Some(&ElectricConfig::disabled())).is_err());
}

// KNOWN BUG P4 (validator weaker than the model): six Lv.4 Mines = 720W on a Lv.2 generator is
// above the 660W boost threshold yet within 800W capacity. The re-check still grants every
// powered unit +20%. Expected after the fix: rejected (or re-timed at the confirmed efficiency;
// under either candidate rule it is not 120%). Today: accepted.
#[test]
fn recheck_rejects_full_boost_above_threshold() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 6, 4)]);
    let power = ElectricConfig::new(2);
    let t = production_time(&items, "rock");
    let plan = manual_plan(&[("rock", 7.2 / t, 6)], &[("rock", 6)]);
    assert!(check(&plan, &items, &counts, Some(&power)).is_err(), "720W > 660W threshold cannot run at 120%");
}

// KNOWN BUG P6 (mixed levels): Mine 2 x Lv.4 + 1 x Lv.1 and Well 1 x Lv.4 + 1 x Lv.1 on a Lv.1
// generator (500W threshold). By electric_rule the real draw of all five is 120+120+30+120+30 =
// 420W, so all five can be powered; the model prices every unit at the facility's top level
// (5 x 120 = 600W) and cannot. Expected after the fix: all 5 units powered.
#[test]
#[ignore = "KNOWN BUG P6: power is priced at the facility's highest owned level"]
fn mixed_levels_are_priced_per_unit() {
    let Some(items) = load_items() else { return };
    let mut counts = FacilityCounts::only(&[]);
    counts.set_tiers("Mine", vec![(2, 4), (1, 1)]);
    counts.set_tiers("Well", vec![(1, 4), (1, 1)]);
    let power = ElectricConfig::new(1);
    let plan = plan_with_power(&items, &counts, Some(&power)).expect("plan");
    let powered: u32 = plan.electric_units.values().sum();
    assert_eq!(powered, 5, "all five fit in 420W: {:?}", plan.electric_units);
}

// ---------------------------------------------------------------------------------------------
// 4. Roster (custom Aniimo) with electricity
// ---------------------------------------------------------------------------------------------

// P1 (fixed in phase 1): with a Lightning Aniimo on the roster the model staffs the Crackle
// Generator, and the re-check used to look the generator up among environment buildings and fail
// with "Crackle Generator needs no Aniimo". Two Earth Aniimo so the hours rule (P2) is not hit:
// one powered Mine needs at most 1.2 days even without any E-mode deduction.
#[test]
fn roster_with_lightning_staffs_generator() {
    let base = FacilityCounts::only(&[("Mine", 1, 4)]);
    let crew = crew_of(&[(2, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let Some((items, counts)) = with_crew(&base, crew) else { return };
    let power = ElectricConfig::new(2);
    let plan = plan_with_power(&items, &counts, Some(&power)).expect("the model finds a plan");
    assert!(
        plan.staffing.iter().any(|(b, m, share)| b == "Crackle Generator" && *m == 1 && (*share - 1.0).abs() < 1e-9),
        "generator staffed by the Lightning member: {:?}",
        plan.staffing
    );
    check(&plan, &items, &counts, Some(&power)).expect("model plan passes its re-check");
}

// P1, confirmed design (phase 1): a roster with E-mode on and no Lightning Aniimo is rejected
// with a reason naming Lightning, by every entry point the page calls, before any model is built.
// (Phase 0 left this open; it used to produce the empty, impossible constraint `0 >= 1`.)
// Below RV12 E-mode is not on, so the same roster plans normally.
#[test]
fn wasm_roster_without_lightning_is_rejected_with_reason() {
    let Some(items) = load_items() else { return };
    let input = roster_input(&items, 12, true, serde_json::json!([{ "count": 1, "abilities": { "Earth": 4 } }]));
    let json = input.to_string();
    for (entry, problem) in [
        ("exact_problem", aniimax::wasm::exact_problem(&json, "{}")),
        ("exact_priority_problem", aniimax::wasm::exact_priority_problem(&json, "{}", "coins")),
        ("exact_level_up_problem", aniimax::wasm::exact_level_up_problem(&json)),
    ] {
        let problem: serde_json::Value = serde_json::from_str(&problem).unwrap();
        assert_eq!(problem["lp"], "", "{entry} built a model: {problem}");
        assert!(problem["error"].as_str().is_some_and(|e| e.contains("Lightning")), "{entry}: {problem}");
    }
    let byproducts: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_byproduct_problems(&json)).unwrap();
    assert!(byproducts[0]["error"].as_str().is_some_and(|e| e.contains("Lightning")), "{byproducts}");
    let solution = serde_json::json!({ "values": [], "proven": true, "bound": 0.0 }).to_string();
    let plan: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_plan(&json, "{}", &solution)).unwrap();
    assert_eq!(plan["success"], false);
    assert!(plan["error"].as_str().is_some_and(|e| e.contains("Lightning")), "{plan}");
}

// The rule applies only when E-mode is on: the same Lightning-less roster plans normally with
// E-mode unticked, and at RV11 where the back end leaves E-mode off whatever the page sends.
#[test]
fn wasm_roster_without_lightning_plans_when_power_is_off() {
    let Some(items) = load_items() else { return };
    let earth_only = serde_json::json!([{ "count": 1, "abilities": { "Earth": 4 } }]);
    for (home_level, electric_enabled) in [(12, false), (11, true)] {
        let plan = wasm_plan(&roster_input(&items, home_level, electric_enabled, earth_only.clone()));
        assert_eq!(plan["success"], true, "RV{home_level}, E-mode {electric_enabled}: {plan}");
        assert_eq!(plan["electric_enabled"], false);
        assert!(plan["staffing"].as_array().unwrap().is_empty(), "{plan}");
    }
}

// The same at the model level, without the WASM layer: no E-mode, no generator, no Lightning
// needed.
#[test]
fn roster_without_lightning_and_without_power_still_passes() {
    let base = FacilityCounts::only(&[("Mine", 1, 4)]);
    let crew = crew_of(&[(1, &[("Earth", 4)])]);
    let Some((items, counts)) = with_crew(&base, crew) else { return };
    let plan = plan_with_power(&items, &counts, None).expect("plan");
    assert!(plan.rate_per_second > 0.0);
    assert!(plan.staffing.is_empty());
    check(&plan, &items, &counts, None).expect("passes its re-check");
}

// The shared rule the model, the re-check and the WASM input check all use (TEST FAKE DATA).
#[test]
fn generator_staff_rule_needs_lightning_and_a_count() {
    use aniimax::electric::{can_staff_generator, crew_can_staff_generator, GENERATOR_ABILITY, GENERATOR_BUILDING};
    assert_eq!(GENERATOR_BUILDING, "Crackle Generator");
    assert_eq!(GENERATOR_ABILITY, "Lightning");
    let crew = crew_of(&[(1, &[("Lightning", 1)]), (0, &[("Lightning", 4)]), (3, &[("Earth", 4), ("Fire", 2)])]);
    assert!(can_staff_generator(&crew.members[0]));
    assert!(!can_staff_generator(&crew.members[1]), "none of it on the roster");
    assert!(!can_staff_generator(&crew.members[2]), "no Lightning");
    assert!(crew_can_staff_generator(&crew));
    assert!(!crew_can_staff_generator(&crew_of(&[(0, &[("Lightning", 4)]), (3, &[("Earth", 4)])])));
    assert!(!crew_can_staff_generator(&crew_of(&[])));
}

// The re-check's own Crackle Generator rule (P1), on hand-made roster plans. TEST FAKE DATA: one
// Lv.4 Mine; roster member 0 is two Earth Aniimo, member 1 one Lightning Aniimo. The Mine runs at
// 1.2 units of time on its one powered unit, which the two Earth Aniimo cover even counting the
// full time (the E-mode hours rule, P2, is not exercised here).
fn generator_staff_case() -> Option<(Vec<ProductionItem>, FacilityCounts, String)> {
    let base = FacilityCounts::only(&[("Mine", 1, 4)]);
    let crew = crew_of(&[(2, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let (items, counts) = with_crew(&base, crew)?;
    let copy = items.iter().find(|i| aniimax::models::base_item_name(&i.name) == "rock" && i.crew == Some(0))?.name.clone();
    Some((items, counts, copy))
}

fn staffed_plan(items: &[ProductionItem], copy: &str, staffing: Vec<(String, usize, f64)>) -> ExactPlan {
    let t = production_time(items, copy);
    let mut plan = manual_plan(&[(copy, 1.2 / t, 1)], &[(copy, 1)]);
    plan.staffing = staffing;
    plan
}

#[test]
fn recheck_accepts_lightning_generator_staff() {
    let Some((items, counts, copy)) = generator_staff_case() else { return };
    let power = ElectricConfig::new(2);
    let plan = staffed_plan(&items, &copy, vec![("Crackle Generator".to_string(), 1, 1.0)]);
    check(&plan, &items, &counts, Some(&power)).expect("a Lightning member may staff the generator");
}

#[test]
fn recheck_rejects_generator_staff_without_lightning() {
    let Some((items, counts, copy)) = generator_staff_case() else { return };
    let power = ElectricConfig::new(2);
    let plan = staffed_plan(&items, &copy, vec![("Crackle Generator".to_string(), 0, 1.0)]);
    let err = check(&plan, &items, &counts, Some(&power)).expect_err("an Earth member cannot staff the generator");
    assert!(err.contains("Lightning") && err.contains("member 0"), "{err}");
}

#[test]
fn recheck_rejects_unstaffed_generator() {
    let Some((items, counts, copy)) = generator_staff_case() else { return };
    let power = ElectricConfig::new(2);
    let plan = staffed_plan(&items, &copy, Vec::new());
    let err = check(&plan, &items, &counts, Some(&power)).expect_err("E-mode on needs a generator worker");
    assert!(err.contains("Lightning"), "{err}");
}

#[test]
fn recheck_rejects_generator_staff_while_power_is_off() {
    let Some((items, counts, copy)) = generator_staff_case() else { return };
    let t = production_time(&items, &copy);
    let mut plan = manual_plan(&[(copy.as_str(), 1.0 / t, 1)], &[]);
    plan.staffing = vec![("Crackle Generator".to_string(), 1, 1.0)];
    let err = check(&plan, &items, &counts, None).expect_err("no generator without E-mode");
    assert!(err.contains("E-mode is off"), "{err}");
}

// Regression: a roster with electricity OFF keeps working exactly as before.
#[test]
fn roster_without_power_still_passes() {
    let base = FacilityCounts::only(&[("Mine", 1, 4)]);
    let crew = crew_of(&[(1, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let Some((items, counts)) = with_crew(&base, crew) else { return };
    let plan = plan_with_power(&items, &counts, None).expect("plan");
    assert!(plan.electric_units.is_empty());
    assert!(plan.staffing.is_empty(), "no generator without electricity: {:?}", plan.staffing);
    check(&plan, &items, &counts, None).expect("passes its re-check");
}

// KNOWN BUG P2 (hours): two Lv.4 Mines, ONE Earth Aniimo plus a Lightning one. The model lets a
// powered unit replace its worker (busy = rate*t - powered - 0.2*boosted), so it plans more than
// one day of Mine work for the single Earth member; the re-check counts the full rate*t and
// rejects it. (Before phase 1 the P1 error was hit first.) Expected after the P2 fix: the model
// plan passes its re-check. The exact hours rule itself still needs in-game confirmation.
#[test]
fn roster_hours_saved_by_power_pass_recheck() {
    let base = FacilityCounts::only(&[("Mine", 2, 4)]);
    let crew = crew_of(&[(1, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let Some((items, counts)) = with_crew(&base, crew) else { return };
    let power = ElectricConfig::new(2);
    let plan = plan_with_power(&items, &counts, Some(&power)).expect("the model finds a plan");
    check(&plan, &items, &counts, Some(&power)).expect("model plan passes its re-check");
}

// P3 (phantom hours), model-level invariant: per roster member, the work it really has to do,
// sum over its recipes of max(0, rate*t - 1.2*powered), plus its staffing shares, fits in its
// day. The model itself lets a recipe's term go negative, so an idle powered unit could lend
// hours to the member's other work; this checks that no such plan is produced. One Aniimo with
// Earth, Grass and Dark (TEST FAKE DATA) works both a Mine and Farmland.
#[test]
fn roster_member_hours_never_rely_on_negative_terms() {
    let base = FacilityCounts::only(&[("Mine", 1, 4), ("Farmland", 6, 5)]);
    let crew = crew_of(&[(1, &[("Earth", 4), ("Grass", 3), ("Dark", 3)]), (1, &[("Lightning", 1)])]);
    let Some((items, counts)) = with_crew(&base, crew.clone()) else { return };
    let power = ElectricConfig::new(5);
    let plan = plan_with_power(&items, &counts, Some(&power)).expect("the model finds a plan");
    let mut busy = vec![0.0; crew.members.len()];
    for (name, &rate) in &plan.recipe_rates {
        let item = items.iter().find(|i| &i.name == name).unwrap();
        let Some(member) = item.crew else { continue };
        let powered = plan.electric_units.get(name).copied().unwrap_or(0) as f64;
        busy[member] += (rate * item.production_time - 1.2 * powered).max(0.0);
    }
    for (_, member, share) in &plan.staffing {
        busy[*member] += share;
    }
    for (member, (&used, aniimo)) in busy.iter().zip(&crew.members).enumerate() {
        assert!(used <= aniimo.count as f64 + 1e-6, "member {member} really works {used} days: {plan:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// 5. WASM entry points (the page's JSON in, the page's JSON out)
// ---------------------------------------------------------------------------------------------

/// Every facility the page knows, set to zero unless listed: `JsPlanInput` defaults an unlisted
/// facility to one Lv.1 unit, which the page never relies on (it always sends every facility).
fn page_facilities(items: &[ProductionItem], owned: &[(&str, u32, u32)]) -> serde_json::Value {
    let mut names: Vec<String> = items.iter().map(|i| i.facility.clone()).collect();
    names.extend(["Heat Furnace", "Cooling Unit", "Sunlamp"].map(String::from));
    names.sort();
    names.dedup();
    let mut map = serde_json::Map::new();
    for name in names {
        let tier = owned
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, c, l)| serde_json::json!([{ "count": c, "level": l }]))
            .unwrap_or_else(|| serde_json::json!([{ "count": 0, "level": 1 }]));
        map.insert(name, tier);
    }
    serde_json::Value::Object(map)
}

/// The page's roster input (`aniimoInput` / `rosterPayload` in web/app.js) for one Lv.4 Mine
/// (TEST FAKE DATA) with `members`, at `home_level`, E-mode ticked or not.
fn roster_input(items: &[ProductionItem], home_level: u32, electric_enabled: bool, members: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(items, &[("Mine", 1, 4)]),
        "prioritize_byproducts": false,
        "home_level": home_level,
        "electric_enabled": electric_enabled,
        "generator_level": 2,
        "electric_strategy": "combined",
        "aniimo": "roster",
        "roster": {
            "members": members,
            "residents": ["Tidewhisper Sandcastle", "Dewy House", "Nimbus Bed", "Starfall Hammock", "Floral Windmill"],
            "environment": { "Heat Furnace": "Fire", "Cooling Unit": "Ice", "Sunlamp": "Light" }
        }
    })
}

/// Solves `exact_problem`'s model and hands the answer to `exact_plan`, as `web/worker.js` does.
fn wasm_plan(input: &serde_json::Value) -> serde_json::Value {
    let input = input.to_string();
    let problem: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_problem(&input, "{}")).unwrap();
    let lp = LpProblem {
        lp: problem["lp"].as_str().unwrap().to_string(),
        variables: problem["variables"].as_u64().unwrap() as usize,
        tiebreak: Vec::new(),
    };
    let (values, objective) = solve_lp(&lp).expect("the model solves");
    let solution = serde_json::json!({ "values": values, "proven": true, "bound": objective });
    serde_json::from_str(&aniimax::wasm::exact_plan(&input, "{}", &solution.to_string())).unwrap()
}

// The page's own path without a roster: RV12, Lv.2 generator, one Lv.4 Mine (TEST FAKE DATA).
// The summary the page shows must agree with the model's limits.
#[test]
fn wasm_powered_plan_reports_electric_summary() {
    let Some(items) = load_items() else { return };
    let input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
        "prioritize_byproducts": false,
        "home_level": 12,
        "electric_enabled": true,
        "generator_level": 2,
        "electric_strategy": "combined",
        "aniimo": "best",
    });
    let plan = wasm_plan(&input);
    assert_eq!(plan["success"], true, "{plan}");
    assert_eq!(plan["electric_enabled"], true);
    let demand = plan["electric_demand"].as_f64().unwrap();
    assert!(demand > 0.0 && demand <= plan["electric_generator_capacity"].as_f64().unwrap());
}

// Below RV12 the page's electric flag must be ignored by the back end.
#[test]
fn wasm_ignores_power_below_rv12() {
    let Some(items) = load_items() else { return };
    let input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
        "prioritize_byproducts": false,
        "home_level": 11,
        "electric_enabled": true,
        "generator_level": 2,
        "aniimo": "best",
    });
    let plan = wasm_plan(&input);
    assert_eq!(plan["success"], true, "{plan}");
    assert_eq!(plan["electric_enabled"], false);
}

// General (non-roster) setups have no roster to check: the Lightning rule never applies to them.
// Both the page's Best and Minimum setups still plan with E-mode on (Best is also covered by
// `wasm_powered_plan_reports_electric_summary`).
#[test]
fn wasm_general_setups_ignore_lightning_rule() {
    let Some(items) = load_items() else { return };
    for setup in ["best", "minimum"] {
        let input = serde_json::json!({
            "currency": "coins",
            "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
            "prioritize_byproducts": false,
            "home_level": 12,
            "electric_enabled": true,
            "generator_level": 2,
            "aniimo": setup,
        });
        let plan = wasm_plan(&input);
        assert_eq!(plan["success"], true, "{setup}: {plan}");
        assert_eq!(plan["electric_enabled"], true, "{setup}");
        assert!(plan["staffing"].as_array().unwrap().is_empty(), "{setup}: {plan}");
    }
}

// P1 (fixed in phase 1) through the page's own path: the page's roster payload
// (`rosterPayload()` in web/app.js) lists only Heat Furnace / Cooling Unit / Sunlamp as staffed
// buildings, which made `exact_plan` reject the model's own plan.
#[test]
fn wasm_roster_plan_with_power_succeeds() {
    let Some(items) = load_items() else { return };
    let input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
        "prioritize_byproducts": false,
        "home_level": 12,
        "electric_enabled": true,
        "generator_level": 2,
        "aniimo": "roster",
        "roster": {
            "members": [
                { "count": 2, "abilities": { "Earth": 4 } },
                { "count": 1, "abilities": { "Lightning": 1 } }
            ],
            "residents": ["Tidewhisper Sandcastle", "Dewy House", "Nimbus Bed", "Starfall Hammock", "Floral Windmill"],
            "environment": { "Heat Furnace": "Fire", "Cooling Unit": "Ice", "Sunlamp": "Light" }
        }
    });
    let plan = wasm_plan(&input);
    assert_eq!(plan["success"], true, "{plan}");
}

// KNOWN BUG (display key, P2/P8): with a roster, `electric_units` names the roster copy
// (`rock__by0`) while the plan rows name the recipe (`rock`), so the page can never match them.
// Expected after the fix: every powered row names a recipe that appears in `coin_items`.
// (Before phase 1 this was also blocked by P1; the name mismatch itself is out of its scope.)
#[test]
#[ignore = "KNOWN BUG: roster electric_units use copy names (rock__by0)"]
fn wasm_roster_electric_units_use_recipe_names() {
    let Some(items) = load_items() else { return };
    let input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
        "prioritize_byproducts": false,
        "home_level": 12,
        "electric_enabled": true,
        "generator_level": 2,
        "aniimo": "roster",
        "roster": {
            "members": [
                { "count": 2, "abilities": { "Earth": 4 } },
                { "count": 1, "abilities": { "Lightning": 1 } }
            ],
            "residents": [],
            "environment": { "Heat Furnace": "Fire", "Cooling Unit": "Ice", "Sunlamp": "Light" }
        }
    });
    let plan = wasm_plan(&input);
    assert_eq!(plan["success"], true, "{plan}");
    let rows: Vec<&str> = plan["coin_items"].as_array().unwrap().iter().filter_map(|s| s["item_name"].as_str()).collect();
    for unit in plan["electric_units"].as_array().unwrap() {
        let name = unit[1].as_str().unwrap();
        assert!(rows.contains(&name), "powered {name} matches no plan row {rows:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// 6. Stage A of the P2 / P3 / P5 plan: tests written before the fixes
// ---------------------------------------------------------------------------------------------
//
// Everything here relies on the CURRENT model assumption that a powered unit runs at 120% and
// replaces its Aniimo (1 + 0.2 units of work per powered unit). That assumption has not been
// confirmed in game; these tests check that the model and the re-check agree on it, not that it
// is right.

/// `(terms, op, rhs)` of every constraint in `write_lp_with_power`'s LP text.
fn lp_constraints(lp: &str) -> Vec<(Vec<(usize, f64)>, String, f64)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in lp.lines() {
        match line.trim() {
            "Subject To" => {
                inside = true;
                continue;
            }
            "Bounds" => break,
            _ => {}
        }
        if !inside {
            continue;
        }
        let Some((_, body)) = line.trim().split_once(':') else { continue };
        let (lhs, op, rhs) = if let Some((l, r)) = body.split_once("<=") {
            (l, "<=", r)
        } else if let Some((l, r)) = body.split_once(">=") {
            (l, ">=", r)
        } else {
            let (l, r) = body.split_once('=').unwrap();
            (l, "=", r)
        };
        let tokens: Vec<&str> = lhs.split_whitespace().collect();
        let terms = tokens
            .chunks(3)
            .map(|t| {
                let sign = if t[0] == "-" { -1.0 } else { 1.0 };
                (t[2].trim_start_matches('x').parse().unwrap(), sign * t[1].parse::<f64>().unwrap())
            })
            .collect();
        out.push((terms, op.to_string(), rhs.trim().parse().unwrap()));
    }
    out
}

/// The `(powered, boosted)` variable pairs of the current model layout: a boosted variable is the
/// one with coefficient -0.2 in a recipe's capacity row (`rate*t - units - 0.2*boosted <= 0`), and
/// its powered variable is the other one in `boosted - powered <= 0`.
///
/// The shape `+1 x_b - 1 x_other <= 0` alone is not enough: with one unit (`max = 1`) the grid row
/// `boosted - max*g <= 0` has it too, with the grid flag `g`. So a powered variable must also be a
/// powered variable by the model's own rows: it is never the grid flag (the only variable a
/// single-term row `+1 x = 1` fixes), and it is in a grid demand row
/// (`sum(electricRequire * powered) <= capacity or threshold`: a positive limit, every coefficient
/// a wattage of at least 15W, the smallest `electric_rule` gives).
fn powered_boosted_pairs(lp: &str) -> Vec<(usize, usize)> {
    let rows = lp_constraints(lp);
    let grid_flags: Vec<usize> = rows
        .iter()
        .filter(|(terms, op, rhs)| op == "=" && *rhs == 1.0 && terms.len() == 1 && terms[0].1 == 1.0)
        .map(|(terms, _, _)| terms[0].0)
        .collect();
    let in_demand_row = |v: usize| {
        rows.iter().any(|(terms, op, rhs)| {
            op == "<=" && *rhs > 0.0 && terms.iter().all(|(_, c)| *c >= 15.0) && terms.iter().any(|(t, _)| *t == v)
        })
    };
    let boosted: Vec<usize> = rows
        .iter()
        .filter(|(_, op, rhs)| op == "<=" && *rhs == 0.0)
        .flat_map(|(terms, _, _)| terms.iter().filter(|(_, c)| (*c + 0.2).abs() < 1e-12).map(|(v, _)| *v))
        .collect();
    let mut pairs = Vec::new();
    for b in boosted {
        for (terms, op, rhs) in &rows {
            if op == "<=" && *rhs == 0.0 && terms.len() == 2 && terms.contains(&(b, 1.0)) {
                if let Some(&(p, _)) = terms.iter().find(|(v, c)| *v != b && *c == -1.0) {
                    if !grid_flags.contains(&p) && in_demand_row(p) {
                        pairs.push((p, b));
                    }
                }
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// The LP text with one more constraint, `extra`, before its bounds.
fn with_extra_row(lp: &str, extra: &str) -> String {
    lp.replacen("Bounds\n", &format!(" extra: {extra}\nBounds\n"), 1)
}

fn one_powered_mine_lp() -> Option<LpProblem> {
    let items = load_items()?;
    let counts = FacilityCounts::only(&[("Mine", 1, 4)]);
    Some(write_lp_with_power(&items, "coins", &counts, &ModuleLevels::default(), Goal::Earn { floors: &[] }, Some(&ElectricConfig::new(2))))
}

// Control for the P5 test below: the current layout has a (powered, boosted) pair, and the model
// can power the Mine with boosted == powered. Without this, the P5 test could pass only because
// the harness made every plan impossible.
#[test]
fn p5_model_allows_boosted_equal_to_powered() {
    let Some(problem) = one_powered_mine_lp() else { return };
    let pairs = powered_boosted_pairs(&problem.lp);
    assert!(!pairs.is_empty(), "no powered/boosted pair found in the model");
    for (p, b) in pairs {
        let lp = with_extra_row(&problem.lp, &format!("+ 1 x{p} >= 1"));
        let lp = with_extra_row(&lp, &format!("+ 1 x{b} - 1 x{p} >= 0"));
        solve_lp(&LpProblem { lp, ..problem.clone() }).expect("powered == boosted == 1 must be possible");
    }
}

// KNOWN BUG P5 (coupling): the row meant to force boosted == powered while the grid is boosted
// (`exact.rs`, the `powered_groups` loop) reads `boosted - powered + max*g >= 0`, which with g
// fixed at 1 says nothing, so a powered unit may go unboosted. Expected after the fix: no plan
// has powered > boosted. This targets the current variable layout; if the fix merges the two
// variables, `powered_boosted_pairs` finds none and the control test above must change too.
#[test]
fn p5_powered_units_are_always_boosted() {
    let Some(problem) = one_powered_mine_lp() else { return };
    let pairs = powered_boosted_pairs(&problem.lp);
    assert!(!pairs.is_empty(), "no powered/boosted pair found in the model");
    for (p, b) in pairs {
        let lp = with_extra_row(&problem.lp, &format!("+ 1 x{p} - 1 x{b} >= 1"));
        assert!(solve_lp(&LpProblem { lp, ..problem.clone() }).is_err(), "x{p} powered without x{b} boosted is still allowed");
    }
}

// P5 baselines, recorded from the model BEFORE the P5 fix (general setup, no roster). Fixing the
// coupling only rules out plans with unboosted powered units, which are never better, so these
// optimal values must not move. They reflect the current 120% assumption and the current
// top-level power pricing (P6), not confirmed game data; the P6 fix may change the mixed case.
#[test]
fn p5_general_setup_optimum_baselines() {
    let Some(items) = load_items() else { return };
    let mut mixed = FacilityCounts::only(&[]);
    mixed.set_tiers("Mine", vec![(2, 4), (1, 1)]);
    mixed.set_tiers("Well", vec![(1, 4), (1, 1)]);
    let cases: Vec<(&str, FacilityCounts, u8, f64)> = vec![
        ("Mine 1 x Lv4, generator Lv2", FacilityCounts::only(&[("Mine", 1, 4)]), 2, 0.35555555555555557),
        ("Mine 6 x Lv4, generator Lv2", FacilityCounts::only(&[("Mine", 6, 4)]), 2, 2.07407407407407396),
        ("Mine 3 + Well 2, Lv4, generator Lv5", FacilityCounts::only(&[("Mine", 3, 4), ("Well", 2, 4)]), 5, 1.86666666666666670),
        ("mixed tiers, generator Lv1", mixed, 1, 1.23777777777777787),
        ("Mine 2 x Lv6 + Well 3 x Lv5, generator Lv4", FacilityCounts::only(&[("Mine", 2, 6), ("Well", 3, 5)]), 4, 3.51111111111111107),
    ];
    for (name, counts, level, baseline) in cases {
        let power = ElectricConfig::new(level);
        let plan = plan_with_power(&items, &counts, Some(&power)).expect("plan");
        assert!((plan.rate_per_second - baseline).abs() <= 1e-9 * baseline, "{name}: {} vs baseline {baseline}", plan.rate_per_second);
        check(&plan, &items, &counts, Some(&power)).expect("passes its re-check");
    }
}

/// What each roster member really works in `plan`: a resident unit is a whole day, a staffing
/// share is that share, and other work is `max(0, rate*t - 1.2*powered)`: a powered unit can stand
/// in for at most its own 1.2 units of work, and never for work elsewhere.
fn honest_busy(plan: &ExactPlan, items: &[ProductionItem], crew: &Crew) -> Vec<f64> {
    let mut busy = vec![0.0; crew.members.len()];
    for (name, &rate) in &plan.recipe_rates {
        let item = items.iter().find(|i| &i.name == name).unwrap();
        let Some(member) = item.crew else { continue };
        busy[member] += if crew.residents.contains(&item.facility) {
            plan.units.get(name).copied().unwrap_or(0) as f64
        } else {
            let powered = plan.electric_units.get(name).copied().unwrap_or(0) as f64;
            (rate * item.production_time - 1.2 * powered).max(0.0)
        };
    }
    for (_, member, share) in &plan.staffing {
        busy[*member] += share;
    }
    busy
}

// P3 (negative hours), more scenarios. Stage A tried ten roster runs (Mine with and without
// Farmland, an input-less Jukebox Dryer, a Carousel Mill, a Tidewhisper Sandcastle; E-mode on and
// off) and the solver never powered an idle unit, so P3 is NOT reproduced; these keep the
// invariant for two of them. TEST FAKE DATA.
#[test]
fn p3_roster_hours_hold_without_negative_terms() {
    let cases: Vec<(&str, Vec<(&str, u32, u32)>, Vec<(u32, Vec<(&str, u32)>)>)> = vec![
        (
            "one Aniimo on a Sandcastle and three Mines",
            vec![("Tidewhisper Sandcastle", 1, 1), ("Mine", 3, 4)],
            vec![(1, vec![("Earth", 4), ("Leisure", 2)]), (1, vec![("Lightning", 1)])],
        ),
        (
            "one Aniimo on a Mill, two Mines and Farmland",
            vec![("Carousel Mill", 1, 1), ("Mine", 2, 4), ("Farmland", 12, 5)],
            vec![(1, vec![("Earth", 4), ("Grass", 3), ("Dark", 3), ("Wind", 3)]), (1, vec![("Lightning", 1)])],
        ),
    ];
    for (name, owned, members) in cases {
        let refs: Vec<(u32, &[(&str, u32)])> = members.iter().map(|(c, a)| (*c, a.as_slice())).collect();
        let crew = crew_of(&refs);
        let Some((items, counts)) = with_crew(&FacilityCounts::only(&owned), crew.clone()) else { return };
        let plan = plan_with_power(&items, &counts, Some(&ElectricConfig::new(5))).expect("plan");
        for powered in plan.electric_units.keys() {
            assert!(plan.recipe_rates.contains_key(powered), "{name}: {powered} powered but idle");
        }
        for (member, (&used, aniimo)) in honest_busy(&plan, &items, &crew).iter().zip(&crew.members).enumerate() {
            assert!(used <= aniimo.count as f64 + 1e-6, "{name}: member {member} really works {used} days");
        }
    }
}

/// TEST FAKE DATA for the hours re-check: `mines` Lv.4 Mines; roster member 0 is one Earth
/// Aniimo, member 1 one Lightning Aniimo staffing the generator (Lv.5 generator, plenty of power).
/// Returns member 0's copies of rock and clay.
fn hours_case(mines: u32) -> Option<(Vec<ProductionItem>, FacilityCounts, String, String)> {
    let base = FacilityCounts::only(&[("Mine", mines, 4)]);
    let crew = crew_of(&[(1, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let (items, counts) = with_crew(&base, crew)?;
    let copy = |recipe: &str| {
        items
            .iter()
            .find(|i| aniimax::models::base_item_name(&i.name) == recipe && i.crew == Some(0))
            .map(|i| i.name.clone())
    };
    let (rock, clay) = (copy("rock")?, copy("clay")?);
    Some((items, counts, rock, clay))
}

fn hours_plan(rows: &[(&str, f64, u32)], powered: &[(&str, u32)]) -> ExactPlan {
    let mut plan = manual_plan(rows, powered);
    plan.staffing = vec![("Crackle Generator".to_string(), 1, 1.0)];
    plan
}

// KNOWN BUG P2, at the limit: three Mines on rock for one Earth Aniimo, two of them powered, 3.4
// units of work (3 units + 0.2 x 2 boosted, the capacity limit). The powered pair covers 2.4 of
// it, leaving exactly 1.0 day for the Aniimo. Expected after the fix: accepted. Today: rejected
// ("works 3.4 days").
#[test]
fn p2_recheck_accepts_hours_exactly_at_limit() {
    let Some((items, counts, rock, _)) = hours_case(3) else { return };
    let t = production_time(&items, &rock);
    let plan = hours_plan(&[(rock.as_str(), 3.4 / t, 3)], &[(rock.as_str(), 2)]);
    check(&plan, &items, &counts, Some(&ElectricConfig::new(5))).expect("1.0 day left for one Aniimo");
}

// KNOWN BUG P2, two powered units: two Mines, both powered, 2.4 units of work; the Earth Aniimo has
// nothing left to do. Expected after the fix: accepted. Today: rejected ("works 2.4 days").
#[test]
fn p2_recheck_accepts_two_powered_units() {
    let Some((items, counts, rock, _)) = hours_case(2) else { return };
    let t = production_time(&items, &rock);
    let plan = hours_plan(&[(rock.as_str(), 2.4 / t, 2)], &[(rock.as_str(), 2)]);
    check(&plan, &items, &counts, Some(&ElectricConfig::new(5))).expect("powered units cover all the work");
}

// Over the limit: four Mines, two powered, 3.5 units of work: 1.1 days left for one Aniimo. Must be
// rejected both before and after the P2 fix (the fix must not loosen this).
#[test]
fn p2_recheck_rejects_hours_over_limit() {
    let Some((items, counts, rock, _)) = hours_case(4) else { return };
    let t = production_time(&items, &rock);
    let plan = hours_plan(&[(rock.as_str(), 3.5 / t, 4)], &[(rock.as_str(), 2)]);
    let err = check(&plan, &items, &counts, Some(&ElectricConfig::new(5))).expect_err("1.1 days for one Aniimo");
    assert!(err.contains("days a day"), "{err}");
}

// Powered units cannot lend hours across recipes (P3 seen from the re-check): one Earth Aniimo
// runs rock on two unpowered Mines (2.0 days) while a third Mine, powered, barely runs clay (0.1
// units of work). Counting clay as `0.1 - 1.2` would leave 0.9 days and accept it; the honest
// count is 2.0 days. Must be rejected before and after the P2 fix. (Since powered units must be
// kept running, the barely-run clay is now rejected for that first; the second plan runs clay's
// powered unit fully, so only the hours check stands between it and acceptance.)
#[test]
fn p2_recheck_rejects_power_lending_hours_to_other_recipes() {
    let Some((items, counts, rock, clay)) = hours_case(3) else { return };
    let (t_rock, t_clay) = (production_time(&items, &rock), production_time(&items, &clay));
    let plan = hours_plan(&[(rock.as_str(), 2.0 / t_rock, 2), (clay.as_str(), 0.1 / t_clay, 1)], &[(clay.as_str(), 1)]);
    let err = check(&plan, &items, &counts, Some(&ElectricConfig::new(5))).expect_err("2.0 days for one Aniimo");
    assert!(err.contains("days a day") || err.contains("keep them running"), "{err}");
    let full = hours_plan(&[(rock.as_str(), 2.0 / t_rock, 2), (clay.as_str(), 1.2 / t_clay, 1)], &[(clay.as_str(), 1)]);
    let err = check(&full, &items, &counts, Some(&ElectricConfig::new(5))).expect_err("still 2.0 days for one Aniimo");
    assert!(err.contains("days a day"), "{err}");
}

// KNOWN BUG P2, close to the local in-game report ("roster member 0 works 6.8 days a day but there
// are 5 of it"): five Earth Aniimo, one Lightning, seven Lv.4 Mines, Lv.5 generator (TEST FAKE
// DATA; the reported setup itself was not available). Today the model powers all seven Mines and
// the re-check says "works 8.4 days a day but there are 5 of it". Expected after the fix: the
// model's plan passes, and nobody really works more than their count.
#[test]
fn p2_near_local_report_plan_passes_recheck() {
    let base = FacilityCounts::only(&[("Mine", 7, 4)]);
    let crew = crew_of(&[(5, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let Some((items, counts)) = with_crew(&base, crew.clone()) else { return };
    let power = ElectricConfig::new(5);
    let plan = plan_with_power(&items, &counts, Some(&power)).expect("the model finds a plan");
    for (member, (&used, aniimo)) in honest_busy(&plan, &items, &crew).iter().zip(&crew.members).enumerate() {
        assert!(used <= aniimo.count as f64 + 1e-6, "member {member} really works {used} days");
    }
    check(&plan, &items, &counts, Some(&power)).expect("model plan passes its re-check");
}

// The page relies on an empty model WITHOUT `error` meaning "nothing to solve here" (worker.js:
// `if (!problem.lp) ...`); only a rejected input carries `error`. These lock that in.
#[test]
fn wasm_empty_models_carry_no_error() {
    let Some(items) = load_items() else { return };
    let input = |currency: &str| {
        serde_json::json!({
            "currency": currency,
            "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
            "prioritize_byproducts": false,
            "home_level": 12,
            "electric_enabled": true,
            "generator_level": 2,
            "aniimo": "best",
        })
        .to_string()
    };
    let no_currency: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_problem(&input(""), "{}")).unwrap();
    assert_eq!(no_currency["lp"], "", "{no_currency}");
    assert!(no_currency.get("error").is_none(), "{no_currency}");
    let no_level_up: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_level_up_problem(&input("coins"))).unwrap();
    assert_eq!(no_level_up["lp"], "", "{no_level_up}");
    assert!(no_level_up.get("error").is_none(), "{no_level_up}");
    assert_eq!(aniimax::wasm::exact_byproduct_problems(&input("coins")), "[]");
}

/// The rate variable of the recipe whose boosted variable is `boosted`, read off its capacity row
/// (`t*rate - units - 0.2*boosted <= 0`: the one positive term).
fn rate_of_boosted(lp: &str, boosted: usize) -> Option<usize> {
    lp_constraints(lp).into_iter().find_map(|(terms, op, rhs)| {
        (op == "<=" && rhs == 0.0 && terms.len() == 3 && terms.contains(&(boosted, -0.2)))
            .then(|| terms.iter().find(|(_, c)| *c > 0.0).map(|(v, _)| *v))
            .flatten()
    })
}

// P3 (lending), forced. Left alone the solver never parks power on an idle unit (stage A), so this
// asks the model directly whether it ALLOWS that: for each Mine recipe in turn, one unit is
// powered while the recipe makes nothing and no other recipe is powered. The resulting plan
// (`plan_from_values_with_power` re-solves its rates in the model itself) must still fit every
// roster member's honest day. TEST FAKE DATA: three Lv.4 Mines, one Earth and one Lightning
// Aniimo, Lv.5 generator. Without a floor on the E-mode deduction, the idle unit's -1.2 lends the
// Earth Aniimo 1.2 days for the other two Mines. A powered unit may wait for inputs (it keeps its
// fixed recipe), so an idle powered unit is a plan; every one of them must fit the honest days.
#[test]
fn p3_forced_idle_power_lends_no_hours() {
    let base = FacilityCounts::only(&[("Mine", 3, 4)]);
    let crew = crew_of(&[(1, &[("Earth", 4)]), (1, &[("Lightning", 1)])]);
    let Some((items, counts)) = with_crew(&base, crew.clone()) else { return };
    let power = ElectricConfig::new(5);
    let modules = ModuleLevels::default();
    let goal = Goal::Earn { floors: &[] };
    let problem = write_lp_with_power(&items, "coins", &counts, &modules, goal, Some(&power));
    let pairs = powered_boosted_pairs(&problem.lp);
    assert!(!pairs.is_empty(), "no powered/boosted pair found in the model");
    let mut solved = 0;
    for (i, &(p, b)) in pairs.iter().enumerate() {
        let rate = rate_of_boosted(&problem.lp, b).expect("capacity row of a powered recipe");
        let mut lp = with_extra_row(&problem.lp, &format!("+ 1 x{rate} = 0"));
        lp = with_extra_row(&lp, &format!("+ 1 x{p} >= 1"));
        for (j, &(q, _)) in pairs.iter().enumerate() {
            if j != i {
                lp = with_extra_row(&lp, &format!("+ 1 x{q} <= 0"));
            }
        }
        let Ok((values, objective)) = solve_lp(&LpProblem { lp, ..problem.clone() }) else { continue };
        solved += 1;
        let plan = plan_from_values_with_power(&items, "coins", &counts, &modules, goal, &values, true, objective, Some(&power))
            .expect("a plan from the forced values");
        for (member, (&used, aniimo)) in honest_busy(&plan, &items, &crew).iter().zip(&crew.members).enumerate() {
            assert!(used <= aniimo.count as f64 + 1e-6, "forced idle x{p}: member {member} really works {used} days: {plan:?}");
        }
    }
    assert!(solved > 0, "an idle powered unit should be allowed");
}

// ---------------------------------------------------------------------------------------------
// 7. Grid capacity (generator count) and E-mode's three priorities
// ---------------------------------------------------------------------------------------------
//
// Confirmed rules: generators 1 / 2 / 3 at RV12-14 / 15-17 / 18-20, the grid's output is count x
// one generator's, powered units may draw up to that over 1.2, and E-mode runs at a fixed 120%.
// Priorities: most powered units (idle ones count), then most watts drawn, then the plan's own goal.
// Facility counts and levels below are TEST FAKE DATA.

/// E-mode ahead of coins: the most powered units, then the most watts with that many, then coins
/// with both kept. The page's worker no longer plans coins in this order (see `revenue_first_plan`);
/// these tests keep checking the two E-mode goals themselves. Returns the plan and the two floors.
fn staged_plan(items: &[ProductionItem], counts: &FacilityCounts, power: ElectricConfig) -> (ExactPlan, u32, u32) {
    let modules = ModuleLevels::default();
    let solve = |goal: Goal, power: &ElectricConfig| -> u32 {
        let problem = write_lp_with_power(items, "coins", counts, &modules, goal, Some(power));
        let (_, objective) = solve_lp(&problem).expect("E-mode stage solves");
        objective.round().max(0.0) as u32
    };
    let most_powered = solve(Goal::MostPowered, &power);
    let most_draw = solve(Goal::MostPowerDraw, &power.with_floors(most_powered, 0));
    let plan = plan_with_power(items, counts, Some(&power.with_floors(most_powered, most_draw))).expect("final plan");
    (plan, most_powered, most_draw)
}

fn powered_units(plan: &ExactPlan) -> u32 {
    plan.electric_units.values().sum()
}

fn powered_facilities(plan: &ExactPlan, items: &[ProductionItem]) -> Vec<String> {
    let mut facilities: Vec<String> = plan
        .electric_units
        .keys()
        .map(|name| items.iter().find(|i| &i.name == name).unwrap().facility.clone())
        .collect();
    facilities.sort();
    facilities.dedup();
    facilities
}

// Priority 1 over revenue. Lv.1 generator (limit 500 W); five Lv.4 Mines (120 W each) and three
// Lv.1 Wells (30 W each). The most units: all three Wells and three Mines, 6 units at 450 W. A
// coins-only plan would rather power four Mines (480 W, 4 units): copper earns more than well water.
#[test]
fn tiers_most_powered_units_come_before_revenue() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 5, 4), ("Well", 3, 1)]);
    let power = ElectricConfig::new(1);
    let coins_only = plan_with_power(&items, &counts, Some(&power)).expect("coins-only plan");
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert_eq!((most_powered, most_draw), (6, 450));
    assert_eq!(powered_units(&plan), 6, "{:?}", plan.electric_units);
    assert_eq!(demand(&plan, &items, &counts), 450.0);
    assert!(powered_units(&coins_only) < 6, "coins alone already powered six: {:?}", coins_only.electric_units);
    assert!(plan.rate_per_second <= coins_only.rate_per_second + 1e-9, "priority 1 may cost coins, never add them");
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// Priority 2: with the unit count fixed, the most watts up to the limit. Lv.1 generator (500 W);
// three Lv.4 Mines (120 W) and three Lv.5 Wells (150 W). Four units would need at least 510 W, so
// three is the most; of the three-unit choices (360 / 390 / 420 / 450 W) the closest to 500 W is all
// three Wells, 450 W.
#[test]
fn tiers_most_draw_among_equal_unit_counts() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 3, 4), ("Well", 3, 5)]);
    let power = ElectricConfig::new(1);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert_eq!((most_powered, most_draw), (3, 450));
    assert_eq!(powered_units(&plan), 3);
    assert_eq!(demand(&plan, &items, &counts), 450.0);
    assert_eq!(powered_facilities(&plan, &items), vec!["Well".to_string()]);
}

// Priority 3: where the first two don't get in the way, revenue is what it was. One Lv.4 Mine on a
// Lv.2 generator: powering it is also the best for coins.
#[test]
fn tiers_keep_revenue_when_power_costs_nothing() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 1, 4)]);
    let power = ElectricConfig::new(2);
    let coins_only = plan_with_power(&items, &counts, Some(&power)).expect("coins-only plan");
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert_eq!((most_powered, most_draw), (1, 120));
    assert!((plan.rate_per_second - coins_only.rate_per_second).abs() < 1e-9, "{} vs {}", plan.rate_per_second, coins_only.rate_per_second);
}

// RV16 grid (Lv.3 x 2 = 2000 W, limit 1666.67 W): ten Lv.4 Mines (120 W) and four Lv.5 Wells
// (150 W). The most units: ten Mines and three Wells, 13 units at 1650 W (14 would need 1800 W; nine
// Mines and four Wells would be 1680 W). Far above what one generator (1000 W) would allow.
#[test]
fn rv16_two_generators_allow_up_to_their_limit() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 10, 4), ("Well", 4, 5)]);
    let power = ElectricConfig::new(3).with_generators(2);
    assert!((power.boost_threshold() - 1666.67).abs() < 0.01);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert_eq!((most_powered, most_draw), (13, 1650));
    assert_eq!(demand(&plan, &items, &counts), 1650.0);
    assert!(demand(&plan, &items, &counts) > ElectricConfig::new(3).capacity());
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// A powered recipe may wait for inputs, but they must have a source: two Lv.4 Woodworking Benches and
// two Lv.4 Chimney Kilns with no Woodland and no Mine (no Wood Blocks, no Mineral Sand) get no power;
// two Lv.5 Pickling Jars with nothing to pickle leave only the Lv.4 Mine next to them powered.
#[test]
fn powered_recipes_need_a_source() {
    let Some(items) = load_items() else { return };
    let power = ElectricConfig::new(5);
    let idle_chain = FacilityCounts::only(&[("Woodworking Bench", 2, 4), ("Chimney Kiln", 2, 4)]);
    let (plan, most_powered, _) = staged_plan(&items, &idle_chain, power);
    assert_eq!(most_powered, 0);
    assert!(plan.electric_units.is_empty(), "{:?}", plan.electric_units);
    let jars = FacilityCounts::only(&[("Pickling Jar", 2, 5), ("Mine", 1, 4)]);
    let (plan, most_powered, most_draw) = staged_plan(&items, &jars, power);
    assert_eq!(most_powered, 1);
    assert_eq!(powered_facilities(&plan, &items), vec!["Mine".to_string()]);
    check(&plan, &items, &jars, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// Bamboo Joss Sticks take Bamboo: with no Bamboo grown (Lv.4 Phonolfactory Tables next to Lv.5 Mines
// and Woodland that grows no Bamboo here), no table is powered on them, however much room the grid
// has; with a recipe whose inputs are made (Woodland for the Dewy House's Aromathyst and Lemons), a
// table may be powered and wait.
#[test]
fn bamboo_joss_sticks_need_bamboo() {
    let Some(items) = load_items() else { return };
    let items: Vec<ProductionItem> = items.into_iter().filter(|i| i.name != "bamboo").collect();
    let counts = FacilityCounts::only(&[("Phonolfactory Table", 2, 4), ("Mine", 1, 4), ("Woodland", 4, 5), ("Dewy House", 1, 2)]);
    let power = ElectricConfig::new(5);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert!(!plan.electric_units.contains_key("bamboo_joss_stick"), "{:?}", plan.electric_units);
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
    // Forced on, there's no plan: Bamboo has no source.
    let problem = write_lp_with_power(&items, "coins", &counts, &ModuleLevels::default(), Goal::MostPowered, Some(&power));
    let (values, _) = solve_lp(&problem).expect("solves");
    let mut forced = plan_from_values_with_power(&items, "coins", &counts, &ModuleLevels::default(), Goal::MostPowered, &values, true, 0.0, Some(&power)).unwrap();
    forced.units.insert("bamboo_joss_stick".into(), 1);
    forced.electric_units.insert("bamboo_joss_stick".into(), 1);
    let err = check(&forced, &items, &counts, Some(&power)).expect_err("a powered Bamboo Joss Stick table with no Bamboo");
    assert!(err.contains("bamboo_joss_stick"), "{err}");
}

// Level-up stock is a source too, used once: one Lv.3 Woodworking Bench and 80 Standard Planks in
// stock, for 10 Laminated Beams. The Bench is powered on Laminated Beams with no plank made in the
// plan; over the level-up it uses no more planks than are in stock.
#[test]
fn level_up_stock_is_a_source_used_once() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Woodworking Bench", 1, 3), ("Mine", 1, 4)]);
    let level_up = aniimax::exact::LevelUp { cost: vec![("laminated_beams".into(), 10.0)], stock: vec![("standard_planks".into(), 80.0)] };
    let power = ElectricConfig::new(3);
    let (plan, pace) = staged_level_up(&items, &counts, &level_up, power);
    assert!(pace > 0.0);
    assert_eq!(plan.electric_units.get("laminated_beams"), Some(&1), "{:?}", plan.electric_units);
    let seconds = aniimax::exact::PACE_UNIT / plan.pace.unwrap();
    let used = plan.recipe_rates.get("laminated_beams").copied().unwrap_or(0.0) * 8.0 * seconds;
    assert!(used <= 80.0 + 1e-6, "{used} planks used of 80 in stock");
    check_plan_with_power(&plan, &items, "coins", &counts, &ModuleLevels::default(), Some(&level_up), Some(&power))
        .expect("passes its re-check");
    // With no stock and nothing to make planks from, the Bench isn't powered.
    let none = aniimax::exact::LevelUp { cost: vec![("laminated_beams".into(), 10.0)], stock: vec![] };
    let (plan, _, _) = staged_plan(&items, &counts, power);
    let _ = none;
    assert!(!plan.electric_units.contains_key("laminated_beams"), "{:?}", plan.electric_units);
}

// With a supply, the Woodworking Bench and Chimney Kiln can be powered (their level-up recipes take
// turns): a Lv.1 Bench and a Lv.1 Kiln, thirty Lv.5 Woodland plots for Wood Blocks and twelve Lv.5
// Mines for Mineral Sand, on a Lv.5 generator (1250 W). Both get power, each kept running.
#[test]
fn bench_and_kiln_can_be_powered() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Woodworking Bench", 1, 1), ("Chimney Kiln", 1, 1), ("Woodland", 30, 5), ("Mine", 12, 5)]);
    let power = ElectricConfig::new(5);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    let facilities = powered_facilities(&plan, &items);
    assert!(facilities.contains(&"Woodworking Bench".to_string()) && facilities.contains(&"Chimney Kiln".to_string()), "{facilities:?}");
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// The three strategies: two Lv.4 Mines, and a Lv.1 Woodworking Bench fed by thirty Lv.5 Woodland
// plots, on a Lv.5 generator. Raw materials powers only the Mines, workstations only the Bench,
// combined all three.
#[test]
fn strategies_limit_what_can_be_powered() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 2, 4), ("Woodworking Bench", 1, 1), ("Woodland", 30, 5)]);
    for (strategy, units, facilities) in [
        (1, 2, vec!["Mine"]),
        (2, 1, vec!["Woodworking Bench"]),
        (0, 3, vec!["Mine", "Woodworking Bench"]),
    ] {
        let power = ElectricConfig::new_with_strategy(5, strategy);
        let (plan, most_powered, _) = staged_plan(&items, &counts, power);
        assert_eq!(most_powered, units, "strategy {strategy}");
        assert_eq!(powered_facilities(&plan, &items), facilities, "strategy {strategy}");
    }
}

// The re-check holds the same rule: a powered unit may make less than its 120%, or nothing while
// it waits, but needs a unit of its own recipe, makes no more than 120%, and the powered units
// together stay within the limit. Lv.1 generator (500 W), Lv.4 Mines (120 W).
#[test]
fn recheck_allows_idle_powered_units() {
    let Some(items) = load_items() else { return };
    let power = ElectricConfig::new(1);
    let t = production_time(&items, "rock");
    let two = FacilityCounts::only(&[("Mine", 2, 4)]);
    let running = manual_plan(&[("rock", 1.2 / t, 1)], &[("rock", 1)]);
    check(&running, &items, &two, Some(&power)).expect("one powered Mine at 120%");
    let underused = manual_plan(&[("rock", 1.0 / t, 1)], &[("rock", 1)]);
    check(&underused, &items, &two, Some(&power)).expect("a powered Mine doing 1.0 units of work");
    let idle = manual_plan(&[("rock", 1.2 / t, 1), ("clay", 0.0, 1)], &[("rock", 1), ("clay", 1)]);
    check(&idle, &items, &two, Some(&power)).expect("clay powered, waiting");
    let no_unit = manual_plan(&[("rock", 1.2 / t, 1)], &[("rock", 1), ("clay", 1)]);
    let err = check(&no_unit, &items, &two, Some(&power)).expect_err("clay powered without a unit on it");
    assert!(err.contains("only 0 units"), "{err}");
    let beyond = manual_plan(&[("rock", 1.3 / t, 1)], &[("rock", 1)]);
    check(&beyond, &items, &two, Some(&power)).expect_err("one powered Mine can't make more than 120%");
    // A powered recipe with inputs makes at least a batch a day from them: a Bench powered on Rough
    // Lumber with no Wood Blocks at all doesn't pass.
    let bench = FacilityCounts::only(&[("Mine", 2, 4), ("Woodworking Bench", 1, 1)]);
    let starved = manual_plan(&[("rock", 1.2 / t, 1), ("rough_lumber", 0.0, 1)], &[("rock", 1), ("rough_lumber", 1)]);
    let err = check(&starved, &items, &bench, Some(&power)).expect_err("rough_lumber powered with no input");
    assert!(err.contains("batches a day"), "{err}");
    let five = FacilityCounts::only(&[("Mine", 5, 4)]);
    let over = manual_plan(&[("rock", 6.0 / t, 5)], &[("rock", 5)]);
    let err = check(&over, &items, &five, Some(&power)).expect_err("600 W > 500 W");
    assert!(err.contains("120% limit"), "{err}");
}

// The Aniipod Maker and Dance Pad Polisher get E-mode only through their own switches, off unless
// turned on. A Lv.1 Aniipod Maker (fed Rock by two Lv.4 Mines) and a Lv.1 Dance Pad Polisher (fed
// Potatoes by eight Lv.2 Farmland plots), Lv.5 generator: switched off, only the Mines are
// powered; switched on, both join them.
#[test]
fn special_facilities_need_their_own_switches() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Aniipod Maker", 1, 1), ("Dance Pad Polisher", 1, 1), ("Mine", 2, 4), ("Farmland", 8, 2)]);
    let off = ElectricConfig::new(5);
    assert!(!off.aniipod_maker && !off.dance_pad_polisher, "both off by default");
    assert!(!off.allows_facility("Aniipod Maker") && !off.allows_facility("Dance Pad Polisher"));
    assert!(off.allows_facility("Mine") && off.allows_facility("Carousel Mill"));
    let (plan, _, _) = staged_plan(&items, &counts, off);
    assert_eq!(powered_facilities(&plan, &items), vec!["Mine".to_string()]);
    let on = off.with_special_facilities(true, true);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, on);
    assert_eq!(powered_facilities(&plan, &items), vec!["Aniipod Maker".to_string(), "Dance Pad Polisher".to_string(), "Mine".to_string()]);
    check(&plan, &items, &counts, Some(&on.with_floors(most_powered, most_draw))).expect("passes its re-check");
    let only_pods = off.with_special_facilities(true, false);
    let (plan, _, _) = staged_plan(&items, &counts, only_pods);
    assert_eq!(powered_facilities(&plan, &items), vec!["Aniipod Maker".to_string(), "Mine".to_string()]);
}

// The page's input leaves both switches off unless it sends them: the most powered units count
// only the Mines; with `electric_aniipod_maker` the Aniipod Maker joins.
#[test]
fn wasm_special_switches_default_off() {
    let Some(items) = load_items() else { return };
    let input = |aniipod: Option<bool>| {
        let mut input = serde_json::json!({
            "currency": "coins",
            "facilities": page_facilities(&items, &[("Aniipod Maker", 1, 1), ("Mine", 2, 4)]),
            "prioritize_byproducts": false,
            "home_level": 16,
            "electric_enabled": true,
            "generator_level": 3,
            "aniimo": "best",
        });
        if let Some(on) = aniipod {
            input["electric_aniipod_maker"] = serde_json::json!(on);
        }
        input.to_string()
    };
    let most = |input: String| -> f64 {
        let problem: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_electric_problem(&input, "{}", "units")).unwrap();
        let lp = LpProblem { lp: problem["lp"].as_str().unwrap().to_string(), variables: problem["variables"].as_u64().unwrap() as usize, tiebreak: Vec::new() };
        solve_lp(&lp).expect("solves").1.round()
    };
    assert_eq!(most(input(None)), 2.0, "left out: off");
    assert_eq!(most(input(Some(false))), 2.0);
    assert_eq!(most(input(Some(true))), 3.0);
}

// The page's path: the generator count follows `home_level` unless the page sends it, and the
// summary reports the whole grid. RV16, Lv.3: 2 x 1000 W = 2000 W, limit 1666.67 W.
#[test]
fn wasm_summary_reports_the_whole_grid() {
    let Some(items) = load_items() else { return };
    let input = |count: Option<u8>| {
        let mut input = serde_json::json!({
            "currency": "coins",
            "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
            "prioritize_byproducts": false,
            "home_level": 16,
            "electric_enabled": true,
            "generator_level": 3,
            "aniimo": "best",
        });
        if let Some(count) = count {
            input["generator_count"] = serde_json::json!(count);
        }
        input
    };
    let plan = wasm_plan(&input(None));
    assert_eq!(plan["success"], true, "{plan}");
    assert_eq!(plan["electric_generator_count"], 2);
    assert_eq!(plan["electric_generator_unit_capacity"], 1000.0);
    assert_eq!(plan["electric_generator_capacity"], 2000.0);
    assert!((plan["electric_boost_threshold"].as_f64().unwrap() - 1666.67).abs() < 0.01, "{plan}");
    let one = wasm_plan(&input(Some(1)));
    assert_eq!(one["electric_generator_capacity"], 1000.0);
}

// The new E-mode model entry: a model with E-mode on, none with it off, and a rejected roster's
// reason passed on as it is.
#[test]
fn wasm_electric_problem_entry() {
    let Some(items) = load_items() else { return };
    let base = |electric: bool| {
        serde_json::json!({
            "currency": "coins",
            "facilities": page_facilities(&items, &[("Mine", 1, 4)]),
            "prioritize_byproducts": false,
            "home_level": 16,
            "electric_enabled": electric,
            "generator_level": 3,
            "aniimo": "best",
        })
        .to_string()
    };
    let on: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_electric_problem(&base(true), "{}", "units")).unwrap();
    assert!(on["lp"].as_str().is_some_and(|lp| !lp.is_empty()) && on.get("error").is_none(), "{on}");
    let off: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_electric_problem(&base(false), "{}", "units")).unwrap();
    assert_eq!(off["lp"], "");
    assert!(off.get("error").is_none(), "{off}");
    let rejected = roster_input(&items, 16, true, serde_json::json!([{ "count": 1, "abilities": { "Earth": 4 } }])).to_string();
    let rejected: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_electric_problem(&rejected, "{}", "units")).unwrap();
    assert!(rejected["error"].as_str().is_some_and(|e| e.contains("Lightning")), "{rejected}");
}

// ---------------------------------------------------------------------------------------------
// 8. The soonest level-up stays first
// ---------------------------------------------------------------------------------------------

/// Solves `goal` with `power` and builds its plan, as the page does with any one model.
fn solve_goal(items: &[ProductionItem], counts: &FacilityCounts, goal: Goal, power: &ElectricConfig) -> (ExactPlan, f64) {
    let modules = ModuleLevels::default();
    let problem = write_lp_with_power(items, "coins", counts, &modules, goal, Some(power));
    let (values, objective) = solve_lp(&problem).expect("solves");
    let plan = plan_from_values_with_power(items, "coins", counts, &modules, goal, &values, true, objective, Some(power)).expect("plan");
    (plan, objective)
}

/// The level-up strategy as the page's worker now runs it: the soonest level-up's pace first,
/// then E-mode's two floors keeping that pace, then coins with all three kept. Returns the plan
/// and the pace found first.
fn staged_level_up(items: &[ProductionItem], counts: &FacilityCounts, level_up: &aniimax::exact::LevelUp, power: ElectricConfig) -> (ExactPlan, f64) {
    let (_, pace) = solve_goal(items, counts, Goal::LevelUp(level_up), &power);
    let (_, most) = solve_goal(items, counts, Goal::MostPoweredLevelingUp(level_up, pace), &power);
    let power = power.with_floors(most.round() as u32, 0);
    let (_, draw) = solve_goal(items, counts, Goal::MostPowerDrawLevelingUp(level_up, pace), &power);
    let power = power.with_floors(most.round() as u32, draw.round() as u32);
    let (plan, _) = solve_goal(items, counts, Goal::EarnWhileLevelingUp(level_up, pace), &power);
    (plan, pace)
}

/// TEST FAKE DATA for the level-up tests: an RV16-like chain, three Lv.3 Woodworking Benches and
/// three Chimney Kilns (one per tier: every unit keeps to one recipe), sixteen Lv.5 Woodland plots, three Lv.5 Mines (few enough that Mineral Sand is
/// short) and a Lv.1 Aniipod Maker, with its E-mode switch on, on an RV16 grid (Lv.3 x 2). The
/// level-up is RV17's materials (630 Laminated Beams, 380 Refined Ore); its coins are left out to
/// look at the materials alone.
fn level_up_case() -> Option<(Vec<ProductionItem>, FacilityCounts, aniimax::exact::LevelUp, ElectricConfig)> {
    let items = load_items()?;
    let counts = FacilityCounts::only(&[
        ("Woodworking Bench", 3, 3),
        ("Chimney Kiln", 3, 3),
        ("Woodland", 16, 5),
        ("Mine", 3, 5),
        ("Aniipod Maker", 1, 1),
    ]);
    let level_up = aniimax::exact::LevelUp { cost: vec![("laminated_beams".into(), 630.0), ("refined_ore".into(), 380.0)], stock: vec![] };
    let power = ElectricConfig::new(3).with_generators(2).with_special_facilities(true, false);
    Some((items, counts, level_up, power))
}

// The soonest level-up comes before E-mode's priorities. Asking for the most powered units first
// (the order used for coins) would keep the Aniipod Maker running on Rock, taking a Mine off
// Quartz Ore and its Mineral Sand: a slower level-up than with no E-mode at all. Keeping the pace
// first, E-mode only helps it, and the Mines stay on the recipe with the most Mineral Sand per
// second.
#[test]
fn level_up_pace_comes_before_e_mode_priorities() {
    let Some((items, counts, level_up, power)) = level_up_case() else { return };
    let (_, pace_off) = solve_goal(&items, &counts, Goal::LevelUp(&level_up), &ElectricConfig::disabled());
    let (plan, pace) = staged_level_up(&items, &counts, &level_up, power);
    assert!(plan.pace.unwrap() >= pace * (1.0 - 1e-3), "final pace {:?} vs {pace}", plan.pace);
    assert!(pace >= pace_off - 1e-9, "E-mode slowed the level-up: {pace} vs {pace_off} without it");
    // The old order, for comparison: E-mode floors first, then the level-up. A powered unit may
    // wait for inputs, so E-mode first no longer has to keep the Aniipod Maker busy on Rock; it
    // still never beats keeping the pace first.
    let (_, most) = solve_goal(&items, &counts, Goal::MostPowered, &power);
    let first = power.with_floors(most.round() as u32, 0);
    let (_, draw) = solve_goal(&items, &counts, Goal::MostPowerDraw, &first);
    let (_, pace_old) = solve_goal(&items, &counts, Goal::LevelUp(&level_up), &power.with_floors(most.round() as u32, draw.round() as u32));
    assert!(pace >= pace_old * (1.0 - 1e-9), "keeping the pace first should never lose to E-mode first: {pace} vs {pace_old}");
    // The Mine recipe with the most Mineral Sand per second at Lv.5, from the data itself.
    let best_sand = items
        .iter()
        .filter(|i| i.facility == "Mine" && i.facility_level <= 5 && i.module_requirement.is_none())
        .max_by(|a, b| {
            let sand = |i: &ProductionItem| i.byproduct.as_ref().map_or(0.0, |(_, amount)| *amount as f64) / i.production_time;
            sand(a).partial_cmp(&sand(b)).unwrap()
        })
        .unwrap();
    let mine_recipes: Vec<&String> = plan.units.keys().filter(|n| items.iter().any(|i| &i.name == *n && i.facility == "Mine")).collect();
    assert_eq!(mine_recipes, vec![&best_sand.name], "Mines should all make {}", best_sand.name);
}

// Both RV level-up materials are made, through their whole chains: Wood Blocks into Rough Lumber,
// Standard Planks and Laminated Beams; Mineral Sand into Coarse-Sifted Ore, Sintered Ore Brick and
// Refined Ore. The plan passes its re-check with every floor kept.
#[test]
fn level_up_supplies_both_target_materials() {
    let Some((items, counts, level_up, power)) = level_up_case() else { return };
    let (plan, _) = staged_level_up(&items, &counts, &level_up, power);
    for recipe in ["rough_lumber", "standard_planks", "laminated_beams", "coarse_sifted_ore", "sintered_ore_brick", "refined_ore"] {
        assert!(plan.recipe_rates.get(recipe).copied().unwrap_or(0.0) > 1e-9, "{recipe} isn't made: {:?}", plan.recipe_rates);
    }
    assert!(plan.pace.unwrap() > 0.0);
    check_plan_with_power(&plan, &items, "coins", &counts, &ModuleLevels::default(), Some(&level_up), Some(&power))
        .expect("passes its re-check");
}

// ---------------------------------------------------------------------------------------------
// 9. Regressions from the RV16 E-mode diagnosis
// ---------------------------------------------------------------------------------------------

/// The data's items with the Phonolfactory Table left only its Cherry Incense recipe, so what's
/// asked of it is that one recipe's supply: Dried Cherry Blossom (Jukebox Dryer, from Woodland's
/// Cherry Blossom) and Aromathyst (Dewy House).
fn cherry_incense_items() -> Option<Vec<ProductionItem>> {
    let mut items = load_items()?;
    items.retain(|i| i.facility != "Phonolfactory Table" || i.name == "cherry_incense");
    Some(items)
}

/// TEST FAKE DATA: two Lv.2 Phonolfactory Tables, a Lv.3 Jukebox Dryer, `woodland` Lv.3 Woodland
/// plots and `dewy` Dewy Houses, on a Lv.5 generator.
fn cherry_incense_counts(woodland: u32, dewy: u32) -> FacilityCounts {
    FacilityCounts::only(&[("Phonolfactory Table", 2, 2), ("Jukebox Dryer", 1, 3), ("Woodland", woodland, 3), ("Dewy House", dewy, 1)])
}

// With the supply it has (four Woodland plots, one Dewy House), Cherry Incense is made, but not
// enough to keep a Phonolfactory Table busy all day at 120%. The table may still be powered: it
// waits for inputs, and makes no more than the supply allows (the same as without power). The
// counts are TEST FAKE DATA; how much each makes is the model's own.
#[test]
fn cherry_incense_short_of_inputs_can_still_be_powered() {
    let Some(items) = cherry_incense_items() else { return };
    let counts = cherry_incense_counts(4, 1);
    let power = ElectricConfig::new(5);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert!(plan.recipe_rates.get("cherry_incense").copied().unwrap_or(0.0) > 1e-9, "Cherry Incense should still be made: {:?}", plan.recipe_rates);
    assert!(powered_facilities(&plan, &items).contains(&"Phonolfactory Table".to_string()), "{:?}", plan.electric_units);
    let unpowered = plan_with_power(&items, &counts, None).expect("plan without E-mode");
    let made = |plan: &ExactPlan| plan.recipe_rates.get("cherry_incense").copied().unwrap_or(0.0);
    assert!(made(&plan) <= made(&unpowered) * (1.0 + 1e-6) + 1e-9, "power made incense from nothing: {} vs {}", made(&plan), made(&unpowered));
    let _ = phonolfactory_powered_vars(&items, &counts, &power);
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// With supply enough for a table at 120% (sixty Woodland plots, thirty Dewy Houses; TEST FAKE
// DATA), the same recipe qualifies: a Phonolfactory Table is powered, kept running on Cherry Incense.
#[test]
fn cherry_incense_with_enough_inputs_is_powered() {
    let Some(items) = cherry_incense_items() else { return };
    let counts = cherry_incense_counts(60, 30);
    let power = ElectricConfig::new(5);
    let (plan, most_powered, most_draw) = staged_plan(&items, &counts, power);
    assert!(powered_facilities(&plan, &items).contains(&"Phonolfactory Table".to_string()), "{:?}", plan.electric_units);
    assert!(plan.electric_units.get("cherry_incense").copied().unwrap_or(0) >= 1, "{:?}", plan.electric_units);
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

/// The Phonolfactory Tables' powered variables in the `MostPowered` model for
/// [`cherry_incense_counts`]: the terms of the model's own grid limit row
/// (`sum(watts * powered) <= limit`) at the table's wattage. The only other electric facility
/// there, the Jukebox Dryer, draws a different wattage, so they can't be confused.
fn phonolfactory_powered_vars(items: &[ProductionItem], counts: &FacilityCounts, power: &ElectricConfig) -> Vec<usize> {
    let watts = electric_require("Phonolfactory Table", counts.get_level("Phonolfactory Table")).unwrap();
    assert_ne!(Some(watts), electric_require("Jukebox Dryer", counts.get_level("Jukebox Dryer")));
    let problem = write_lp_with_power(items, "coins", counts, &ModuleLevels::default(), Goal::MostPowered, Some(power));
    let limit_row = lp_constraints(&problem.lp)
        .into_iter()
        .find(|(terms, op, rhs)| op == "<=" && (*rhs - power.boost_threshold()).abs() < 1e-6 && !terms.is_empty())
        .map(|(terms, _, _)| terms)
        .expect("the grid's limit row");
    let vars: Vec<usize> = limit_row.into_iter().filter(|(_, w)| (*w - watts).abs() < 1e-9).map(|(v, _)| v).collect();
    assert!(!vars.is_empty(), "no Phonolfactory powered variable found");
    vars
}

/// The page's Simple-mode RV16 input with E-mode on, combined strategy, Best Aniimo, RV16's grid
/// (Lv.3 x 2) and modules, cut down to its E-mode core: the facilities, counts and levels below are
/// `simpleSetup(16)`'s (web/facility-config.js) for the Mines and Wells the RV16 diagnosis powered
/// and for the incense chain (Phonolfactory Table, Jukebox Dryer, Dewy House, Woodland). The whole
/// RV16 homeland is too big a model for this harness's solver (microlp gives up on it after its
/// time limit); the page solves it with HiGHS.
fn rv16_simple_input(items: &[ProductionItem]) -> serde_json::Value {
    let owned = [
        ("Woodland", 17, 5), ("Mine", 8, 5), ("Well", 3, 4), ("Dewy House", 1, 2), ("Jukebox Dryer", 2, 6),
        ("Phonolfactory Table", 2, 4),
    ];
    serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(items, &owned),
        "modules": { "ecological_module": 6, "kitchen_module": 6, "resource_detector": 6, "crafting_module": 4 },
        "prioritize_byproducts": false,
        "home_level": 16,
        "electric_enabled": true,
        "generator_level": 3,
        "generator_count": 2,
        "electric_strategy": "combined",
        "aniimo": "best",
    })
}

/// Solves one of the page's models (from a `exact_*_problem` export): its objective.
fn page_objective(problem_json: &str) -> f64 {
    let problem: serde_json::Value = serde_json::from_str(problem_json).unwrap();
    let lp = LpProblem { lp: problem["lp"].as_str().unwrap().to_string(), variables: problem["variables"].as_u64().unwrap() as usize, tiebreak: Vec::new() };
    solve_lp(&lp).expect("the model solves").1
}

// RV16 (its E-mode core, see `rv16_simple_input`), combined strategy: E-mode's priorities (most
// powered units, then most watts) cost no coins against the plan that only looks at coins, and
// power at least as many units. Both plans come from the page's own models; no coin figure is
// written down here.
#[test]
#[ignore = "microlp panics (solver.rs:787) on this RV16 model since powered units may wait for inputs; the page checks the same with HiGHS"]
fn rv16_combined_e_mode_order_keeps_revenue() {
    let Some(items) = load_items() else { return };
    let input = rv16_simple_input(&items);
    let coins_only = wasm_plan(&input);
    assert_eq!(coins_only["success"], true, "{coins_only}");
    let most_powered = page_objective(&aniimax::wasm::exact_electric_problem(&input.to_string(), "{}", "units")).round().max(0.0) as u64;
    let mut floored = input.clone();
    floored["electric_min_powered"] = serde_json::json!(most_powered);
    let most_draw = page_objective(&aniimax::wasm::exact_electric_problem(&floored.to_string(), "{}", "draw")).round().max(0.0) as u64;
    floored["electric_min_draw"] = serde_json::json!(most_draw);
    let staged = wasm_plan(&floored);
    assert_eq!(staged["success"], true, "{staged}");
    let (coins, staged_coins) = (coins_only["rate_per_second"].as_f64().unwrap(), staged["rate_per_second"].as_f64().unwrap());
    assert!(staged_coins >= coins * (1.0 - 1e-6), "E-mode's priorities lowered revenue: {staged_coins} vs {coins}");
    let powered = |plan: &serde_json::Value| -> u64 { plan["electric_units"].as_array().unwrap().iter().map(|r| r[2].as_u64().unwrap()).sum() };
    assert_eq!(powered(&staged), most_powered);
    assert!(powered(&staged) >= powered(&coins_only), "{} vs {}", powered(&staged), powered(&coins_only));
}

// ---------------------------------------------------------------------------------------------
// 10. Combined strategy only, the Harvest Moon Festival's crops and recipes, Nimbus Bed petals
// ---------------------------------------------------------------------------------------------

/// Two Lv.4 Mines and a Lv.1 Woodworking Bench fed by thirty Lv.5 Woodland plots, RV16 grid, with
/// `strategy` as an older saved setup or share link may still send it (`None`: not sent at all).
fn strategy_input(items: &[ProductionItem], strategy: Option<&str>) -> serde_json::Value {
    let mut input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(items, &[("Mine", 2, 4), ("Woodworking Bench", 1, 1), ("Woodland", 30, 5)]),
        "prioritize_byproducts": false,
        "home_level": 16,
        "electric_enabled": true,
        "generator_level": 3,
        "generator_count": 2,
    });
    if let Some(strategy) = strategy {
        input["electric_strategy"] = serde_json::json!(strategy);
    }
    input
}

// Whatever strategy an input carries, the page's model powers what the combined one does (both
// Mines and the Bench), and the plan reports "combined".
#[test]
fn wasm_old_strategy_values_plan_as_combined() {
    let Some(items) = load_items() else { return };
    let most = |strategy: Option<&str>| {
        page_objective(&aniimax::wasm::exact_electric_problem(&strategy_input(&items, strategy).to_string(), "{}", "units")).round()
    };
    let combined = most(Some("combined"));
    assert_eq!(combined, 3.0);
    for strategy in [Some("raw"), Some("workstations"), Some("processing"), Some(""), None] {
        assert_eq!(most(strategy), combined, "{strategy:?}");
        let plan = wasm_plan(&strategy_input(&items, strategy));
        assert_eq!(plan["success"], true, "{strategy:?}: {plan}");
        assert_eq!(plan["electric_strategy"], "combined", "{strategy:?}");
    }
}

/// The festival's crops and recipes on the page's input: `owned` facilities, the two Recipe Notes
/// recipes left out as the page leaves them out until ticked, plus `exclude`.
fn season_input(items: &[ProductionItem], owned: &[(&str, u32, u32)], season: bool, exclude: &[&str]) -> serde_json::Value {
    let mut excluded = vec!["umbral_pickle", "umbral_hot_pot"];
    excluded.extend_from_slice(exclude);
    serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(items, owned),
        "prioritize_byproducts": false,
        "home_level": 10,
        "season": season,
        "exclude": excluded,
        "aniimo": "best",
    })
}

/// Plots (or units) the plan gives `item` on its producing rows.
fn plan_units(plan: &serde_json::Value, item: &str) -> u64 {
    plan["coin_items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["status"] == "producing" && row["item_name"].as_str().is_some_and(|n| aniimax::models::base_item_name(n) == item))
        .map(|row| row["facility_count"].as_u64().unwrap())
        .sum()
}

// While the festival is on, Moondew Radish and Waxing Moon Pepper each get at least one plot, even
// on Lv.5 Farmland. With it off, neither is planted.
#[test]
fn festival_plants_both_crops_only_while_on() {
    let Some(items) = load_items() else { return };
    let owned = [("Farmland", 4, 5)];
    let on = wasm_plan(&season_input(&items, &owned, true, &[]));
    assert_eq!(on["success"], true, "{on}");
    for crop in ["moondew_radish", "waxing_moon_pepper"] {
        assert!(plan_units(&on, crop) >= 1, "{crop}: {on}");
    }
    let off = wasm_plan(&season_input(&items, &owned, false, &[]));
    assert_eq!(off["success"], true, "{off}");
    for crop in ["moondew_radish", "waxing_moon_pepper"] {
        assert_eq!(plan_units(&off, crop), 0, "{crop}: {off}");
    }
    println!("on {} radish {} pepper, {:.6}/s; off {:.6}/s", plan_units(&on, "moondew_radish"), plan_units(&on, "waxing_moon_pepper"), on["rate_per_second"], off["rate_per_second"]);
}

// One plot can't hold both crops: the page's model and plan say so instead of dropping one.
#[test]
fn festival_with_one_plot_reports_why() {
    let Some(items) = load_items() else { return };
    let input = season_input(&items, &[("Farmland", 1, 5)], true, &[]).to_string();
    let problem: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_problem(&input, "{}")).unwrap();
    assert!(problem["error"].as_str().is_some_and(|e| e.contains("盈月豐收節") && e.contains("1 塊")), "{problem}");
    let plan: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_plan(&input, "{}", r#"{"values":[],"proven":true,"bound":0}"#)).unwrap();
    assert_eq!(plan["success"], false);
    assert!(plan["error"].as_str().is_some_and(|e| e.contains("盈月豐收節")), "{plan}");
}

// Lv.1 Farmland grows no Sugarcane, so there's no Rock Candy for Moondew Radish Slices and no
// Harvest Platter, however much it sells for (5290 against the sauce's 2390): the plan makes Umbral
// Sweet and Spicy Sauce from the festival crops instead.
#[test]
fn festival_without_rock_candy_makes_the_sauce() {
    let Some(items) = load_items() else { return };
    let owned = [
        ("Farmland", 6, 1), ("Tidewhisper Sandcastle", 3, 1), ("Simmering Pot", 1, 3), ("Claw Game Cooker", 1, 1),
        ("Blazing Stove", 1, 1), ("Crafting Table", 1, 1),
    ];
    let plan = wasm_plan(&season_input(&items, &owned, true, &[]));
    assert_eq!(plan["success"], true, "{plan}");
    assert_eq!(plan_units(&plan, "harvest_platter"), 0, "{plan}");
    assert_eq!(plan_units(&plan, "moondew_radish_slices"), 0, "{plan}");
    assert!(plan_units(&plan, "umbral_sweet_and_spicy_sauce") >= 1, "{plan}");
}

// With Rock Candy in reach (Lv.5 Farmland under a Heat Furnace for Sugarcane, a Lv.3 Simmering Pot)
// both recipes can be made. The plan earns at least what either recipe earns alone, and makes the
// one that earns more alone; the comparison is the model's net revenue, not the sale price.
#[test]
fn festival_recipes_compete_on_net_revenue() {
    let Some(items) = load_items() else { return };
    let owned = [
        ("Farmland", 6, 5), ("Tidewhisper Sandcastle", 3, 1), ("Well", 1, 2), ("Simmering Pot", 1, 3),
        ("Claw Game Cooker", 1, 1), ("Blazing Stove", 1, 1), ("Crafting Table", 1, 1), ("Heat Furnace", 1, 1),
    ];
    let rate = |plan: &serde_json::Value| plan["rate_per_second"].as_f64().unwrap();
    let both = wasm_plan(&season_input(&items, &owned, true, &[]));
    let platter = wasm_plan(&season_input(&items, &owned, true, &["umbral_sweet_and_spicy_sauce"]));
    let sauce = wasm_plan(&season_input(&items, &owned, true, &["harvest_platter"]));
    for plan in [&both, &platter, &sauce] {
        assert_eq!(plan["success"], true, "{plan}");
    }
    println!("both {:.6}/s, platter only {:.6}/s, sauce only {:.6}/s", rate(&both), rate(&platter), rate(&sauce));
    assert!(rate(&both) >= rate(&platter).max(rate(&sauce)) * (1.0 - 1e-6));
    let (better, worse) = if rate(&platter) > rate(&sauce) { ("harvest_platter", rate(&sauce)) } else { ("umbral_sweet_and_spicy_sauce", rate(&platter)) };
    if rate(&both) > worse * (1.0 + 1e-6) {
        assert!(plan_units(&both, better) >= 1, "{better}: {both}");
    }
}

/// Petals' rate in a plan for `counts`, and its coins per second.
fn petals_plan(items: &[ProductionItem], counts: &FacilityCounts) -> (f64, f64) {
    let plan = plan_with_power(items, counts, None).expect("plans");
    check(&plan, items, counts, None).expect("passes its re-check");
    (plan.recipe_rates.iter().filter(|(n, _)| aniimax::models::base_item_name(n) == "petals").map(|(_, r)| r).sum(), plan.rate_per_second)
}

// Petals (Nimbus Bed Lv.3, no inputs, 6 per 2700 workload) enter the plan once the bed is Lv.3, and
// a second bed (RV16 has two) makes twice as many and earns twice as much. At Lv.2 there are none.
#[test]
fn nimbus_bed_petals_enter_the_plan() {
    let Some(items) = load_items() else { return };
    let (one_rate, one_coins) = petals_plan(&items, &FacilityCounts::only(&[("Nimbus Bed", 1, 3)]));
    let (two_rate, two_coins) = petals_plan(&items, &FacilityCounts::only(&[("Nimbus Bed", 2, 3)]));
    assert!(one_rate > 0.0);
    assert!((two_rate - 2.0 * one_rate).abs() < 1e-9 && (two_coins - 2.0 * one_coins).abs() < 1e-9, "{one_rate} {two_rate} {one_coins} {two_coins}");
    let (lv2_rate, _) = petals_plan(&items, &FacilityCounts::only(&[("Nimbus Bed", 2, 2)]));
    assert_eq!(lv2_rate, 0.0);
}

// Where the festival rule has to bind: two Lv.1 plots, and a Claw Game Cooker that roasts Waxing
// Moon Pepper (with Sea Salt) while Moondew Radish has no recipe here, so without the rule both
// plots grow pepper. With `require_units` each crop keeps one plot, and the page's plan does too.
#[test]
fn festival_rule_binds_when_one_crop_pays_more() {
    let Some(mut items) = load_items() else { return };
    let mut season = aniimax::data::parse_season(&std::fs::read_to_string("data/harvest_moon_festival.csv").unwrap()).unwrap();
    aniimax::models::apply_watering(&mut season);
    items.extend(season.clone());
    let owned = [("Farmland", 2, 1), ("Tidewhisper Sandcastle", 3, 1), ("Claw Game Cooker", 1, 1)];
    let plots = |plan: &ExactPlan, crop: &str| -> u32 {
        plan.units.iter().filter(|(n, _)| aniimax::models::base_item_name(n) == crop).map(|(_, u)| *u).sum()
    };
    let only_festival = |counts: &FacilityCounts| {
        let mut items = items.clone();
        // Only the festival's crops on the Farmland, as the comparison is between the two of them.
        items.retain(|i| i.facility != "Farmland" || season.iter().any(|s| s.name == i.name));
        let plan = plan_with_power(&items, counts, None).expect("plans");
        check(&plan, &items, counts, None).expect("passes its re-check");
        plan
    };
    let free = FacilityCounts::only(&owned);
    let plan = only_festival(&free);
    assert_eq!((plots(&plan, "moondew_radish"), plots(&plan, "waxing_moon_pepper")), (0, 2), "{:?}", plan.units);
    let mut required = FacilityCounts::only(&owned);
    required.require_units("moondew_radish", 1).require_units("waxing_moon_pepper", 1);
    let plan = only_festival(&required);
    assert_eq!((plots(&plan, "moondew_radish"), plots(&plan, "waxing_moon_pepper")), (1, 1), "{:?}", plan.units);
    // The re-check holds the rule as well: the free plan doesn't pass it with the rule set.
    let free_plan = only_festival(&free);
    let items_with_season: Vec<ProductionItem> =
        items.iter().filter(|i| i.facility != "Farmland" || season.iter().any(|s| s.name == i.name)).cloned().collect();
    assert!(check(&free_plan, &items_with_season, &required, None).is_err_and(|e| e.contains("moondew_radish")));

    let page = wasm_plan(&season_input(&items, &[("Farmland", 2, 1), ("Tidewhisper Sandcastle", 3, 1), ("Claw Game Cooker", 1, 1)], true, &[]));
    assert_eq!(page["success"], true, "{page}");
    assert_eq!((plan_units(&page, "moondew_radish"), plan_units(&page, "waxing_moon_pepper")), (1, 1), "{page}");
}

// ---------------------------------------------------------------------------------------------
// 11. Mine/Well power allocation: manual counts, auto by default, and the plan's goal first
// ---------------------------------------------------------------------------------------------

/// TEST FAKE DATA: four Lv.4 Mines and two Lv.4 Wells on the RV16 grid (Lv.3 x 2, limit
/// 1666.67 W), with `allocation` fields merged in as the page sends them.
fn allocation_input(items: &[ProductionItem], owned: &[(&str, u32, u32)], allocation: serde_json::Value) -> serde_json::Value {
    let mut input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(items, owned),
        "prioritize_byproducts": false,
        "home_level": 16,
        "electric_enabled": true,
        "generator_level": 3,
        "generator_count": 2,
        "aniimo": "best",
    });
    for (key, value) in allocation.as_object().unwrap() {
        input[key] = value.clone();
    }
    input
}

const MINES_AND_WELLS: [(&str, u32, u32); 2] = [("Mine", 4, 4), ("Well", 2, 4)];

/// Powered units per facility in a page plan: `(Mine, Well)`.
fn powered_mines_wells(plan: &serde_json::Value) -> (u64, u64) {
    let of = |facility: &str| -> u64 {
        plan["electric_units"].as_array().unwrap().iter().filter(|r| r[0] == facility).map(|r| r[2].as_u64().unwrap()).sum()
    };
    (of("Mine"), of("Well"))
}

// Auto is the default: no allocation field (an older saved setup), "auto", or anything unclear all
// build the same model, and counts sent alongside auto are ignored.
#[test]
fn allocation_defaults_to_auto() {
    let Some(items) = load_items() else { return };
    let lp = |allocation: serde_json::Value| -> String {
        let problem: serde_json::Value =
            serde_json::from_str(&aniimax::wasm::exact_problem(&allocation_input(&items, &MINES_AND_WELLS, allocation).to_string(), "{}")).unwrap();
        assert!(problem.get("error").is_none(), "{problem}");
        problem["lp"].as_str().unwrap().to_string()
    };
    let default = lp(serde_json::json!({}));
    for allocation in [
        serde_json::json!({ "electric_allocation": "auto" }),
        serde_json::json!({ "electric_allocation": "bogus", "electric_mine_units": 1 }),
        serde_json::json!({ "electric_allocation": "auto", "electric_mine_units": 1, "electric_well_units": 0 }),
    ] {
        assert_eq!(lp(allocation.clone()), default, "{allocation}");
    }
    assert_ne!(lp(serde_json::json!({ "electric_allocation": "manual", "electric_mine_units": 1 })), default);
}

// Manual counts are kept exactly, never fewer or more: two Mines and no Wells (left unticked), or
// one Mine and both Wells, where auto powers all six (powering is worth it for each of them).
#[test]
fn manual_counts_are_kept_exactly() {
    let Some(items) = load_items() else { return };
    let auto = wasm_plan(&allocation_input(&items, &MINES_AND_WELLS, serde_json::json!({})));
    assert_eq!(auto["success"], true, "{auto}");
    assert_eq!(powered_mines_wells(&auto), (4, 2));
    for (mine, well, expected) in [(Some(2), None, (2, 0)), (Some(1), Some(2), (1, 2)), (None, Some(1), (0, 1)), (None, None, (0, 0))] {
        let allocation = serde_json::json!({ "electric_allocation": "manual", "electric_mine_units": mine, "electric_well_units": well });
        let plan = wasm_plan(&allocation_input(&items, &MINES_AND_WELLS, allocation.clone()));
        assert_eq!(plan["success"], true, "{allocation}: {plan}");
        assert_eq!(powered_mines_wells(&plan), expected, "{allocation}");
        assert_eq!(plan["electric_strategy"], "combined");
    }
}

// A manual count that can't be kept is reported with its reason, by the model and by the plan:
// more Mines than owned, or more than the grid's 120% limit holds.
#[test]
fn manual_counts_that_cannot_be_kept_say_why() {
    let Some(items) = load_items() else { return };
    let reason = |owned: &[(&str, u32, u32)], allocation: serde_json::Value| -> String {
        let input = allocation_input(&items, owned, allocation).to_string();
        let problem: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_problem(&input, "{}")).unwrap();
        let units: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_electric_problem(&input, "{}", "units")).unwrap();
        let plan: serde_json::Value =
            serde_json::from_str(&aniimax::wasm::exact_plan(&input, "{}", r#"{"values":[],"proven":true,"bound":0}"#)).unwrap();
        assert_eq!(plan["success"], false);
        assert_eq!(problem["error"], plan["error"]);
        assert_eq!(units["error"], plan["error"]);
        plan["error"].as_str().unwrap().to_string()
    };
    let more_than_owned = reason(&MINES_AND_WELLS, serde_json::json!({ "electric_allocation": "manual", "electric_mine_units": 5 }));
    assert!(more_than_owned.contains("礦山接電 5 座") && more_than_owned.contains("只有 4 座礦山"), "{more_than_owned}");
    // Twelve Lv.5 Mines (150 W each): 1800 W over the 1666.67 W limit.
    let over_limit = reason(&[("Mine", 12, 5)], serde_json::json!({ "electric_allocation": "manual", "electric_mine_units": 12 }));
    assert!(over_limit.contains("1800W") && over_limit.contains("1666.67W"), "{over_limit}");
    // Mines and Wells together: eight Lv.5 Mines (1200 W) and four Lv.5 Wells (600 W).
    let together = reason(&[("Mine", 8, 5), ("Well", 4, 5)], serde_json::json!({ "electric_allocation": "manual", "electric_mine_units": 8, "electric_well_units": 4 }));
    assert!(together.contains("礦山 8 座") && together.contains("水井 4 座") && together.contains("1800W"), "{together}");
}

// The independent re-check holds the manual counts too.
#[test]
fn recheck_requires_manual_counts() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 4, 4)]);
    let power = ElectricConfig::new(3).with_generators(2).with_manual_units(Some(2), Some(0));
    let t = production_time(&items, "rock");
    let two = manual_plan(&[("rock", 4.0 / t, 4)], &[("rock", 2)]);
    check(&two, &items, &counts, Some(&power)).expect("two powered Mines, as asked");
    let three = manual_plan(&[("rock", 4.0 / t, 4)], &[("rock", 3)]);
    let err = check(&three, &items, &counts, Some(&power)).unwrap_err();
    assert!(err.contains("Mine must have 2 powered units but has 3"), "{err}");
}

/// The page's worker for a coin plan now (`exactPlanJson` in web/worker.js): the most coins first,
/// then E-mode's two priorities keeping those coins, then coins again with both kept. Returns the
/// plan, the coins found first, and the two floors.
fn revenue_first_plan(items: &[ProductionItem], counts: &FacilityCounts, power: ElectricConfig) -> (ExactPlan, f64, u32, u32) {
    let modules = ModuleLevels::default();
    let coins_problem = write_lp_with_power(items, "coins", counts, &modules, Goal::Earn { floors: &[] }, Some(&power));
    let (_, coins) = solve_lp(&coins_problem).expect("coins solve");
    let floors = vec![("coins".to_string(), coins)];
    let solve = |goal: Goal, power: &ElectricConfig| -> u32 {
        let problem = write_lp_with_power(items, "coins", counts, &modules, goal, Some(power));
        let (_, objective) = solve_lp(&problem).expect("E-mode stage solves");
        objective.round().max(0.0) as u32
    };
    let most_powered = solve(Goal::MostPoweredEarning(&floors), &power);
    let most_draw = solve(Goal::MostPowerDrawEarning(&floors), &power.with_floors(most_powered, 0));
    let plan = plan_with_power(items, counts, Some(&power.with_floors(most_powered, most_draw))).expect("final plan");
    (plan, coins, most_powered, most_draw)
}

// Highest revenue: the most coins come first, and E-mode is shared out within them. The case where
// E-mode first would cost coins (`tiers_most_powered_units_come_before_revenue`): five Lv.4 Mines
// and three Lv.1 Wells on a Lv.1 generator (500 W). E-mode first powers six units (three Mines and
// three Wells, 450 W) and earns less; revenue first keeps the coin plan's four Mines (480 W).
#[test]
fn revenue_first_never_trades_coins_for_power() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 5, 4), ("Well", 3, 1)]);
    let power = ElectricConfig::new(1);
    let (plan, coins, most_powered, most_draw) = revenue_first_plan(&items, &counts, power);
    let (power_first, _, _) = staged_plan(&items, &counts, power);
    assert!(plan.rate_per_second >= coins * (1.0 - 1e-4), "{} vs {coins}", plan.rate_per_second);
    assert!(power_first.rate_per_second < plan.rate_per_second - 1e-6, "{} vs {}", power_first.rate_per_second, plan.rate_per_second);
    assert_eq!((most_powered, most_draw), (4, 480));
    assert_eq!(powered_units(&plan), 4, "{:?}", plan.electric_units);
    assert_eq!(powered_facilities(&plan, &items), vec!["Mine".to_string()]);
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// Auto mode shares out the power itself, with no counts given: the Mines, a Woodworking Bench fed
// by thirty Lv.5 Woodland plots, on the RV16 grid, revenue first. The plan keeps its coins and
// powers at least as many units as the coin plan alone did, and all of them stay within the limit.
#[test]
fn auto_mode_shares_out_power_after_revenue() {
    let Some(items) = load_items() else { return };
    let counts = FacilityCounts::only(&[("Mine", 2, 4), ("Woodworking Bench", 1, 1), ("Woodland", 30, 5)]);
    let power = ElectricConfig::new(3).with_generators(2);
    let coins_only = plan_with_power(&items, &counts, Some(&power)).expect("coins-only plan");
    let (plan, coins, most_powered, most_draw) = revenue_first_plan(&items, &counts, power);
    assert!(plan.rate_per_second >= coins * (1.0 - 1e-4));
    assert!(powered_units(&plan) >= powered_units(&coins_only));
    assert_eq!(powered_units(&plan), most_powered);
    assert!(powered_facilities(&plan, &items).contains(&"Mine".to_string()), "{:?}", plan.electric_units);
    assert!(demand(&plan, &items, &counts) <= power.boost_threshold());
    check(&plan, &items, &counts, Some(&power.with_floors(most_powered, most_draw))).expect("passes its re-check");
}

// The page's own E-mode models follow the same order: with the coins in `stage_json`'s floors,
// `exact_electric_problem` keeps them, so the most powered units it finds is the revenue-first one
// (four Mines), not E-mode first's six.
#[test]
fn wasm_electric_problem_keeps_the_coin_floor() {
    let Some(items) = load_items() else { return };
    let input = serde_json::json!({
        "currency": "coins",
        "facilities": page_facilities(&items, &[("Mine", 5, 4), ("Well", 3, 1)]),
        "prioritize_byproducts": false,
        "home_level": 12,
        "electric_enabled": true,
        "generator_level": 1,
        "generator_count": 1,
        "aniimo": "best",
    });
    let coins = page_objective(&aniimax::wasm::exact_problem(&input.to_string(), "{}"));
    let unfloored = page_objective(&aniimax::wasm::exact_electric_problem(&input.to_string(), "{}", "units")).round();
    let stage = serde_json::json!({ "floors": [["coins", coins]] }).to_string();
    let floored = page_objective(&aniimax::wasm::exact_electric_problem(&input.to_string(), &stage, "units")).round();
    assert_eq!((unfloored, floored), (6.0, 4.0));
}

// ---------------------------------------------------------------------------------------------
// 12. Manual mode's "最佳" (best): the planner decides that facility's count
// ---------------------------------------------------------------------------------------------

/// A manual allocation as the page sends it: each facility `"best"`, a fixed count, or `"off"`.
fn manual_choice(mine: &str, well: &str) -> serde_json::Value {
    let pick = |choice: &str| -> (serde_json::Value, bool) {
        match choice {
            "best" => (serde_json::Value::Null, true),
            "off" => (serde_json::Value::Null, false),
            n => (serde_json::json!(n.parse::<u32>().unwrap()), false),
        }
    };
    let ((mine_units, mine_best), (well_units, well_best)) = (pick(mine), pick(well));
    serde_json::json!({
        "electric_allocation": "manual",
        "electric_mine_units": mine_units, "electric_mine_best": mine_best,
        "electric_well_units": well_units, "electric_well_best": well_best,
    })
}

/// The page's model text for `input` (see `exact_problem`).
fn page_lp(input: &serde_json::Value) -> String {
    let problem: serde_json::Value = serde_json::from_str(&aniimax::wasm::exact_problem(&input.to_string(), "{}")).unwrap();
    assert!(problem.get("error").is_none(), "{problem}");
    problem["lp"].as_str().unwrap().to_string()
}

/// The page's worker for a coin plan (`exactPlanJson` in web/worker.js) through its own models:
/// the most coins, then E-mode's two priorities keeping them, then the plan with both kept.
/// Returns the plan and the coins found first.
fn wasm_revenue_first(input: &serde_json::Value) -> (serde_json::Value, f64) {
    let coins = page_objective(&aniimax::wasm::exact_problem(&input.to_string(), "{}"));
    let stage = serde_json::json!({ "floors": [["coins", coins]] }).to_string();
    let mut floored = input.clone();
    floored["electric_min_powered"] =
        serde_json::json!(page_objective(&aniimax::wasm::exact_electric_problem(&input.to_string(), &stage, "units")).round().max(0.0) as u64);
    floored["electric_min_draw"] =
        serde_json::json!(page_objective(&aniimax::wasm::exact_electric_problem(&floored.to_string(), &stage, "draw")).round().max(0.0) as u64);
    let plan = wasm_plan(&floored);
    assert_eq!(plan["success"], true, "{plan}");
    (plan, coins)
}

// Both "best" is auto: the same model, with nothing fixed.
#[test]
fn best_for_both_is_the_auto_model() {
    let Some(items) = load_items() else { return };
    let auto = page_lp(&allocation_input(&items, &MINES_AND_WELLS, serde_json::json!({})));
    assert_eq!(page_lp(&allocation_input(&items, &MINES_AND_WELLS, manual_choice("best", "best"))), auto);
    // "Best" isn't a count: one fixed at what the planner would pick builds a different model.
    assert_ne!(page_lp(&allocation_input(&items, &MINES_AND_WELLS, manual_choice("4", "best"))), auto);
}

// Mines "best", Wells off: the Wells get none, the Mines what the planner picks (all four here, as
// auto picks). Wells "best", Mines off: the other way round.
#[test]
fn best_for_one_with_the_other_off() {
    let Some(items) = load_items() else { return };
    let (auto, _) = wasm_revenue_first(&allocation_input(&items, &MINES_AND_WELLS, serde_json::json!({})));
    assert_eq!(powered_mines_wells(&auto), (4, 2));
    let (mines, coins) = wasm_revenue_first(&allocation_input(&items, &MINES_AND_WELLS, manual_choice("best", "off")));
    assert_eq!(powered_mines_wells(&mines), (4, 0));
    assert!(mines["rate_per_second"].as_f64().unwrap() >= coins * (1.0 - 1e-4));
    let (wells, coins) = wasm_revenue_first(&allocation_input(&items, &MINES_AND_WELLS, manual_choice("off", "best")));
    assert_eq!(powered_mines_wells(&wells), (0, 2));
    assert!(wells["rate_per_second"].as_f64().unwrap() >= coins * (1.0 - 1e-4));
}

// One fixed, the other "best". Five Lv.4 Mines (120 W) and three Lv.1 Wells (30 W) on one Lv.1
// generator (500 W): the Wells fixed at three (90 W) leave room for three Mines, not the four the
// Mines' own cap (500 W / 120 W) offers, and "best" takes three; it is no count of its own. The
// other way, the Mines fixed at two, the Wells take what the planner picks (all three fit).
#[test]
fn best_for_one_with_the_other_fixed() {
    let Some(items) = load_items() else { return };
    let owned = [("Mine", 5, 4), ("Well", 3, 1)];
    let input = |allocation: serde_json::Value| {
        let mut input = allocation_input(&items, &owned, allocation);
        input["home_level"] = serde_json::json!(12);
        input["generator_level"] = serde_json::json!(1);
        input["generator_count"] = serde_json::json!(1);
        input
    };
    let (plan, coins) = wasm_revenue_first(&input(manual_choice("best", "3")));
    assert_eq!(powered_mines_wells(&plan), (3, 3));
    assert!(plan["rate_per_second"].as_f64().unwrap() >= coins * (1.0 - 1e-4));
    let (plan, _) = wasm_revenue_first(&input(manual_choice("2", "best")));
    assert_eq!(powered_mines_wells(&plan).0, 2);
    assert_eq!(powered_mines_wells(&plan).1, 3);
}

// Whatever the Mines get, the rest of the grid stays the planner's for the processing facilities:
// two Lv.4 Mines and a Lv.6 Jukebox Dryer kept busy all day (Shredded Coconut, from 240 Lv.5
// Woodland plots: fewer leave it idle part of the day, and a powered unit has to run all day, so
// powering it would cost coins) on the RV16 grid. With the Mines off, "best" or fixed, the Dryer is
// powered too.
#[test]
fn spare_power_goes_to_processing_facilities() {
    let Some(items) = load_items() else { return };
    let owned = [("Mine", 2, 4), ("Jukebox Dryer", 1, 6), ("Woodland", 240, 5)];
    for (mine, mines) in [("off", 0), ("best", 2), ("1", 1)] {
        let (plan, coins) = wasm_revenue_first(&allocation_input(&items, &owned, manual_choice(mine, "off")));
        let dryer: u64 = plan["electric_units"].as_array().unwrap().iter().filter(|r| r[0] == "Jukebox Dryer").map(|r| r[2].as_u64().unwrap()).sum();
        assert_eq!(dryer, 1, "Mines {mine}: {}", plan["electric_units"]);
        assert_eq!(powered_mines_wells(&plan).0, mines, "Mines {mine}");
        assert!(plan["rate_per_second"].as_f64().unwrap() >= coins * (1.0 - 1e-4));
    }
}

// The soonest level-up keeps its pace whatever the allocation: the Mines "best" (no Wells here)
// is the auto pace, and every staged plan keeps the pace it found first; E-mode never slows the
// level-up below what it is without E-mode.
#[test]
fn best_keeps_the_level_up_pace() {
    let Some((items, counts, level_up, power)) = level_up_case() else { return };
    let (_, pace_off) = solve_goal(&items, &counts, Goal::LevelUp(&level_up), &ElectricConfig::disabled());
    let (_, pace_auto) = staged_level_up(&items, &counts, &level_up, power);
    let (plan, pace_best) = staged_level_up(&items, &counts, &level_up, power.with_manual_units(None, Some(0)));
    assert!((pace_best - pace_auto).abs() <= pace_auto * 1e-9, "{pace_best} vs {pace_auto}");
    assert!(plan.pace.unwrap() >= pace_best * (1.0 - 1e-3));
    assert!(pace_best >= pace_off - 1e-9);
    let (plan, pace_off_mines) = staged_level_up(&items, &counts, &level_up, power.with_manual_units(Some(0), Some(0)));
    assert!(plan.pace.unwrap() >= pace_off_mines * (1.0 - 1e-3));
    assert!(powered_facilities(&plan, &items).iter().all(|f| f != "Mine"), "{:?}", plan.electric_units);
}

// ---------------------------------------------------------------------------------------------
// 13. Every unit keeps to one fixed recipe
// ---------------------------------------------------------------------------------------------

/// Units the plan gives each facility, summed over its recipes, and its powered units.
fn units_by_facility(plan: &ExactPlan, items: &[ProductionItem]) -> BTreeMap<String, (u32, u32)> {
    let mut by: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for (name, &units) in &plan.units {
        let facility = items.iter().find(|i| &i.name == name).unwrap().facility.clone();
        by.entry(facility).or_default().0 += units;
    }
    for (name, &powered) in &plan.electric_units {
        assert!(powered <= plan.units.get(name).copied().unwrap_or(0), "{name}: {powered} powered of {:?}", plan.units.get(name));
        let facility = items.iter().find(|i| &i.name == name).unwrap().facility.clone();
        by.entry(facility).or_default().1 += powered;
    }
    by
}

// Three Woodworking Benches and three Chimney Kilns (the level-up case): however the plan runs the
// tiers, for the level-up or for coins, each facility's recipes together get no more units than are
// owned (no recipe's share of time rounded up to a unit of its own), and powered units are whole
// units of their own recipe. Every other facility is held to the same.
#[test]
fn fixed_recipes_never_exceed_owned_units() {
    let Some((items, counts, level_up, power)) = level_up_case() else { return };
    let (leveling, _) = staged_level_up(&items, &counts, &level_up, power);
    let (earning, _, _) = staged_plan(&items, &counts, power);
    for plan in [&leveling, &earning] {
        for (facility, (units, powered)) in units_by_facility(plan, &items) {
            let owned = counts.get_count(&facility);
            assert!(units <= owned, "{facility}: {units} units given out of {owned}: {:?}", plan.units);
            assert!(powered <= units, "{facility}: {powered} powered of {units}");
        }
    }
    // The level-up runs every tier, each on whole units of its own.
    for recipe in ["rough_lumber", "standard_planks", "laminated_beams"] {
        assert!(leveling.units.get(recipe).copied().unwrap_or(0) >= 1, "{recipe}: {:?}", leveling.units);
    }
}
