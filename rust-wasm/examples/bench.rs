//! Reproducible solver benchmark and independent result checker.
//!
//!   cargo run --release --example bench            # full table
//!   cargo run --release --example bench -- quick   # fewer instances
//!
//! Instances follow the Bischoff–Ratcliff recipe (20 ft container, 3–20 box
//! types, total cargo volume ≈ target × container volume) with a fixed LCG so
//! every run sees the same cargo. Each returned plan is re-verified here
//! without using any solver internals: bounds, overlap, full support,
//! permitted orientation, weight limit and quantity conservation.

use packer_wasm::run_pack_engine_v3;
use serde_json::{Value, json};
use std::time::Instant;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

const L: f64 = 5870.0;
const W: f64 = 2330.0;
const H: f64 = 2200.0;

fn instance(types: usize, seed: u64, fill: f64, max_weight: f64) -> (Value, Value) {
    let mut rng = Lcg(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ types as u64);
    let mut boxes = Vec::new();
    for _ in 0..types {
        let l = rng.range(30, 120) as f64 * 10.0;
        let w = rng.range(25, 100) as f64 * 10.0;
        let h = rng.range(20, 80) as f64 * 10.0;
        let ok = |d: f64, a: f64, b: f64| d / a.min(b) < 2.0;
        // density 150..600 kg/m3
        let density = rng.range(150, 600) as f64;
        boxes.push((
            l,
            w,
            h,
            ok(h, l, w),
            ok(l, w, h),
            ok(w, l, h),
            density,
            0u32,
        ));
    }
    let target = L * W * H * fill;
    let mut volume = 0.0;
    loop {
        let k = rng.range(0, types as u64 - 1) as usize;
        let b = &mut boxes[k];
        let v = b.0 * b.1 * b.2;
        if volume + v > target {
            break;
        }
        volume += v;
        b.7 += 1;
    }
    let items: Vec<Value> = boxes
        .iter()
        .enumerate()
        .filter(|(_, b)| b.7 > 0)
        .map(|(i, b)| {
            let upright = b.3 || !(b.4 || b.5);
            json!({"name": format!("T{i}"), "color": "#888888", "l": b.0, "w": b.1, "h": b.2,
                "weight": (b.0 * b.1 * b.2 / 1e9 * b.6 * 10.0).round() / 10.0, "qty": b.7,
                "rot0": upright, "rot1": upright, "rot2": b.4, "rot3": b.4, "rot4": b.5, "rot5": b.5})
        })
        .collect();
    let container = json!([{"key": "c", "label": "20GP", "length": L, "width": W, "height": H,
        "maxWeight": max_weight}]);
    (container, json!(items))
}

/// Independent verification. Returns (packed volume, pieces) or an error.
fn verify(templates: &Value, items: &Value, out: &Value) -> Result<(f64, usize), String> {
    let t = &templates[0];
    let (cl, cw, ch) = (
        t["length"].as_f64().unwrap(),
        t["width"].as_f64().unwrap(),
        t["height"].as_f64().unwrap(),
    );
    let e = 1e-3;
    let mut counts = std::collections::HashMap::<String, u64>::new();
    let mut total_volume = 0.0;
    let mut pieces = 0;
    for r in out["results"].as_array().ok_or("no results")? {
        let p: Vec<[f64; 7]> = r["placed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| ["x", "y", "z", "dx", "dy", "dz", "weight"].map(|k| p[k].as_f64().unwrap()))
            .collect();
        let mut weight = 0.0;
        for (i, a) in p.iter().enumerate() {
            let name = r["placed"][i]["cargoName"].as_str().unwrap().to_string();
            *counts.entry(name.clone()).or_default() += 1;
            weight += a[6];
            total_volume += a[3] * a[4] * a[5];
            pieces += 1;
            if a[0] < -e
                || a[1] < -e
                || a[2] < -e
                || a[0] + a[3] > cl + e
                || a[1] + a[4] > ch + e
                || a[2] + a[5] > cw + e
            {
                return Err(format!("out of bounds: {a:?}"));
            }
            let it = items
                .as_array()
                .unwrap()
                .iter()
                .find(|it| it["name"] == name.as_str())
                .ok_or("unknown item")?;
            let (l, w, h) = (
                it["l"].as_f64().unwrap(),
                it["w"].as_f64().unwrap(),
                it["h"].as_f64().unwrap(),
            );
            let allowed = [
                ("rot0", [l, h, w]),
                ("rot1", [w, h, l]),
                ("rot2", [h, l, w]),
                ("rot3", [w, l, h]),
                ("rot4", [l, w, h]),
                ("rot5", [h, w, l]),
            ]
            .iter()
            .any(|(k, d)| {
                it[*k].as_bool().unwrap_or(false)
                    && (d[0] - a[3]).abs() < e
                    && (d[1] - a[4]).abs() < e
                    && (d[2] - a[5]).abs() < e
            });
            if !allowed {
                return Err(format!("orientation not permitted: {name} {a:?}"));
            }
            let mut support = 0.0;
            for (j, b) in p.iter().enumerate() {
                if i == j {
                    continue;
                }
                let ox = (a[0] + a[3]).min(b[0] + b[3]) - a[0].max(b[0]);
                let oy = (a[1] + a[4]).min(b[1] + b[4]) - a[1].max(b[1]);
                let oz = (a[2] + a[5]).min(b[2] + b[5]) - a[2].max(b[2]);
                if ox > e && oy > e && oz > e {
                    return Err(format!("overlap: {a:?} / {b:?}"));
                }
                if (b[1] + b[4] - a[1]).abs() < e && ox > 0.0 && oz > 0.0 {
                    support += ox * oz;
                }
            }
            if a[1] > e && support < a[3] * a[5] * (1.0 - 1e-6) {
                return Err(format!("unsupported base: {a:?} support {support}"));
            }
        }
        if weight > t["maxWeight"].as_f64().unwrap() + 0.01 {
            return Err(format!("overweight {weight}"));
        }
        verify_balance(t, r, &p)?;
    }
    for it in items.as_array().unwrap() {
        let name = it["name"].as_str().unwrap();
        let placed = counts.get(name).copied().unwrap_or(0);
        let left = out["globalUnplacedAgg"][name].as_u64().unwrap_or(0);
        if placed + left != it["qty"].as_u64().unwrap() {
            return Err(format!("quantity not conserved for {name}"));
        }
    }
    Ok((total_volume, pieces))
}

/// Recomputes the centre of gravity and the two support reactions from the
/// placed boxes and checks them against the limits in the template.
fn verify_balance(t: &Value, r: &Value, p: &[[f64; 7]]) -> Result<(), String> {
    let f = |k: &str| t[k].as_f64().unwrap_or(0.0);
    let weight: f64 = p.iter().map(|a| a[6]).sum();
    if weight <= 0.0 {
        return Ok(());
    }
    let cx = p.iter().map(|a| a[6] * (a[0] + a[3] / 2.0)).sum::<f64>() / weight;
    let cz = p.iter().map(|a| a[6] * (a[2] + a[5] / 2.0)).sum::<f64>() / weight;
    let (cl, cw) = (f("length"), f("width"));
    let e = 1e-3;
    if f("maxCogOffsetX") > 0.0 && (cx - cl / 2.0).abs() > f("maxCogOffsetX") + e {
        return Err(format!("longitudinal COG offset {}", cx - cl / 2.0));
    }
    if f("maxCogOffsetZ") > 0.0 && (cz - cw / 2.0).abs() > f("maxCogOffsetZ") + e {
        return Err(format!("lateral COG offset {}", cz - cw / 2.0));
    }
    let (a, b) = (f("frontAxlePos"), f("rearAxlePos"));
    if b - a > e {
        let rear = weight * (cx - a) / (b - a);
        let front = weight - rear;
        if f("maxFrontAxleLoad") > 0.0 && front > f("maxFrontAxleLoad") + 0.01 {
            return Err(format!("front axle load {front}"));
        }
        if f("maxRearAxleLoad") > 0.0 && rear > f("maxRearAxleLoad") + 0.01 {
            return Err(format!("rear axle load {rear}"));
        }
        for (k, v) in [
            ("cogX", cx),
            ("cogZ", cz),
            ("frontAxleLoad", front),
            ("rearAxleLoad", rear),
        ] {
            if let Some(reported) = r[k].as_f64() {
                if (reported - v).abs() > 0.01 {
                    return Err(format!("reported {k}={reported} but recomputed {v}"));
                }
            }
        }
    }
    Ok(())
}

fn run(templates: &Value, items: &Value, strategy: &str) -> (Value, f64) {
    let start = Instant::now();
    let out = run_pack_engine_v3(
        &templates.to_string(),
        &items.to_string(),
        "large",
        strategy,
        "large",
    );
    let secs = start.elapsed().as_secs_f64();
    (serde_json::from_str(&out).unwrap(), secs)
}

fn main() {
    let quick = std::env::args().any(|a| a == "quick");
    let n = if quick { 5 } else { 20 };
    let mut failures = 0;
    let sections: Vec<String> = std::env::args().skip(1).filter(|a| a != "quick").collect();
    let want = |name: &str| sections.is_empty() || sections.iter().any(|s| s == name);
    if want("single") {
        println!("== Single container fill (cargo volume = 100% of container) ==");
        println!("types strategy  first-container fill %   time/instance s");
        for strategy in ["wall", "floor"] {
            let mut all = 0.0;
            let mut count = 0.0;
            for types in [3usize, 5, 8, 10, 12, 15, 20] {
                let (mut sum, mut time) = (0.0, 0.0);
                for seed in 0..n {
                    let (c, items) = instance(types, seed, 1.0, 1e9);
                    let (out, secs) = run(&c, &items, strategy);
                    if let Err(e) = verify(&c, &items, &out) {
                        failures += 1;
                        println!("INVALID types={types} seed={seed}: {e}");
                    }
                    sum += out["results"][0]["volumeRate"].as_f64().unwrap_or(0.0);
                    time += secs;
                }
                println!(
                    "{types:5} {strategy:8} {:10.2} {:22.3}",
                    sum / n as f64,
                    time / n as f64
                );
                all += sum / n as f64;
                count += 1.0;
            }
            println!("  all {strategy:8} {:10.2}", all / count);
        }
    }
    if want("fit") {
        println!("\n== Does it fit in ONE container? (share of instances, wall) ==");
        println!("cargo volume / container   fits in one   avg containers   time/instance s");
        for fill in [0.70, 0.75, 0.80, 0.85, 0.90] {
            let (mut fits, mut used, mut time, mut count) = (0, 0.0, 0.0, 0.0);
            for types in [5usize, 10, 15] {
                for seed in 0..n {
                    let (c, items) = instance(types, 3000 + seed, fill, 1e9);
                    let (out, secs) = run(&c, &items, "wall");
                    if let Err(e) = verify(&c, &items, &out) {
                        failures += 1;
                        println!("INVALID fit types={types} seed={seed}: {e}");
                    }
                    let k = out["results"].as_array().unwrap().len();
                    fits += usize::from(k == 1);
                    used += k as f64;
                    time += secs;
                    count += 1.0;
                }
            }
            println!(
                "{:10.0}% {:20.0}% {:14.2} {:16.3}",
                fill * 100.0,
                fits as f64 / count * 100.0,
                used / count,
                time / count
            );
        }
    }
    if want("multi") {
        println!("\n== Multi-container (cargo volume = 4.5 containers) ==");
        println!("types  containers(avg)  lower-bound  time/instance s");
        for types in [5usize, 10, 20] {
            let (mut used, mut time) = (0.0, 0.0);
            for seed in 0..n.min(10) {
                let (c, items) = instance(types, 1000 + seed, 4.5, 1e9);
                let (out, secs) = run(&c, &items, "wall");
                if let Err(e) = verify(&c, &items, &out) {
                    failures += 1;
                    println!("INVALID multi types={types} seed={seed}: {e}");
                }
                used += out["results"].as_array().unwrap().len() as f64;
                time += secs;
            }
            let m = n.min(10) as f64;
            println!("{types:5} {:12.2} {:12} {:14.3}", used / m, 5, time / m);
        }
    }
    if want("balance") {
        println!("\n== Balance limits (cargo = 60% volume, heavy/light mix) ==");
        println!(
            "limit                         packed %  cogOffsetX mm  front kg  rear kg  shift mm"
        );
        for (label, extra) in [
            ("none", json!({})),
            (
                "COG ±300 mm",
                json!({"maxCogOffsetX": 300.0, "maxCogOffsetZ": 200.0}),
            ),
            (
                "supports 0/5870, 60% each",
                json!({"frontAxlePos": 0.0, "rearAxlePos": L, "maxFrontAxleLoad": -1.0, "maxRearAxleLoad": -1.0}),
            ),
        ] {
            let (mut packed, mut off, mut front, mut rear, mut shift) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for seed in 0..n {
                let (mut c, items) = instance(8, 5000 + seed, 0.6, 1e9);
                let total: f64 = items
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["weight"].as_f64().unwrap() * i["qty"].as_f64().unwrap())
                    .sum();
                for (k, v) in extra.as_object().unwrap() {
                    c[0][k] = if v.as_f64() == Some(-1.0) {
                        json!(total * 0.6)
                    } else {
                        v.clone()
                    };
                }
                let (out, _) = run(&c, &items, "wall");
                if let Err(e) = verify(&c, &items, &out) {
                    failures += 1;
                    println!("INVALID balance {label} seed={seed}: {e}");
                }
                let r = &out["results"][0];
                let p: Vec<[f64; 3]> = r["placed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| ["x", "dx", "weight"].map(|k| p[k].as_f64().unwrap()))
                    .collect();
                let w: f64 = p.iter().map(|a| a[2]).sum();
                let cx = p.iter().map(|a| a[2] * (a[0] + a[1] / 2.0)).sum::<f64>() / w;
                packed += r["volumeRate"].as_f64().unwrap() / 0.6;
                off += (cx - L / 2.0).abs();
                rear += w * cx / L;
                front += w - w * cx / L;
                shift += r["loadShiftX"].as_f64().unwrap_or(0.0);
            }
            let m = n as f64;
            println!(
                "{label:28} {:8.1} {:13.0} {:9.0} {:8.0} {:9.0}",
                packed / m,
                off / m,
                front / m,
                rear / m,
                shift / m
            );
        }
    }
    if want("scale") {
        println!("\n== Scaling (wall) ==");
        println!("pieces  containers  time s");
        for (types, fill) in [(10usize, 2.0), (20, 6.0), (30, 12.0)] {
            let (c, items) = instance(types, 77, fill, 1e9);
            let pieces: u64 = items
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["qty"].as_u64().unwrap())
                .sum();
            let (out, secs) = run(&c, &items, "wall");
            if let Err(e) = verify(&c, &items, &out) {
                failures += 1;
                println!("INVALID scaling: {e}");
            }
            println!(
                "{pieces:6} {:10} {:8.2}",
                out["results"].as_array().unwrap().len(),
                secs
            );
        }
    }
    println!("\nindependent verification failures: {failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
