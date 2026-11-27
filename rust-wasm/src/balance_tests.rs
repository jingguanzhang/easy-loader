//! Centre-of-gravity limits, support (axle) loads and the order search.
use super::*;
use serde_json::{Value, json};

fn cargo(name: &str, l: f64, w: f64, h: f64, weight: f64, qty: u32) -> Value {
    json!({"name":name,"l":l,"w":w,"h":h,"weight":weight,"qty":qty,"rot0":true,"rot1":false,
        "rot2":false,"rot3":false,"rot4":false,"rot5":false,"color":"#79a886"})
}
fn bin(l: f64, w: f64, h: f64, extra: Value) -> Value {
    let mut c = json!({"key":"c","label":"柜","length":l,"width":w,"height":h,"maxWeight":100000.});
    for (k, v) in extra.as_object().unwrap() {
        c[k] = v.clone();
    }
    json!([c])
}
fn solve_json(c: &Value, items: Vec<Value>, groups: Vec<Value>) -> Value {
    let out: Value = serde_json::from_str(&run_pack_engine_v4(
        &c.to_string(),
        &json!(items).to_string(),
        &json!(groups).to_string(),
        "large",
        "wall",
        "large",
    ))
    .unwrap();
    assert!(out.get("error").is_none(), "{out}");
    out
}
/// Centre of gravity recomputed from the returned pieces and pallet decks.
fn recomputed(result: &Value) -> (f64, f64, f64) {
    let (mut w, mut mx, mut mz) = (0.0, 0.0, 0.0);
    for key in ["placed", "palletBases"] {
        for p in result[key].as_array().into_iter().flatten() {
            let f = |k: &str| p[k].as_f64().unwrap();
            w += f("weight");
            mx += f("weight") * (f("x") + f("dx") / 2.0);
            mz += f("weight") * (f("z") + f("dz") / 2.0);
        }
    }
    (w, mx / w, mz / w)
}
fn inside(result: &Value, l: f64, w: f64) {
    for p in result["placed"].as_array().unwrap() {
        let f = |k: &str| p[k].as_f64().unwrap();
        assert!(f("x") >= -1e-6 && f("x") + f("dx") <= l + 1e-6, "{p}");
        assert!(f("z") >= -1e-6 && f("z") + f("dz") <= w + 1e-6, "{p}");
    }
}

#[test]
fn lever_rule_matches_a_hand_calculation() {
    // One 1000 kg box, 1000 long, against the front wall of a 4000 container:
    // centre of gravity at x = 500. Supports at 0 and 4000 carry 875 / 125 kg.
    let c = bin(
        4000.,
        1000.,
        1000.,
        json!({"frontAxlePos":0.,"rearAxlePos":4000.}),
    );
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 1000., 1)], vec![]);
    let r = &out["results"][0];
    assert_eq!(r["cogX"], 500.);
    assert_eq!(r["cogY"], 500.);
    assert_eq!(r["cogZ"], 500.);
    assert_eq!(r["frontAxleLoad"], 875.);
    assert_eq!(r["rearAxleLoad"], 125.);
    assert_eq!(r["loadShiftX"], 0.);
}

#[test]
fn without_limits_positions_and_support_fields_are_untouched() {
    let c = bin(4000., 1000., 1000., json!({}));
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 1000., 2)], vec![]);
    let r = &out["results"][0];
    assert_eq!(r["placed"][0]["x"], 0.);
    assert_eq!(r["placed"][1]["x"], 1000.);
    assert_eq!(r["loadShiftX"], 0.);
    assert_eq!(r["loadShiftZ"], 0.);
    assert!(r["frontAxleLoad"].is_null() && r["rearAxleLoad"].is_null());
    assert_eq!(r["cogX"], 1000.);
}

#[test]
fn centre_of_gravity_limit_shifts_the_whole_load_by_the_minimum() {
    // Two boxes fill x 0..2000 (cog 1000) in a 4000 container. A ±300 limit
    // needs cog >= 1700, so the load moves exactly 700 towards the door.
    let c = bin(4000., 1000., 1000., json!({"maxCogOffsetX":300.}));
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 1000., 2)], vec![]);
    let r = &out["results"][0];
    assert_eq!(r["loadShiftX"], 700.);
    assert_eq!(r["placed"][0]["x"], 700.);
    assert_eq!(r["placed"][1]["x"], 1700.);
    assert_eq!(r["cogX"], 1700.);
    assert_eq!(r["usedLength"], 2700.);
    assert_eq!(r["tailSpace"], 1300.);
    inside(r, 4000., 1000.);
}

#[test]
fn lateral_limit_centres_a_one_sided_load() {
    let c = bin(1000., 3000., 1000., json!({"maxCogOffsetZ":100.}));
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 500., 1)], vec![]);
    let r = &out["results"][0];
    assert_eq!(r["loadShiftZ"], 900.);
    assert_eq!(r["cogZ"], 1400.);
    inside(r, 1000., 3000.);
}

#[test]
fn support_limit_moves_the_load_until_both_loads_are_legal() {
    // 2000 kg at cog 1000 puts 1500 kg on the front support. A 1100 kg front
    // limit requires cog >= 4000 - 1100 * 4000 / 2000 = 1800.
    let c = bin(
        4000.,
        1000.,
        1000.,
        json!({"frontAxlePos":0.,"rearAxlePos":4000.,"maxFrontAxleLoad":1100.,"maxRearAxleLoad":1100.}),
    );
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 1000., 2)], vec![]);
    let r = &out["results"][0];
    assert_eq!(r["cogX"], 1800.);
    assert!((r["frontAxleLoad"].as_f64().unwrap() - 1100.).abs() < 1e-6);
    assert!((r["rearAxleLoad"].as_f64().unwrap() - 900.).abs() < 1e-6);
}

#[test]
fn cargo_that_no_shift_can_balance_goes_to_the_next_container() {
    // Four 1000 kg boxes exceed the 3000 kg the two supports may carry in
    // total, so one container can take three at most; nothing is lost.
    let c = bin(
        4000.,
        1000.,
        1000.,
        json!({"frontAxlePos":0.,"rearAxlePos":4000.,"maxFrontAxleLoad":1500.,"maxRearAxleLoad":1500.}),
    );
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 1000., 4)], vec![]);
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["placed"].as_array().unwrap().len(), 3);
    assert_eq!(results[1]["placed"].as_array().unwrap().len(), 1);
    assert!(out["globalUnplacedAgg"].as_object().unwrap().is_empty());
    for r in results {
        assert!(r["frontAxleLoad"].as_f64().unwrap() <= 1500. + 1e-6);
        assert!(r["rearAxleLoad"].as_f64().unwrap() <= 1500. + 1e-6);
        inside(r, 4000., 1000.);
    }
}

#[test]
fn a_single_unit_heavier_than_both_supports_is_reported_unplaced() {
    let c = bin(
        4000.,
        1000.,
        1000.,
        json!({"frontAxlePos":0.,"rearAxlePos":4000.,"maxFrontAxleLoad":400.,"maxRearAxleLoad":400.}),
    );
    let out = solve_json(&c, vec![cargo("A", 1000., 1000., 1000., 1000., 1)], vec![]);
    assert!(out["results"].as_array().unwrap().is_empty());
    assert_eq!(out["globalUnplacedAgg"]["A"], 1);
}

#[test]
fn full_container_without_slack_sheds_cargo_instead_of_violating_the_limit() {
    // Heavy boxes load first and end up at the front; the container is full,
    // so nothing can shift until some of the last (light) boxes leave.
    let c = bin(4000., 1000., 1000., json!({"maxCogOffsetX":200.}));
    let out = solve_json(
        &c,
        vec![
            cargo("Heavy", 1000., 1000., 1000., 900., 2),
            cargo("Light", 500., 1000., 1000., 50., 4),
        ],
        vec![],
    );
    for r in out["results"].as_array().unwrap() {
        let (_, cx, _) = recomputed(r);
        assert!((cx - 2000.).abs() <= 200. + 1e-6, "cog {cx}");
        assert!((cx - r["cogX"].as_f64().unwrap()).abs() < 1e-6);
        inside(r, 4000., 1000.);
    }
    let placed: usize = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["placed"].as_array().unwrap().len())
        .sum();
    assert_eq!(placed, 6);
}

#[test]
fn mixed_pallet_centre_of_gravity_uses_real_member_positions_and_yaw() {
    // A 1000 x 500 deck (10 kg) with one 100 kg member on its left end. The
    // reported centre of gravity must equal the one recomputed from the
    // expanded members and the deck, also when the unit is turned 90 degrees.
    let item = json!({"name":"A","l":200.,"w":200.,"h":200.,"weight":100.,"qty":1,"rot0":true,
        "rot1":true,"rot2":false,"rot3":false,"rot4":false,"rot5":false,"color":"#79a886"});
    let group = json!({"id":"g","name":"g","enabled":true,"copies":1,"length":1000.,"width":500.,
        "maxHeight":2000.,"pallet":{"length":1000.,"width":500.,"thickness":50.,"weight":10.,"maxLoad":1000.},
        "members":[{"id":"a","cargoName":"A","orientation":0,"x":0.,"y":0.,"z":0.}]});
    for (l, w) in [(3000., 600.), (600., 3000.)] {
        let c = bin(l, w, 1000., json!({"frontAxlePos":0.,"rearAxlePos":l}));
        let out = solve_json(&c, vec![item.clone()], vec![group.clone()]);
        let r = &out["results"][0];
        let (weight, cx, cz) = recomputed(r);
        assert_eq!(weight, 110.);
        assert!((cx - r["cogX"].as_f64().unwrap()).abs() < 1e-6, "{r}");
        assert!((cz - r["cogZ"].as_f64().unwrap()).abs() < 1e-6, "{r}");
        // Not the geometric centre of the unit box.
        assert!((cx - r["palletBases"][0]["dx"].as_f64().unwrap() / 2.0).abs() > 50.0 || l < w);
        assert_eq!(r["placed"][0]["unitYaw90"], json!(l < w));
    }
}

#[test]
fn invalid_balance_numbers_are_rejected() {
    let c = bin(4000., 1000., 1000., json!({"maxCogOffsetX":-1.}));
    let out: Value = serde_json::from_str(&run_pack_engine_v4(
        &c.to_string(),
        &json!([cargo("A", 1000., 1000., 1000., 1., 1)]).to_string(),
        "[]",
        "large",
        "wall",
        "large",
    ))
    .unwrap();
    assert!(out.get("error").is_some());
}

fn score(packed: f64, containers: usize, volume: f64, last: f64) -> PlanScore {
    PlanScore {
        packed_volume: packed,
        unplaced_pieces: 0,
        total_container_volume: volume,
        container_count: containers,
        last_container_volume: last,
    }
}

#[test]
fn equal_cargo_never_prefers_more_containers() {
    // Same cargo loaded: one large container beats large + small.
    assert!(plan_is_better(
        score(10., 1, 76., 10.),
        score(10., 2, 109., 2.),
        "large"
    ));
    assert!(!plan_is_better(
        score(10., 2, 109., 2.),
        score(10., 1, 76., 10.),
        "large"
    ));
    // "small" still minimises total container volume first.
    assert!(plan_is_better(
        score(10., 2, 66., 2.),
        score(10., 1, 76., 10.),
        "small"
    ));
    // Same containers: the plan that leaves less in the last one wins.
    assert!(plan_is_better(
        score(10., 2, 66., 1.),
        score(10., 2, 66., 3.),
        "large"
    ));
}

#[test]
fn order_search_fits_cargo_that_the_fixed_order_splits() {
    // Width 1000. "Wide" (600) is the largest SKU and is tried first at every
    // frontier position; "Slim" (500) pairs across the width only when it
    // leads. Volume-first order strands the Slim boxes behind Wide ones.
    let items = vec![
        cargo("Wide", 1000., 600., 1000., 1., 2),
        cargo("Slim", 1000., 500., 1000., 1., 4),
        cargo("Fill", 1000., 400., 1000., 1., 2),
    ];
    let c = bin(4000., 1000., 1000., json!({}));
    let out = solve_json(&c, items.clone(), vec![]);
    assert_eq!(out["results"].as_array().unwrap().len(), 1, "{out}");
    assert!(out["globalUnplacedAgg"].as_object().unwrap().is_empty());
    // Deterministic: the same input always gives the same plan.
    assert_eq!(out, solve_json(&c, items, vec![]));
}
