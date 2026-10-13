//! Validated mixed-pallet rigid units, per-piece geometry, and SKU accounting.
use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Board {
    length: f64,
    width: f64,
    thickness: f64,
    weight: f64,
    max_load: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    id: String,
    cargo_name: String,
    orientation: u8,
    x: f64,
    y: f64,
    z: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    id: String,
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default = "one")]
    copies: u32,
    length: f64,
    width: f64,
    #[serde(default)]
    max_height: f64,
    #[serde(default)]
    pallet: Option<Board>,
    members: Vec<Member>,
    /// Per-pallet individual layouts (copy indices 2..=copies). Copy 1 is the
    /// template carried by `members` and may not be overridden.
    #[serde(default)]
    custom_layouts: BTreeMap<u32, Vec<Member>>,
}
#[derive(Clone)]
struct Part {
    m: Member,
    item: ItemDef,
    o: Orientation,
    level: u32,
    ceiling: u32,
}
#[derive(Clone)]
struct Checked {
    g: Group,
    parts: Vec<Part>,
    counts: HashMap<String, u32>,
    l: f64,
    w: f64,
    h: f64,
    ph: f64,
    weight: f64,
    yaw: bool,
    min_x: f64,
    min_z: f64,
    /// 0 = template layout; otherwise the 1-based copy index this unit describes.
    copy: u32,
    /// How many physical pallets share this exact layout.
    qty: u32,
}
fn orient(i: &ItemDef, n: u8) -> Option<Orientation> {
    let flags = [i.rot0, i.rot1, i.rot2, i.rot3, i.rot4, i.rot5];
    if n > 5 || !flags[n as usize] {
        return None;
    }
    let vals = [
        (i.l, i.h, i.w),
        (i.w, i.h, i.l),
        (i.h, i.l, i.w),
        (i.w, i.l, i.h),
        (i.l, i.w, i.h),
        (i.h, i.w, i.l),
    ];
    let (dx, dy, dz) = vals[n as usize];
    Some(Orientation { dx, dy, dz })
}
fn overlap(a: &Part, b: &Part) -> f64 {
    ((a.m.x + a.o.dx).min(b.m.x + b.o.dx) - a.m.x.max(b.m.x)).max(0.)
        * ((a.m.z + a.o.dz).min(b.m.z + b.o.dz) - a.m.z.max(b.m.z)).max(0.)
}
fn intersect(a: &Part, b: &Part) -> bool {
    overlap(a, b) > EPS && a.m.y < b.m.y + b.o.dy - EPS && a.m.y + a.o.dy > b.m.y + EPS
}
fn check(g: Group, items: &[ItemDef]) -> Result<Checked, String> {
    let fail = |s: &str| format!("组合「{}」：{}", g.name, s);
    if g.id.is_empty() || g.name.trim().is_empty() {
        return Err(fail("缺少编号或名称"));
    }
    if g.copies == 0 || g.copies > 10000 {
        return Err(fail("托数应为 1–10000"));
    }
    if g.members.is_empty() || g.members.len() > 500 {
        return Err(fail("每个组合需要 1–500 件货物"));
    }
    if !finite_positive(g.length)
        || !finite_positive(g.width)
        || !g.max_height.is_finite()
        || g.max_height < 0.
    {
        return Err(fail("工作区尺寸无效"));
    }
    let (l, w, ph, tare, cap) = if let Some(b) = &g.pallet {
        if !finite_positive(b.length)
            || !finite_positive(b.width)
            || !finite_positive(b.thickness)
            || !b.weight.is_finite()
            || b.weight < 0.
            || !b.max_load.is_finite()
            || b.max_load < 0.
        {
            return Err(fail("托盘尺寸、自重或承载上限无效"));
        }
        (b.length, b.width, b.thickness, b.weight, b.max_load)
    } else {
        (g.length, g.width, 0., 0., f64::MAX)
    };
    let mut ids = HashSet::new();
    let mut parts = Vec::new();
    let mut counts = HashMap::new();
    let mut mass = 0.;
    for m in &g.members {
        if m.id.is_empty() || !ids.insert(m.id.clone()) {
            return Err(fail("单件编号重复或为空"));
        }
        let item = items
            .iter()
            .find(|i| i.name == m.cargo_name)
            .ok_or_else(|| fail("组合引用了不存在或未启用的货物"))?;
        if !item_is_valid(item) {
            return Err(fail("货物尺寸或重量无效"));
        }
        let o = orient(item, m.orientation).ok_or_else(|| fail("单件方向不在第二步允许范围内"))?;
        if ![m.x, m.y, m.z].iter().all(|v| v.is_finite() && *v >= -EPS) {
            return Err(fail("单件坐标无效或小于零"));
        }
        if m.x + o.dx > l + EPS || m.z + o.dz > w + EPS {
            return Err(fail("货物超出托盘或工作区边界"));
        }
        if g.max_height > 0. && m.y + o.dy + ph > g.max_height + EPS {
            return Err(fail("超过组合最大高度"));
        }
        if item.no_understack && m.y > EPS {
            return Err(fail("必须置底的货物只能放在托盘顶面或组合底面"));
        }
        *counts.entry(item.name.clone()).or_insert(0_u32) += 1;
        mass += item.weight;
        parts.push(Part {
            m: m.clone(),
            item: item.clone(),
            o,
            level: 1,
            ceiling: u32::MAX,
        });
    }
    if mass > cap + EPS {
        return Err(fail("货物净重超过托盘承载上限"));
    }
    for (name, n) in &counts {
        let needed = n.checked_mul(g.copies).ok_or_else(|| fail("件数溢出"))?;
        let source = items.iter().find(|i| i.name == *name).unwrap();
        if needed > source.qty {
            return Err(fail("每托用量 × 托数超过货物库存"));
        }
    }
    for i in 0..parts.len() {
        for j in i + 1..parts.len() {
            if intersect(&parts[i], &parts[j]) {
                return Err(fail("货物之间发生三维碰撞"));
            }
        }
    }
    for p in &parts {
        for q in &parts {
            if p.m.id != q.m.id
                && q.item.no_overstack
                && p.m.y >= q.m.y + q.o.dy - SUPPORT_EPS
                && overlap(p, q) > EPS
            {
                return Err(fail("不可堆叠的货物上方禁止放置任何货物（含跨高桥接）"));
            }
        }
    }
    parts.sort_by(|a, b| {
        a.m.y
            .total_cmp(&b.m.y)
            .then(b.item.priority.cmp(&a.item.priority))
            .then(a.m.id.cmp(&b.m.id))
    });
    for i in 0..parts.len() {
        let p = &parts[i];
        let mut level = 1;
        let mut supporters = Vec::new();
        if p.m.y > EPS {
            let mut area = 0.;
            for j in 0..i {
                let q = &parts[j];
                if (q.m.y + q.o.dy - p.m.y).abs() <= SUPPORT_EPS && overlap(p, q) > EPS {
                    area += overlap(p, q);
                    supporters.push(j);
                    level = level.max(q.level + 1);
                    if q.item.no_overstack {
                        return Err(fail("不可堆叠的货物上方承托了其他货物"));
                    }
                    if p.item.priority > q.item.priority {
                        return Err(fail("支撑关系与第二步装载顺序冲突：底件必须先于上件"));
                    }
                }
            }
            if area + EPS < p.o.dx * p.o.dz {
                return Err(fail("高处货物没有同一高度的完整底面支撑"));
            }
        }
        if p.item.max_stack > 0 && level > p.item.max_stack {
            return Err(fail("单件超过最多堆叠层数"));
        }
        for &j in &supporters {
            if parts[j].item.max_stack > 0 && level > parts[j].item.max_stack {
                return Err(fail("上件超过支撑货物的限层"));
            }
        }
        let mut ceiling = if p.item.max_stack > 0 {
            p.item.max_stack
        } else {
            u32::MAX
        };
        for j in supporters {
            ceiling = ceiling.min(parts[j].ceiling)
        }
        if level > ceiling {
            return Err(fail("超过整个支撑链的堆叠限层"));
        }
        parts[i].level = level;
        parts[i].ceiling = ceiling;
    }
    let min_x = if ph > 0. {
        0.
    } else {
        parts.iter().map(|p| p.m.x).fold(f64::MAX, f64::min)
    };
    let min_z = if ph > 0. {
        0.
    } else {
        parts.iter().map(|p| p.m.z).fold(f64::MAX, f64::min)
    };
    let size_l = if ph > 0. {
        l
    } else {
        parts.iter().map(|p| p.m.x + p.o.dx).fold(0., f64::max) - min_x
    };
    let size_w = if ph > 0. {
        w
    } else {
        parts.iter().map(|p| p.m.z + p.o.dz).fold(0., f64::max) - min_z
    };
    let h = ph + parts.iter().map(|p| p.m.y + p.o.dy).fold(0., f64::max);
    let yaw = parts.iter().all(|p| {
        orientations(&p.item).iter().any(|o| {
            (o.dx - p.o.dz).abs() < EPS
                && (o.dy - p.o.dy).abs() < EPS
                && (o.dz - p.o.dx).abs() < EPS
        })
    });
    parts.sort_by(|a, b| {
        b.item
            .priority
            .cmp(&a.item.priority)
            .then(a.m.y.total_cmp(&b.m.y))
            .then(a.m.id.cmp(&b.m.id))
    });
    let copies = g.copies;
    Ok(Checked {
        g,
        parts,
        counts,
        l: size_l,
        w: size_w,
        h,
        ph,
        weight: mass + tare,
        yaw,
        min_x,
        min_z,
        copy: 0,
        qty: copies,
    })
}
fn parse_items(text: &str) -> Result<Vec<ItemDef>, String> {
    let mut items: Vec<ItemDef> =
        serde_json::from_str(text).map_err(|e| format!("货物数据格式无效：{e}"))?;
    let mut names = HashSet::new();
    for i in &mut items {
        if i.name.trim().is_empty()
            || !item_is_valid(i)
            || i.qty == 0
            || !names.insert(i.name.clone())
            || orientations(i).is_empty()
        {
            return Err("货物尺寸、数量、名称或方向无效".into());
        }
        i.palletize = false;
        i.piece_count = 1;
        i.stack_height = 1;
        i.source_name = None;
    }
    Ok(items)
}
fn summary(c: &Checked) -> Value {
    json!({"length":c.l,"width":c.w,"height":c.h,"weight":c.weight,"cargoWeight":c.weight-c.g.pallet.as_ref().map_or(0.,|b|b.weight),"piecesPerCopy":c.parts.len(),"copies":c.g.copies,"yaw90Allowed":c.yaw,"counts":c.counts,"assemblyOrder":c.parts.iter().map(|p|p.m.id.clone()).collect::<Vec<_>>()})
}
#[wasm_bindgen]
pub fn validate_pallet_v4(items_json: &str, group_json: &str) -> String {
    let result = (|| {
        let items = parse_items(items_json)?;
        let group: Group =
            serde_json::from_str(group_json).map_err(|e| format!("组合数据格式无效：{e}"))?;
        check(group, &items)
    })();
    match result {
        Ok(c) => json!({"valid":true,"summary":summary(&c)}).to_string(),
        Err(e) => json!({"valid":false,"errors":[e]}).to_string(),
    }
}
#[wasm_bindgen]
pub fn run_pack_engine_v4(
    templates_json: &str,
    items_json: &str,
    groups_json: &str,
    priority: &str,
    strategy: &str,
    preference: &str,
) -> String {
    let result = (|| -> Result<Value, String> {
        let templates: Vec<ContainerDef> =
            serde_json::from_str(templates_json).map_err(|e| format!("货柜格式无效：{e}"))?;
        let items = parse_items(items_json)?;
        let groups: Vec<Group> =
            serde_json::from_str(groups_json).map_err(|e| format!("组合格式无效：{e}"))?;
        if groups.len() > 200 {
            return Err("组合数量超过 200，请拆分方案".into());
        }
        let mut remaining = items.clone();
        let mut units = BTreeMap::<String, Checked>::new();
        let mut group_ids = HashSet::new();
        for g in groups.into_iter().filter(|g| g.enabled) {
            if !group_ids.insert(g.id.clone()) {
                return Err("组合编号重复".into());
            }
            // Per-pallet layouts: copy 1 is the template; indices 2..=copies
            // may carry an individually edited member list.
            if g.custom_layouts.is_empty() {
                let c = check(g, &items)?;
                for (name, n) in &c.counts {
                    let needed = n.checked_mul(c.g.copies).ok_or("库存计数溢出")?;
                    let i = remaining.iter_mut().find(|i| i.name == *name).unwrap();
                    if needed > i.qty {
                        return Err(format!("货物「{}」被多个组合重复占用", name));
                    }
                    i.qty -= needed;
                }
                let name = format!("__pallet_v4_{}", c.g.id);
                if items.iter().any(|i| i.name == name) {
                    return Err("货物名称与内部组合编号冲突".into());
                }
                units.insert(name, c);
                continue;
            }
            if g.custom_layouts.contains_key(&1) {
                return Err(format!(
                    "组合「{}」第 1 托固定为模板，请从第 2 托起单独调整",
                    g.name
                ));
            }
            let copies = g.copies;
            let mut slots: Vec<Checked> = Vec::with_capacity(g.custom_layouts.len() + 1);
            for (&k, members) in &g.custom_layouts {
                if k < 2 || k > copies {
                    return Err(format!(
                        "组合「{}」的逐托调整序号 {} 超出 2–{} 范围",
                        g.name, k, copies
                    ));
                }
                if members.is_empty() {
                    return Err(format!("组合「{}」第 {} 托不能为空托", g.name, k));
                }
                let mut cg = g.clone();
                cg.members = members.clone();
                cg.copies = 1;
                cg.name = format!("{}（第 {} 托）", g.name, k);
                let mut c = check(cg, &items)?;
                c.copy = k;
                c.qty = 1;
                c.g.name = g.name.clone();
                slots.push(c);
            }
            let mut template = g.clone();
            template.members = g.members.clone();
            template.custom_layouts.clear();
            let custom_count = slots.len() as u32;
            let mut c0 = check(template, &items)?;
            c0.copy = 0;
            c0.qty = copies.checked_sub(custom_count).ok_or("逐托调整数量超过托数")?;
            // 库存按 模板计数×模板托数 + 每定制托计数 合计校核，且不同组合间不重复占用。
            let mut totals = BTreeMap::<String, u64>::new();
            for c in std::iter::once(&c0).chain(slots.iter()) {
                for (name, n) in &c.counts {
                    *totals.entry(name.clone()).or_default() += (*n as u64) * c.qty as u64;
                }
            }
            for (name, needed) in totals {
                let i = remaining.iter_mut().find(|i| i.name == name).unwrap();
                if needed > i.qty as u64 {
                    return Err(format!("货物「{}」被多个组合重复占用", name));
                }
                i.qty -= needed as u32;
            }
            slots.insert(0, c0);
            for c in slots {
                if c.qty == 0 {
                    continue;
                }
                let name = if c.copy == 0 {
                    format!("__pallet_v4_{}", c.g.id)
                } else {
                    format!("__pallet_v4_{}__c{}", c.g.id, c.copy)
                };
                if items.iter().any(|i| i.name == name) {
                    return Err("货物名称与内部组合编号冲突".into());
                }
                units.insert(name, c);
            }
        }
        remaining.retain(|i| i.qty > 0);
        for (name, c) in &units {
            let layers = c.parts.iter().map(|p| p.level).max().unwrap_or(1);
            // Limit the global vertical offset of every member, conservatively using
            // the smallest remaining allowance in this rigid unit.
            let allowance = c
                .parts
                .iter()
                .filter(|p| p.item.max_stack > 0)
                .map(|p| p.item.max_stack.saturating_sub(p.level))
                .min();
            let max_stack = allowance.map_or(0, |n| layers.saturating_add(n));
            let no_overstack = c.parts.iter().any(|p| p.item.no_overstack);
            let no_understack = c.parts.iter().any(|p| p.item.no_understack);
            let faces = c
                .parts
                .iter()
                .filter(|p| (c.ph + p.m.y + p.o.dy - c.h).abs() <= SUPPORT_EPS)
                .map(|p| SupportFace {
                    x: p.m.x - c.min_x,
                    z: p.m.z - c.min_z,
                    dx: p.o.dx,
                    dz: p.o.dz,
                })
                .collect();
            let base_faces = c
                .parts
                .iter()
                .filter(|p| p.m.y <= EPS)
                .map(|p| SupportFace {
                    x: p.m.x - c.min_x,
                    z: p.m.z - c.min_z,
                    dx: p.o.dx,
                    dz: p.o.dz,
                })
                .collect();
            let rigid = RigidSupport {
                faces,
                base_faces,
                length: c.l,
                width: c.w,
                capacity: c.g.pallet.as_ref().map(|b| b.max_load),
                cargo_weight: c.weight - c.g.pallet.as_ref().map_or(0., |b| b.weight),
                load_above: 0.,
                parents: Vec::new(),
            };
            remaining.push(ItemDef {
                name: name.clone(),
                color: "#ad8556".into(),
                l: c.l,
                w: c.w,
                h: c.h,
                weight: c.weight,
                qty: c.qty,
                priority: c.parts.iter().map(|p| p.item.priority).max().unwrap_or(0),
                rot0: true,
                rot1: c.yaw,
                rot2: false,
                rot3: false,
                rot4: false,
                rot5: false,
                max_stack,
                no_overstack,
                no_understack,
                palletize: false,
                pallet_columns: 1,
                pallet_layers: 1,
                stack_height: layers,
                piece_count: 1,
                source_name: None,
                rigid: Some(rigid),
            });
        }
        let mut out: Value =
            serde_json::from_str(&solve(templates, remaining, priority, strategy, preference))
                .map_err(|e| e.to_string())?;
        if out.get("error").is_some() {
            return Ok(out);
        }
        let mut aggregate = BTreeMap::<String, u64>::new();
        for (name, n) in out["globalUnplacedAgg"].as_object().ok_or("缺少未装清单")? {
            let qty = n.as_u64().unwrap_or(0);
            if let Some(c) = units.get(name) {
                for (sku, count) in &c.counts {
                    *aggregate.entry(sku.clone()).or_default() += qty * (*count as u64)
                }
            } else {
                *aggregate.entry(name.clone()).or_default() += qty
            }
        }
        out["globalUnplacedAgg"] = json!(aggregate);
        for (ci, r) in out["results"]
            .as_array_mut()
            .ok_or("缺少结果")?
            .iter_mut()
            .enumerate()
        {
            let original = r["placed"].as_array().ok_or("缺少位置")?.clone();
            let mut placed = Vec::new();
            let mut bases = Vec::new();
            let mut group_records = Vec::new();
            for (ui, u) in original.iter().enumerate() {
                let Some(c) = units.get(u["name"].as_str().unwrap_or("")) else {
                    placed.push(u.clone());
                    continue;
                };
                let ux = u["x"].as_f64().unwrap();
                let uy = u["y"].as_f64().unwrap();
                let uz = u["z"].as_f64().unwrap();
                let yaw = (u["dx"].as_f64().unwrap() - c.l).abs() > EPS
                    || (u["dz"].as_f64().unwrap() - c.w).abs() > EPS;
                let instance = format!("{}:{}:{}", c.g.id, ci, ui);
                let first = placed.len();
                for p in &c.parts {
                    let x = p.m.x - c.min_x;
                    let z = p.m.z - c.min_z;
                    let (x, z, dx, dz) = if yaw {
                        (c.w - z - p.o.dz, x, p.o.dz, p.o.dx)
                    } else {
                        (x, z, p.o.dx, p.o.dz)
                    };
                    let placed_orientation = if yaw {
                        (0..6)
                            .find(|n| {
                                orient(&p.item, *n).is_some_and(|o| {
                                    (o.dx - dx).abs() < EPS
                                        && (o.dy - p.o.dy).abs() < EPS
                                        && (o.dz - dz).abs() < EPS
                                })
                            })
                            .ok_or("整托旋转后的单件朝向无法回映射")?
                    } else {
                        p.m.orientation
                    };
                    placed.push(json!({"name":p.item.name,"cargoName":p.item.name,"color":p.item.color,"weight":p.item.weight,"x":ux+x,"y":uy+c.ph+p.m.y,"z":uz+z,"dx":dx,"dy":p.o.dy,"dz":dz,"noOverstack":p.item.no_overstack,"maxStack":p.item.max_stack,"stackLevel":u["stackLevel"].as_u64().unwrap_or(1).saturating_sub(c.parts.iter().map(|p|p.level as u64).max().unwrap_or(1))+p.level as u64,"pieceCount":1,"palletized":false,"palletColumns":1,"palletLayers":1,"groupId":c.g.id,"groupName":c.g.name,"groupInstance":instance,"memberId":p.m.id,"orientation":placed_orientation,"originalOrientation":p.m.orientation,"unitYaw90":yaw}));
                }
                let last = placed.len() - 1;
                if let Some(b) = &c.g.pallet {
                    bases.push(json!({"id":instance,"groupId":c.g.id,"groupName":c.g.name,"x":ux,"y":uy,"z":uz,"dx":u["dx"],"dy":b.thickness,"dz":u["dz"],"weight":b.weight,"firstItemIndex":first,"lastItemIndex":last}));
                }
                group_records.push(json!({"id":instance,"groupId":c.g.id,"name":c.g.name,"copyIndex":c.copy,"firstItemIndex":first,"lastItemIndex":last,"x":ux,"y":uy,"z":uz,"length":u["dx"],"height":c.h,"width":u["dz"],"weight":c.weight,"yaw90":yaw,"contents":c.counts,"stackLayer":u["stackLevel"],"stackable":!c.parts.iter().any(|p|p.item.no_overstack)}));
            }
            let volume = r["containerLength"].as_f64().unwrap()
                * r["containerWidth"].as_f64().unwrap()
                * r["containerHeight"].as_f64().unwrap();
            let used: f64 = placed
                .iter()
                .map(|p| {
                    p["dx"].as_f64().unwrap()
                        * p["dy"].as_f64().unwrap()
                        * p["dz"].as_f64().unwrap()
                })
                .sum::<f64>()
                + bases
                    .iter()
                    .map(|p| {
                        p["dx"].as_f64().unwrap()
                            * p["dy"].as_f64().unwrap()
                            * p["dz"].as_f64().unwrap()
                    })
                    .sum::<f64>();
            r["reservedVolumeRate"] = r["volumeRate"].clone();
            r["volumeRate"] = json!(used / volume * 100.);
            r["placedCount"] = json!(placed.len());
            r["placed"] = json!(placed);
            r["palletBases"] = json!(bases);
            r["palletUnits"] = json!(group_records);
        }
        out["engineVersion"] = json!("EMS v4.8 load-path mixed-pallet");
        Ok(out)
    })();
    match result {
        Ok(v) => v.to_string(),
        Err(e) => json!({"error":e,"engineVersion":"EMS v4.8 load-path mixed-pallet"}).to_string(),
    }
}
#[wasm_bindgen]
pub fn arrange_pallet_v4(items_json: &str, group_json: &str) -> String {
    let result = (|| -> Result<Value, String> {
        let items = parse_items(items_json)?;
        let mut g: Group = serde_json::from_str(group_json).map_err(|e| e.to_string())?;
        if g.members.is_empty() || g.members.len() > 500 {
            return Err("混排需要 1–500 个单件".into());
        }
        let ph = g.pallet.as_ref().map_or(0., |b| b.thickness);
        let l = g.pallet.as_ref().map_or(g.length, |b| b.length);
        let w = g.pallet.as_ref().map_or(g.width, |b| b.width);
        let mut h = if g.max_height > ph {
            g.max_height - ph
        } else {
            2500.
        };
        let cap = g.pallet.as_ref().map_or(f64::MAX, |b| b.max_load);
        let mut pool = Vec::new();
        for m in &g.members {
            let mut i = items
                .iter()
                .find(|i| i.name == m.cargo_name)
                .ok_or("未知货物")?
                .clone();
            i.name = m.id.clone();
            i.source_name = Some(m.id.clone());
            i.qty = 1;
            i.palletize = false;
            pool.push(i);
        }
        if g.max_height == 0. {
            h = h.max(
                pool.iter()
                    .map(|i| orientations(i).iter().map(|o| o.dy).fold(0., f64::max))
                    .sum::<f64>(),
            );
        }
        let bin = ContainerDef {
            key: "pallet".into(),
            label: "Pallet".into(),
            length: l,
            width: w,
            height: h,
            max_weight: cap.max(EPS * 10.),
        };
        if !container_is_valid(&bin) {
            return Err("托盘工作区尺寸无效".into());
        }
        let mut best = None;
        for v in 0..18 {
            let r = pack_one_container(&bin, &pool, "floor", v, "large", None);
            if r.unplaced.is_empty() {
                let mut trial = g.clone();
                for p in &r.placed {
                    if let Some(m) = trial.members.iter_mut().find(|m| m.id == p.name) {
                        let i = items.iter().find(|i| i.name == m.cargo_name).unwrap();
                        m.x = p.x;
                        m.y = p.y;
                        m.z = p.z;
                        m.orientation = (0..6)
                            .find(|n| {
                                orient(i, *n).is_some_and(|o| {
                                    (o.dx - p.dx).abs() < EPS
                                        && (o.dy - p.dy).abs() < EPS
                                        && (o.dz - p.dz).abs() < EPS
                                })
                            })
                            .unwrap_or(255);
                    }
                }
                if check(trial, &items).is_err() {
                    continue;
                }
            }
            if best.as_ref().map_or(true, |b: &PackResult| {
                r.placed_count > b.placed_count
                    || (r.placed_count == b.placed_count
                        && r.placed.iter().map(|p| p.y + p.dy).fold(0., f64::max)
                            < b.placed.iter().map(|p| p.y + p.dy).fold(0., f64::max))
            }) {
                best = Some(r)
            }
        }
        let r =
            best.ok_or("所有混排候选均违反方向、支撑、限层、顺序、承载或库存约束，原布局未改动")?;
        let unplaced = r
            .unplaced
            .iter()
            .map(|i| i.name.clone())
            .collect::<Vec<_>>();
        if !unplaced.is_empty() {
            return Ok(
                json!({"success":false,"unplaced":unplaced,"message":"空间、支撑、朝向、堆叠或承载限制下无法完整混排；原布局未改动"}),
            );
        }
        for p in r.placed {
            let m = g.members.iter_mut().find(|m| m.id == p.name).unwrap();
            let i = items.iter().find(|i| i.name == m.cargo_name).unwrap();
            m.x = p.x;
            m.y = p.y;
            m.z = p.z;
            m.orientation = (0..6)
                .find(|n| {
                    orient(i, *n).is_some_and(|o| {
                        (o.dx - p.dx).abs() < EPS
                            && (o.dy - p.dy).abs() < EPS
                            && (o.dz - p.dz).abs() < EPS
                    })
                })
                .ok_or("方向无法回映射")?;
        }
        let c = check(g.clone(), &items)?;
        Ok(json!({"success":true,"group":g,"summary":summary(&c)}))
    })();
    match result {
        Ok(v) => v.to_string(),
        Err(e) => json!({"success":false,"error":e}).to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn i(name: &str) -> Value {
        json!({"name":name,"color":"#79a886","l":100.,"w":100.,"h":100.,"weight":5.,"qty":10,"rot0":true,"rot1":true,"rot2":false,"rot3":false,"rot4":false,"rot5":false})
    }
    fn g() -> Value {
        json!({"id":"g","name":"混托","enabled":true,"copies":2,"length":300.,"width":200.,"maxHeight":500.,"pallet":{"length":300.,"width":200.,"thickness":50.,"weight":10.,"maxLoad":100.},"members":[{"id":"a","cargoName":"A","orientation":0,"x":0.,"y":0.,"z":0.},{"id":"b","cargoName":"B","orientation":0,"x":100.,"y":0.,"z":0.}]})
    }
    #[test]
    fn mixture_tare_inventory() {
        let t = json!([{"key":"x","label":"柜","length":1000.,"width":500.,"height":500.,"maxWeight":100.}]);
        let out: Value = serde_json::from_str(&run_pack_engine_v4(
            &t.to_string(),
            &json!([i("A"), i("B")]).to_string(),
            &json!([g()]).to_string(),
            "large",
            "floor",
            "large",
        ))
        .unwrap();
        assert!(out.get("error").is_none(), "{out}");
        let qty: u64 = out["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["placedCount"].as_u64().unwrap())
            .sum();
        assert_eq!(qty, 20);
        let weight: f64 = out["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["totalWeight"].as_f64().unwrap())
            .sum();
        assert_eq!(weight, 120.);
        assert_eq!(
            out["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["palletBases"].as_array().unwrap().len())
                .sum::<usize>(),
            2
        );
    }
    #[test]
    fn collision_and_support() {
        let mut v = g();
        v["members"][1]["x"] = json!(50.);
        let out: Value = serde_json::from_str(&validate_pallet_v4(
            &json!([i("A"), i("B")]).to_string(),
            &v.to_string(),
        ))
        .unwrap();
        assert_eq!(out["valid"], false);
        v["members"][1]["y"] = json!(200.);
        let out: Value = serde_json::from_str(&validate_pallet_v4(
            &json!([i("A"), i("B")]).to_string(),
            &v.to_string(),
        ))
        .unwrap();
        assert_eq!(out["valid"], false);
    }
    #[test]
    fn global_double_reservation() {
        let mut v = g();
        v["copies"] = json!(6);
        let mut g2 = v.clone();
        g2["id"] = json!("second");
        let out: Value = serde_json::from_str(&run_pack_engine_v4(
            "[]",
            &json!([i("A"), i("B")]).to_string(),
            &json!([v, g2]).to_string(),
            "large",
            "floor",
            "large",
        ))
        .unwrap();
        assert!(out["error"].as_str().unwrap().contains("重复占用"));
    }
    #[test]
    fn auto_arranges_real_members() {
        let mut v = g();
        v["members"][1]["x"] = json!(0.);
        let out: Value = serde_json::from_str(&arrange_pallet_v4(
            &json!([i("A"), i("B")]).to_string(),
            &v.to_string(),
        ))
        .unwrap();
        assert_eq!(out["success"], true, "{out}");
    }
    #[test]
    fn inherited_stack_ceiling() {
        let mut a = i("A");
        a["maxStack"] = json!(2);
        let mut v = g();
        v["members"][1]["x"] = json!(0.);
        v["members"][1]["y"] = json!(100.);
        v["members"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c","cargoName":"C","orientation":0,"x":0.,"y":200.,"z":0.}));
        let out: Value = serde_json::from_str(&validate_pallet_v4(
            &json!([a, i("B"), i("C")]).to_string(),
            &v.to_string(),
        ))
        .unwrap();
        assert_eq!(out["valid"], false);
        assert!(out["errors"][0].as_str().unwrap().contains("支撑链"));
    }
    #[test]
    fn physical_floor_and_yaw_constraints() {
        let mut a = i("A");
        a["l"] = json!(120.);
        a["rot1"] = json!(false);
        a["noUnderstack"] = json!(true);
        let mut v = g();
        v["members"][1]["x"] = json!(120.);
        let out: Value = serde_json::from_str(&validate_pallet_v4(
            &json!([a, i("B")]).to_string(),
            &v.to_string(),
        ))
        .unwrap();
        assert_eq!(out["valid"], true);
        assert_eq!(out["summary"]["height"], json!(150.));
        assert_eq!(out["summary"]["yaw90Allowed"], false);
    }

    #[test]
    fn assembly_priority_precedes_independent_floor_piece() {
        let mut a = i("A");
        a["priority"] = json!(3);
        let mut b = i("B");
        b["priority"] = json!(2);
        let mut c = i("C");
        c["priority"] = json!(1);
        let mut v = g();
        v["copies"] = json!(1);
        v["members"][1]["x"] = json!(0.);
        v["members"][1]["y"] = json!(100.);
        v["members"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c","cargoName":"C","orientation":0,"x":100.,"y":0.,"z":0.}));
        let out: Value = serde_json::from_str(&validate_pallet_v4(
            &json!([a, b, c]).to_string(),
            &v.to_string(),
        ))
        .unwrap();
        assert_eq!(out["valid"], true);
        assert_eq!(out["summary"]["assemblyOrder"], json!(["a", "b", "c"]));
    }
}

#[cfg(test)]
#[path = "stacking_tests.rs"]
mod stacking_tests;
