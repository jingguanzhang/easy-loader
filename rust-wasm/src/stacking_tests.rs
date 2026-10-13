use super::*;
fn stock(name: &str, l: f64, w: f64, h: f64, qty: u32) -> Value {
    json!({"name":name,"l":l,"w":w,"h":h,"weight":5.,"qty":qty,"rot0":true,"rot1":true,"rot2":false,"rot3":false,"rot4":false,"rot5":false,"color":"#79a886"})
}
fn part(id: &str, name: &str, x: f64, y: f64, z: f64) -> Value {
    json!({"id":id,"cargoName":name,"orientation":0,"x":x,"y":y,"z":z})
}
fn unit(id: &str, l: f64, w: f64, copies: u32, parts: Vec<Value>) -> Value {
    json!({"id":id,"name":id,"enabled":true,"copies":copies,"length":l,"width":w,"maxHeight":2000.,"pallet":{"length":l,"width":w,"thickness":50.,"weight":10.,"maxLoad":1000.},"members":parts})
}
fn run(items: Vec<Value>, groups: Vec<Value>, l: f64, w: f64, h: f64) -> Value {
    let c = json!([{"key":"c","label":"柜","length":l,"width":w,"height":h,"maxWeight":10000.}]);
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
fn heights(out: &Value) -> Vec<f64> {
    out["results"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| {
            r["palletBases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b["y"].as_f64().unwrap())
        })
        .collect()
}
#[test]
fn three_complete_pallets_stack_with_tare_and_member_levels() {
    let out = run(
        vec![stock("A", 100., 100., 100., 3)],
        vec![unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)])],
        100.,
        100.,
        450.,
    );
    assert_eq!(out["results"].as_array().unwrap().len(), 1);
    assert_eq!(heights(&out), vec![0., 150., 300.]);
    assert_eq!(out["results"][0]["totalWeight"], 45.);
    assert_eq!(out["results"][0]["placedCount"], 3);
    let levels: Vec<u64> = out["results"][0]["placed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["stackLevel"].as_u64().unwrap())
        .collect();
    assert_eq!(levels, vec![1, 2, 3]);
}
#[test]
fn any_nonstackable_member_prohibits_whole_pallet_support() {
    let mut a = stock("A", 100., 100., 100., 3);
    a["noOverstack"] = json!(true);
    let out = run(
        vec![a],
        vec![unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)])],
        100.,
        100.,
        450.,
    );
    assert_eq!(heights(&out), vec![0., 0., 0.]);
}
#[test]
fn floor_only_members_keep_their_whole_unit_on_floor() {
    let mut a = stock("A", 100., 100., 100., 3);
    a["noUnderstack"] = json!(true);
    let out = run(
        vec![a],
        vec![unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)])],
        100.,
        100.,
        450.,
    );
    assert_eq!(heights(&out), vec![0., 0., 0.]);
}
#[test]
fn load_path_support_allows_deck_overhang_over_gaps() {
    // Every upper member base is fully covered at one elevation; the deck
    // overhangs the empty half of the lower pallet, which is legal.
    let out = run(
        vec![stock("A", 100., 100., 100., 2)],
        vec![unit("g", 200., 100., 2, vec![part("a", "A", 0., 0., 0.)])],
        200.,
        100.,
        400.,
    );
    assert_eq!(out["results"].as_array().unwrap().len(), 1);
    assert_eq!(heights(&out), vec![0., 150.]);
}
#[test]
fn offcenter_member_without_real_base_cover_cannot_stack() {
    // The upper member sits over the empty half: no top faces below it, so the
    // load path is broken even though the bounding boxes line up.
    let out = run(
        vec![stock("A", 100., 100., 100., 1), {
            let mut b = stock("B", 100., 100., 100., 1);
            b["priority"] = json!(1);
            b
        }],
        vec![
            unit("lower", 200., 100., 1, vec![part("a", "A", 0., 0., 0.)]),
            unit("upper", 200., 100., 1, vec![part("b", "B", 100., 0., 0.)]),
        ],
        200.,
        100.,
        400.,
    );
    assert!(
        out["results"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["palletUnits"].as_array().unwrap())
            .all(|u| u["y"].as_f64().unwrap() == 0.)
    );
}
#[test]
fn realistic_margin_pallets_stack_when_load_path_is_complete() {
    // 300x250 cartons in a 3x2 grid leave 100/100 mm margins on a 1000x600
    // pallet; every member base is still covered so stacking is legal.
    let members: Vec<Value> = (0..3)
        .flat_map(|ix| {
            (0..2).map(move |iz| {
                part(
                    &format!("m{ix}{iz}"),
                    "A",
                    50. + ix as f64 * 300.,
                    0.,
                    50. + iz as f64 * 250.,
                )
            })
        })
        .collect();
    let out = run(
        vec![stock("A", 300., 250., 100., 12)],
        vec![unit("g", 1000., 600., 2, members)],
        1000.,
        600.,
        400.,
    );
    assert_eq!(out["results"].as_array().unwrap().len(), 1);
    assert_eq!(heights(&out), vec![0., 150.]);
}
#[test]
fn a_smaller_upper_board_can_use_a_real_flat_top_patch() {
    let mut a = stock("A", 100., 100., 200., 1);
    a["priority"] = json!(2);
    let mut b = stock("B", 100., 100., 100., 1);
    b["priority"] = json!(2);
    let c = stock("C", 100., 100., 100., 1);
    let lower = unit(
        "lower",
        200.,
        100.,
        1,
        vec![part("a", "A", 0., 0., 0.), part("b", "B", 100., 0., 0.)],
    );
    let upper = unit("upper", 100., 100., 1, vec![part("c", "C", 0., 0., 0.)]);
    let out = run(vec![a, b, c], vec![lower, upper], 200., 100., 450.);
    assert_eq!(out["results"].as_array().unwrap().len(), 1);
    assert_eq!(heights(&out), vec![0., 250.]);
}
#[test]
fn different_height_faces_cannot_jointly_support_a_large_upper_board() {
    let mut a = stock("A", 100., 100., 200., 1);
    a["priority"] = json!(2);
    let mut b = stock("B", 100., 100., 100., 1);
    b["priority"] = json!(2);
    let c = stock("C", 200., 100., 100., 1);
    let lower = unit(
        "lower",
        200.,
        100.,
        1,
        vec![part("a", "A", 0., 0., 0.), part("b", "B", 100., 0., 0.)],
    );
    let upper = unit("upper", 200., 100., 1, vec![part("c", "C", 0., 0., 0.)]);
    let out = run(vec![a, b, c], vec![lower, upper], 200., 100., 450.);
    assert_eq!(heights(&out), vec![0., 0.]);
}
#[test]
fn upper_board_selfweight_counts_against_lower_board_capacity() {
    let mut g = unit("g", 100., 100., 2, vec![part("a", "A", 0., 0., 0.)]);
    g["pallet"]["maxLoad"] = json!(19.);
    let out = run(
        vec![stock("A", 100., 100., 100., 2)],
        vec![g],
        100.,
        100.,
        300.,
    );
    assert_eq!(heights(&out), vec![0., 0.]);
}
#[test]
fn capacity_exactly_twenty_allows_two_but_not_three() {
    let mut g = unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["pallet"]["maxLoad"] = json!(20.);
    let out = run(
        vec![stock("A", 100., 100., 100., 3)],
        vec![g],
        100.,
        100.,
        450.,
    );
    assert_eq!(heights(&out), vec![0., 150., 0.]);
}
#[test]
fn cumulative_load_reaches_every_board_in_a_three_pallet_chain() {
    let mut g = unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["pallet"]["maxLoad"] = json!(35.);
    let out = run(
        vec![stock("A", 100., 100., 100., 3)],
        vec![g],
        100.,
        100.,
        450.,
    );
    assert_eq!(heights(&out), vec![0., 150., 300.]);
}
#[test]
fn original_cargo_max_stack_one_blocks_external_pallet_stacking() {
    let mut a = stock("A", 100., 100., 100., 3);
    a["maxStack"] = json!(1);
    let out = run(
        vec![a],
        vec![unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)])],
        100.,
        100.,
        450.,
    );
    assert_eq!(heights(&out), vec![0., 0., 0.]);
}
#[test]
fn original_cargo_limit_two_survives_mixed_skus_in_the_support_chain() {
    let mut a = stock("A", 100., 100., 100., 1);
    a["maxStack"] = json!(2);
    a["priority"] = json!(3);
    let mut b = stock("B", 100., 100., 100., 1);
    b["priority"] = json!(2);
    let c = stock("C", 100., 100., 100., 1);
    let groups = vec![
        unit("a", 100., 100., 1, vec![part("a", "A", 0., 0., 0.)]),
        unit("b", 100., 100., 1, vec![part("b", "B", 0., 0., 0.)]),
        unit("c", 100., 100., 1, vec![part("c", "C", 0., 0., 0.)]),
    ];
    let out = run(vec![a, b, c], groups, 100., 100., 450.);
    assert_eq!(heights(&out), vec![0., 150., 0.]);
}
#[test]
fn multi_layer_pallet_counts_all_internal_layers_against_max_stack() {
    let mut a = stock("A", 100., 100., 100., 4);
    a["maxStack"] = json!(3);
    let g = unit(
        "g",
        100.,
        100.,
        2,
        vec![part("a1", "A", 0., 0., 0.), part("a2", "A", 0., 100., 0.)],
    );
    let out = run(vec![a], vec![g], 100., 100., 500.);
    assert_eq!(heights(&out), vec![0., 0.]);
}
#[test]
fn yaw_rotated_support_faces_match_the_real_member_footprints() {
    let g = unit("g", 200., 100., 2, vec![part("a", "A", 0., 0., 0.)]);
    let out = run(
        vec![stock("A", 200., 100., 100., 2)],
        vec![g],
        100.,
        200.,
        300.,
    );
    assert_eq!(heights(&out), vec![0., 150.]);
    assert!(
        out["results"][0]["palletUnits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["yaw90"] == true)
    );
}
#[test]
fn loose_cargo_is_not_put_on_a_rigid_unit_or_used_as_pallet_support() {
    let mut a = stock("A", 100., 100., 100., 1);
    a["priority"] = json!(2);
    let b = stock("B", 100., 100., 100., 1);
    let out = run(
        vec![a, b],
        vec![unit("g", 100., 100., 1, vec![part("a", "A", 0., 0., 0.)])],
        100.,
        100.,
        300.,
    );
    assert_eq!(out["results"].as_array().unwrap().len(), 2);
    assert!(
        out["results"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["placed"].as_array().unwrap())
            .all(|p| p["y"].as_f64().unwrap() <= 50.)
    );
}
#[test]
fn bridge_above_nonstackable_goods_is_rejected_without_direct_contact() {
    let mut a = stock("A", 100., 100., 100., 1);
    a["noOverstack"] = json!(true);
    let b = stock("B", 100., 100., 200., 2);
    let c = stock("C", 300., 100., 100., 1);
    let g = unit(
        "g",
        300.,
        100.,
        1,
        vec![
            part("a", "A", 100., 0., 0.),
            part("b1", "B", 0., 0., 0.),
            part("b2", "B", 200., 0., 0.),
            part("c", "C", 0., 200., 0.),
        ],
    );
    let out: Value = serde_json::from_str(&validate_pallet_v4(
        &json!([a, b, c]).to_string(),
        &g.to_string(),
    ))
    .unwrap();
    assert_eq!(out["valid"], false);
    assert!(out["errors"][0].as_str().unwrap().contains("不可堆叠"));
}
#[test]
fn no_board_rigid_units_can_stack_on_real_complete_cargo_tops() {
    let mut g = unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["pallet"] = Value::Null;
    let out = run(
        vec![stock("A", 100., 100., 100., 3)],
        vec![g],
        100.,
        100.,
        300.,
    );
    let ys: Vec<f64> = out["results"][0]["palletUnits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["y"].as_f64().unwrap())
        .collect();
    assert_eq!(ys, vec![0., 100., 200.]);
}

#[test]
fn custom_copy_expands_into_distinct_units_with_copy_index() {
    // copies=3: template 1×A; copy2 carries an extra C piece in the gap.
    let mut g = unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["customLayouts"] = json!({"2": [part("a", "A", 0., 0., 0.), part("c", "C", 0., 100., 0.)]});
    let out = run(
        vec![stock("A", 100., 100., 100., 3), stock("C", 50., 50., 50., 1)],
        vec![g],
        100.,
        100.,
        300.,
    );
    let mut recs: Vec<&Value> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["palletUnits"].as_array().unwrap())
        .collect();
    recs.sort_by_key(|u| u["copyIndex"].as_u64().unwrap());
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0]["copyIndex"], 0.);
    assert_eq!(recs[1]["copyIndex"], 0.);
    assert_eq!(recs[2]["copyIndex"], 2.);
    assert_eq!(recs[2]["contents"]["C"], 1.);
    assert_eq!(recs[2]["contents"]["A"], 1.);
    let placed: Vec<&Value> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["placed"].as_array().unwrap())
        .collect();
    assert_eq!(placed.len(), 4);
    assert!(out["globalUnplacedAgg"].as_object().unwrap().is_empty(), "{out}");
}
#[test]
fn stock_is_accounted_across_template_and_custom_copies() {
    // Template needs 1×A per pallet; customized copy2 needs 2×A. With only 3
    // in stock (3 copies = 3+1 = 4 needed) the whole packing must fail.
    let mut g = unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["customLayouts"] = json!({"2": [part("a1", "A", 0., 0., 0.), part("a2", "A", 0., 100., 0.)]});
    let c = json!([{"key":"c","label":"柜","length":100.,"width":100.,"height":300.,"maxWeight":10000.}]);
    let out: Value = serde_json::from_str(&run_pack_engine_v4(
        &c.to_string(),
        &json!([stock("A", 100., 100., 100., 3)]).to_string(),
        &json!([g]).to_string(),
        "large",
        "wall",
        "large",
    ))
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("重复占用"), "{out}");
}
#[test]
fn custom_copy_index_must_stay_within_copy_range() {
    let mut g = unit("g", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["customLayouts"] = json!({"1": [part("a", "A", 0., 0., 0.)]});
    let c = json!([{"key":"c","label":"柜","length":100.,"width":100.,"height":300.,"maxWeight":10000.}]);
    let out: Value = serde_json::from_str(&run_pack_engine_v4(
        &c.to_string(),
        &json!([stock("A", 100., 100., 100., 3)]).to_string(),
        &json!([g]).to_string(),
        "large",
        "wall",
        "large",
    ))
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("第 1 托"), "{out}");
    let mut g = unit("g2", 100., 100., 3, vec![part("a", "A", 0., 0., 0.)]);
    g["customLayouts"] = json!({"9": [part("a", "A", 0., 0., 0.)]});
    let out: Value = serde_json::from_str(&run_pack_engine_v4(
        &c.to_string(),
        &json!([stock("A", 100., 100., 100., 3)]).to_string(),
        &json!([g]).to_string(),
        "large",
        "wall",
        "large",
    ))
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("超出"), "{out}");
}
