//! Three-dimensional container loading engine.
//!
//! The engine uses the Maximal Empty Spaces (EMS) heuristic, a standard
//! practical approach for orthogonal 3D bin packing.  Every placement splits
//! the currently available empty spaces, rather than relying on a handful of
//! box corners.  This keeps candidates aligned to actual free-volume faces and
//! makes the result compact while retaining all UI loading constraints.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

const EPS: f64 = 1e-6;
const SUPPORT_EPS: f64 = 1e-3;
const MAX_EMPTY_SPACES: usize = 1_024;
const MAX_TOP_SURFACE_ANCHORS: usize = 64;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDef {
    pub key: String,
    pub label: String,
    pub length: f64,
    pub width: f64,
    pub height: f64,
    pub max_weight: f64,
}

// Internal physical support metadata cannot be supplied by imported JSON.
#[derive(Clone, Debug)]
struct SupportFace {
    x: f64,
    z: f64,
    dx: f64,
    dz: f64,
}
#[derive(Clone, Debug)]
struct RigidSupport {
    /// Real top faces offered to whatever rests on this unit (supporter role).
    faces: Vec<SupportFace>,
    /// Real bottom faces of the unit's own members sitting on the deck. When
    /// this unit is raised, every one of these faces must be completely
    /// covered by real top faces at one common elevation (load-path complete
    /// support). The deck itself may overhang, matching real pallet work.
    base_faces: Vec<SupportFace>,
    width: f64,
    length: f64,
    capacity: Option<f64>,
    cargo_weight: f64,
    load_above: f64,
    parents: Vec<usize>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ItemDef {
    #[serde(skip)]
    rigid: Option<RigidSupport>,
    pub name: String,
    pub color: String,
    pub l: f64,
    pub w: f64,
    pub h: f64,
    pub weight: f64,
    pub qty: u32,
    /// Optional browser input. Greater values are loaded first; zero follows
    /// the selected default large-cargo / small-cargo loading preference.
    #[serde(default)]
    pub priority: u32,
    pub rot0: bool,
    pub rot1: bool,
    pub rot2: bool,
    pub rot3: bool,
    pub rot4: bool,
    pub rot5: bool,
    #[serde(default)]
    pub max_stack: u32,
    #[serde(default)]
    pub no_overstack: bool,
    #[serde(default)]
    pub no_understack: bool,
    #[serde(default)]
    pub palletize: bool,
    #[serde(default = "one")]
    pub pallet_columns: u32,
    #[serde(default = "one")]
    pub pallet_layers: u32,
    // Solver metadata used after palletisation. These have defaults so the
    // browser's existing input contract is unchanged.
    #[serde(default = "one")]
    pub stack_height: u32,
    #[serde(default = "one")]
    pub piece_count: u32,
    #[serde(default)]
    pub source_name: Option<String>,
}

fn one() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlacedItem {
    #[serde(skip)]
    rigid: Option<RigidSupport>,
    #[serde(skip)]
    stack_ceiling: u32,
    pub name: String,
    pub cargo_name: String,
    pub color: String,
    pub weight: f64,
    // x = container length, y = height, z = width.
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub no_overstack: bool,
    pub stack_level: u32,
    pub max_stack: u32,
    pub piece_count: u32,
    pub palletized: bool,
    pub pallet_columns: u32,
    pub pallet_layers: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PackResult {
    pub container_name: String,
    pub container_width: f64,
    pub container_height: f64,
    pub container_length: f64,
    pub volume_rate: f64,
    pub weight_rate: f64,
    pub placed_count: usize,
    pub total_weight: f64,
    pub used_length: f64,
    pub tail_space: f64,
    pub placed: Vec<PlacedItem>,
    pub unplaced: Vec<ItemDef>,
}

#[derive(Clone, Copy, Debug)]
struct Space {
    x: f64,
    y: f64,
    z: f64,
    dx: f64,
    dy: f64,
    dz: f64,
}

#[derive(Clone, Copy, Debug)]
struct Orientation {
    dx: f64,
    dy: f64,
    dz: f64,
}

#[derive(Clone, Copy, Debug)]
struct SupportInfo {
    stack_level: u32,
    ceiling: u32,
}

#[derive(Clone, Debug)]
struct Candidate {
    item_index: usize,
    x: f64,
    y: f64,
    z: f64,
    priority: u32,
    preference_rank: usize,
    continuation_rank: u8,
    orientation: Orientation,
    support: SupportInfo,
    type_rank: usize,
    slack_axes: u8,
    free_volume: f64,
    largest_remainder: f64,
    grouping_score: f64,
}

#[derive(Clone, Copy)]
struct PlanScore {
    packed_volume: f64,
    unplaced_pieces: u32,
    total_container_volume: f64,
    container_count: usize,
}

fn finite_positive(value: f64) -> bool {
    value.is_finite() && value > EPS
}

fn container_is_valid(container: &ContainerDef) -> bool {
    finite_positive(container.length)
        && finite_positive(container.width)
        && finite_positive(container.height)
        && container.max_weight.is_finite()
        && container.max_weight >= 0.0
}

fn item_is_valid(item: &ItemDef) -> bool {
    finite_positive(item.l)
        && finite_positive(item.w)
        && finite_positive(item.h)
        && item.weight.is_finite()
        && item.weight >= 0.0
}

fn container_volume(container: &ContainerDef) -> f64 {
    container.length * container.width * container.height
}

fn item_volume(item: &ItemDef) -> f64 {
    item.l * item.w * item.h
}

fn placed_volume(item: &PlacedItem) -> f64 {
    item.dx * item.dy * item.dz
}

fn space_volume(space: Space) -> f64 {
    space.dx * space.dy * space.dz
}

fn orientations(item: &ItemDef) -> Vec<Orientation> {
    let mut values = Vec::with_capacity(6);
    let mut push_unique = |dx: f64, dy: f64, dz: f64| {
        if !finite_positive(dx) || !finite_positive(dy) || !finite_positive(dz) {
            return;
        }
        let orientation = Orientation { dx, dy, dz };
        if !values.iter().any(|other: &Orientation| {
            (other.dx - orientation.dx).abs() <= EPS
                && (other.dy - orientation.dy).abs() <= EPS
                && (other.dz - orientation.dz).abs() <= EPS
        }) {
            values.push(orientation);
        }
    };

    // The UI defines these mappings as x=length, y=height, z=width.
    if item.rot0 {
        push_unique(item.l, item.h, item.w);
    }
    if item.rot1 {
        push_unique(item.w, item.h, item.l);
    }
    if item.rot2 {
        push_unique(item.h, item.l, item.w);
    }
    if item.rot3 {
        push_unique(item.w, item.l, item.h);
    }
    if item.rot4 {
        push_unique(item.l, item.w, item.h);
    }
    if item.rot5 {
        push_unique(item.h, item.w, item.l);
    }
    values
}

/// Converts a selected SKU into indivisible full pallet units plus loose
/// remainder cartons. A pallet keeps the first permitted product orientation,
/// matching the existing UI's visual split convention.
fn prepare_items(items: Vec<ItemDef>) -> Result<Vec<ItemDef>, String> {
    let mut prepared = Vec::with_capacity(items.len() * 2);
    for mut item in items {
        item.stack_height = item.stack_height.max(1);
        item.piece_count = item.piece_count.max(1);
        if !item_is_valid(&item) {
            return Err(format!("{} has invalid dimensions or weight.", item.name));
        }
        if orientations(&item).is_empty() {
            return Err(format!(
                "{} has no permitted loading orientation.",
                item.name
            ));
        }
        if !item.palletize {
            prepared.push(item);
            continue;
        }
        if item.pallet_columns == 0 || item.pallet_layers == 0 {
            return Err(format!(
                "{} pallet columns and layers must both be at least one.",
                item.name
            ));
        }
        if item.pallet_layers > 1 && item.no_understack {
            return Err(format!(
                "{} is floor-only and cannot be made into a multi-layer pallet.",
                item.name
            ));
        }
        if item.pallet_layers > 1 && item.no_overstack {
            return Err(format!(
                "{} is non-stackable and cannot be made into a multi-layer pallet.",
                item.name
            ));
        }
        if item.max_stack > 0 && item.pallet_layers > item.max_stack {
            return Err(format!(
                "{} pallet layers exceed its maximum stack level ({}).",
                item.name, item.max_stack
            ));
        }

        let pieces_per_pallet = item
            .pallet_columns
            .checked_mul(item.pallet_layers)
            .ok_or_else(|| format!("{} pallet configuration is too large.", item.name))?;
        let pallet_qty = item.qty / pieces_per_pallet;
        let loose_qty = item.qty % pieces_per_pallet;
        if pallet_qty > 0 {
            let base = orientations(&item)[0];
            let mut pallet = item.clone();
            pallet.name = format!("{} [Pallet]", item.name);
            pallet.source_name = Some(item.name.clone());
            // Store the fixed x/y/z layout in the rot0 source fields.
            pallet.l = base.dx * item.pallet_columns as f64;
            pallet.w = base.dz;
            pallet.h = base.dy * item.pallet_layers as f64;
            pallet.weight = item.weight * pieces_per_pallet as f64;
            pallet.qty = pallet_qty;
            pallet.rot0 = true;
            pallet.rot1 = false;
            pallet.rot2 = false;
            pallet.rot3 = false;
            pallet.rot4 = false;
            pallet.rot5 = false;
            pallet.palletize = false;
            pallet.stack_height = item.pallet_layers;
            pallet.piece_count = pieces_per_pallet;
            prepared.push(pallet);
        }
        if loose_qty > 0 {
            item.qty = loose_qty;
            item.palletize = false;
            item.stack_height = 1;
            item.piece_count = 1;
            prepared.push(item);
        }
    }
    Ok(prepared)
}

fn box_intersects_space(item: &PlacedItem, space: Space) -> bool {
    item.x < space.x + space.dx - EPS
        && item.x + item.dx > space.x + EPS
        && item.y < space.y + space.dy - EPS
        && item.y + item.dy > space.y + EPS
        && item.z < space.z + space.dz - EPS
        && item.z + item.dz > space.z + EPS
}

fn footprint_overlap_area(x: f64, z: f64, dx: f64, dz: f64, supporter: &PlacedItem) -> f64 {
    let overlap_x = (x + dx).min(supporter.x + supporter.dx) - x.max(supporter.x);
    let overlap_z = (z + dz).min(supporter.z + supporter.dz) - z.max(supporter.z);
    if overlap_x > EPS && overlap_z > EPS {
        overlap_x * overlap_z
    } else {
        0.0
    }
}

/// A raised carton must be completely supported at one common top elevation.
/// This is intentionally stricter than partial-overhang packing: the result is
/// usable as a loading plan rather than merely geometrically non-overlapping.
fn face_area(x: f64, z: f64, dx: f64, dz: f64, px: f64, pz: f64, face: &SupportFace) -> f64 {
    ((x + dx).min(px + face.x + face.dx) - x.max(px + face.x)).max(0.)
        * ((z + dz).min(pz + face.z + face.dz) - z.max(pz + face.z)).max(0.)
}
/// Unit faces are normalized to the canonical (rot0) deck orientation. A yawed
/// placement swaps every normalized face into the placed orientation.
fn orientation_swaps(profile_length: f64, profile_width: f64, o: Orientation) -> bool {
    (o.dx - profile_length).abs() > EPS || (o.dz - profile_width).abs() > EPS
}
fn swap_face(face: &SupportFace, profile_width: f64) -> SupportFace {
    SupportFace {
        x: profile_width - face.z - face.dz,
        z: face.x,
        dx: face.dz,
        dz: face.dx,
    }
}
fn rigid_oriented(profile: &RigidSupport, orientation: Orientation) -> RigidSupport {
    let mut p = profile.clone();
    if orientation_swaps(profile.length, profile.width, orientation) {
        p.faces = p
            .faces
            .into_iter()
            .map(|f| swap_face(&f, profile.width))
            .collect();
        p.base_faces = p
            .base_faces
            .into_iter()
            .map(|f| swap_face(&f, profile.width))
            .collect();
        p.length = orientation.dx;
        p.width = orientation.dz;
    }
    p.load_above = 0.;
    p.parents.clear();
    p
}
fn contact_area(x: f64, y: f64, z: f64, o: Orientation, item: &ItemDef, q: &PlacedItem) -> f64 {
    if (q.y + q.dy - y).abs() > SUPPORT_EPS {
        return 0.;
    }
    match (&item.rigid, &q.rigid) {
        // Load-path contact: the raised unit's member base faces resting on the
        // lower unit's real top faces. Deck overhang contributes no contact.
        (Some(upper), Some(lower)) => {
            let swapped = orientation_swaps(upper.length, upper.width, o);
            upper
                .base_faces
                .iter()
                .map(|f| {
                    let f = if swapped {
                        swap_face(f, upper.width)
                    } else {
                        f.clone()
                    };
                    lower
                        .faces
                        .iter()
                        .map(|lf| face_area(x + f.x, z + f.z, f.dx, f.dz, q.x, q.z, lf))
                        .sum::<f64>()
                })
                .sum()
        }
        (None, None) => footprint_overlap_area(x, z, o.dx, o.dz, q),
        // This extension permits whole-unit / whole-unit stacking only. Loose cargo
        // never gains a fictional support surface from an irregular rigid envelope.
        _ => 0.,
    }
}
/// Every member base face of a raised rigid unit must be completely covered by
/// real top faces at the common placement elevation. Coverage may be pooled
/// from several lower units of the same height; different-height faces and the
/// rigid bounding box never count as support. This is the load-path complete
/// support rule chosen for whole-pallet stacking.
fn base_fully_supported(
    x: f64,
    y: f64,
    z: f64,
    o: Orientation,
    upper: &RigidSupport,
    placed: &[PlacedItem],
) -> bool {
    let swapped = orientation_swaps(upper.length, upper.width, o);
    upper.base_faces.iter().all(|f| {
        let f = if swapped {
            swap_face(f, upper.width)
        } else {
            f.clone()
        };
        let fx = x + f.x;
        let fz = z + f.z;
        let covered: f64 = placed
            .iter()
            .filter(|q| (q.y + q.dy - y).abs() <= SUPPORT_EPS)
            .map(|q| {
                q.rigid.as_ref().map_or(0., |lower| {
                    lower
                        .faces
                        .iter()
                        .map(|lf| face_area(fx, fz, f.dx, f.dz, q.x, q.z, lf))
                        .sum::<f64>()
                })
            })
            .sum();
        covered + EPS >= f.dx * f.dz
    })
}
fn contact_indices(
    x: f64,
    y: f64,
    z: f64,
    o: Orientation,
    item: &ItemDef,
    placed: &[PlacedItem],
) -> Vec<usize> {
    placed
        .iter()
        .enumerate()
        .filter(|(_, q)| contact_area(x, y, z, o, item, q) > EPS)
        .map(|(i, _)| i)
        .collect()
}
fn support_ancestors(roots: &[usize], placed: &[PlacedItem]) -> Vec<usize> {
    let mut seen = vec![false; placed.len()];
    let mut pending = roots.to_vec();
    let mut out = Vec::new();
    while let Some(i) = pending.pop() {
        if i >= placed.len() || seen[i] {
            continue;
        }
        seen[i] = true;
        out.push(i);
        if let Some(p) = &placed[i].rigid {
            pending.extend(p.parents.iter().copied());
        }
    }
    out
}
fn limit_min(a: u32, b: u32) -> u32 {
    if a == 0 {
        b
    } else if b == 0 {
        a
    } else {
        a.min(b)
    }
}
fn support_for(
    x: f64,
    y: f64,
    z: f64,
    o: Orientation,
    item: &ItemDef,
    placed: &[PlacedItem],
) -> Option<SupportInfo> {
    let own = item.stack_height.max(1);
    if y <= EPS {
        if item.max_stack > 0 && own > item.max_stack {
            return None;
        }
        return Some(SupportInfo {
            stack_level: own,
            ceiling: item.max_stack,
        });
    }
    if item.no_understack {
        return None;
    }
    // Non-stackable means no overlapping cargo anywhere above it, not merely
    // 'not in direct contact'. A bridge at a higher elevation is also forbidden.
    if placed.iter().any(|q| {
        q.no_overstack
            && y >= q.y + q.dy - SUPPORT_EPS
            && footprint_overlap_area(x, z, o.dx, o.dz, q) > EPS
    }) {
        return None;
    }
    let mut area = 0.;
    let mut top = 0;
    let mut ceiling = item.max_stack;
    let mut parents = Vec::new();
    for (i, q) in placed.iter().enumerate() {
        let a = contact_area(x, y, z, o, item, q);
        if a <= EPS {
            continue;
        }
        if q.no_overstack {
            return None;
        }
        area += a;
        top = top.max(q.stack_level);
        ceiling = limit_min(
            ceiling,
            if q.stack_ceiling > 0 {
                q.stack_ceiling
            } else {
                q.max_stack
            },
        );
        parents.push(i);
    }
    if parents.is_empty() {
        return None;
    }
    match &item.rigid {
        // Raised unit: every member base must be fully covered at this elevation.
        Some(upper) => {
            if !base_fully_supported(x, y, z, o, upper, placed) {
                return None;
            }
        }
        // Raised carton: completely supported at one common top elevation.
        None => {
            if area + EPS < o.dx * o.dz {
                return None;
            }
        }
    }
    let level = top.saturating_add(own);
    if ceiling > 0 && level > ceiling {
        return None;
    }
    if item.rigid.is_some() {
        for i in support_ancestors(&parents, placed) {
            if let Some(p) = &placed[i].rigid {
                if p.capacity
                    .is_some_and(|cap| p.cargo_weight + p.load_above + item.weight > cap + EPS)
                {
                    return None;
                }
            }
        }
    }
    Some(SupportInfo {
        stack_level: level,
        ceiling,
    })
}

fn fits_at(space: Space, x: f64, y: f64, z: f64, orientation: Orientation) -> bool {
    x >= space.x - EPS
        && y >= space.y - EPS
        && z >= space.z - EPS
        && x + orientation.dx <= space.x + space.dx + EPS
        && y + orientation.dy <= space.y + space.dy + EPS
        && z + orientation.dz <= space.z + space.dz + EPS
}

fn candidate_at(
    item_index: usize,
    item: &ItemDef,
    orientation: Orientation,
    x: f64,
    y: f64,
    z: f64,
    spaces: &[Space],
    placed: &[PlacedItem],
    container: &ContainerDef,
    ranks: &[usize],
    load_ranks: &[usize],
    continuation_key: Option<&str>,
) -> Option<Candidate> {
    let space_index = spaces
        .iter()
        .position(|space| fits_at(*space, x, y, z, orientation))?;
    let support = support_for(x, y, z, orientation, item, placed)?;
    let space = spaces[space_index];
    let rx = (space.x + space.dx - (x + orientation.dx)).max(0.0);
    let ry = (space.y + space.dy - (y + orientation.dy)).max(0.0);
    let rz = (space.z + space.dz - (z + orientation.dz)).max(0.0);
    Some(Candidate {
        item_index,
        x,
        y,
        z,
        priority: item.priority,
        preference_rank: load_ranks[item_index],
        continuation_rank: continuation_key.map_or(1, |key| u8::from(cargo_key(item) != key)),
        orientation,
        support,
        type_rank: ranks[item_index],
        slack_axes: u8::from(rx > EPS) + u8::from(ry > EPS) + u8::from(rz > EPS),
        free_volume: space_volume(space) - orientation.dx * orientation.dy * orientation.dz,
        largest_remainder: rx.max(ry).max(rz),
        grouping_score: grouping_score(x, y, z, orientation, item, placed, container),
    })
}

fn intervals_overlap(a_start: f64, a_size: f64, b_start: f64, b_size: f64) -> bool {
    a_start < b_start + b_size - EPS && b_start < a_start + a_size - EPS
}

fn interval_gap(a_start: f64, a_size: f64, b_start: f64, b_size: f64) -> f64 {
    if intervals_overlap(a_start, a_size, b_start, b_size) {
        0.0
    } else if a_start + a_size <= b_start {
        b_start - (a_start + a_size)
    } else {
        a_start - (b_start + b_size)
    }
}

fn cargo_key(item: &ItemDef) -> &str {
    item.source_name.as_deref().unwrap_or(&item.name)
}

fn same_product(item: &ItemDef, placed: &PlacedItem) -> bool {
    cargo_key(item) == placed.cargo_name.as_str()
}

/// Lower is better. A touching or directly stacked unit scores zero; a new
/// group or a distant same-SKU unit scores one. The radius keeps grouping a
/// soft preference rather than allowing it to override the loading direction.
fn grouping_score(
    x: f64,
    y: f64,
    z: f64,
    orientation: Orientation,
    item: &ItemDef,
    placed: &[PlacedItem],
    container: &ContainerDef,
) -> f64 {
    let radius = container.length.max(container.width).max(container.height) * 0.25;
    let mut best: f64 = 1.0;
    for other in placed.iter().filter(|other| same_product(item, other)) {
        let gap = (interval_gap(x, orientation.dx, other.x, other.dx).powi(2)
            + interval_gap(y, orientation.dy, other.y, other.dy).powi(2)
            + interval_gap(z, orientation.dz, other.z, other.dz).powi(2))
        .sqrt();
        best = best.min((gap / radius.max(EPS)).min(1.0));
    }
    best
}

fn candidate_before(candidate: &Candidate, best: &Candidate, strategy: &str) -> bool {
    // A user-specified cargo priority takes precedence over the normal
    // geometric ranking. Items with the same priority still use the full
    // existing placement score below.
    if candidate.priority != best.priority {
        return candidate.priority > best.priority;
    }
    // In wall mode, an already-supported elevated EMS is a deliberate stack
    // opportunity. Prefer it over a new floor position farther inside so a
    // small carton that fits on top is not unnecessarily sent to the rear.
    if strategy != "floor" {
        let candidate_is_top_space = candidate.y > EPS;
        let best_is_top_space = best.y > EPS;
        if candidate_is_top_space != best_is_top_space {
            return candidate_is_top_space;
        }
        // Top surfaces are filled by the selected size preference before
        // their geometric anchor is compared. This keeps a large carton from
        // being displaced by a smaller one merely because the small anchor is
        // closer to the front or sits in the middle of the same top surface.
        if candidate_is_top_space && candidate.priority == 0 && best.priority == 0 {
            if candidate.preference_rank != best.preference_rank {
                return candidate.preference_rank < best.preference_rank;
            }
            if candidate.continuation_rank != best.continuation_rank {
                return candidate.continuation_rank < best.continuation_rank;
            }
        }
    }
    // Respect the selected loading direction before cargo-size preference.
    // Floor loading completes the lowest layer before moving up. Wall loading
    // keeps the current in/out frontier, then takes a usable top space before
    // spreading sideways, so stackable height is not lost to flat placement.
    if strategy == "floor" {
        for (left, right) in [
            (candidate.y, best.y),
            (candidate.x, best.x),
            (candidate.z, best.z),
        ] {
            if left < right - EPS {
                return true;
            }
            if left > right + EPS {
                return false;
            }
        }
    } else {
        if candidate.x < best.x - EPS {
            return true;
        }
        if candidate.x > best.x + EPS {
            return false;
        }
        if candidate.y > best.y + EPS {
            return true;
        }
        if candidate.y < best.y - EPS {
            return false;
        }
        if candidate.z < best.z - EPS {
            return true;
        }
        if candidate.z > best.z + EPS {
            return false;
        }
    }

    // Once the spatial frontier is tied, keep the ongoing SKU together and
    // then apply the chosen large-cargo / small-cargo default preference.
    if candidate.priority == 0 && best.priority == 0 {
        if candidate.continuation_rank != best.continuation_rank {
            return candidate.continuation_rank < best.continuation_rank;
        }
        if candidate.preference_rank != best.preference_rank {
            return candidate.preference_rank < best.preference_rank;
        }
    }

    // Keep an SKU beside its existing group before opening another group at
    // the same spatial frontier.
    if candidate.grouping_score < best.grouping_score - EPS {
        return true;
    }
    if candidate.grouping_score > best.grouping_score + EPS {
        return false;
    }

    // Prefer a placement that exactly consumes EMS dimensions. It prevents
    // sliver spaces and naturally closes rectangular rows and layers.
    if candidate.slack_axes != best.slack_axes {
        return candidate.slack_axes < best.slack_axes;
    }
    if candidate.free_volume < best.free_volume - EPS {
        return true;
    }
    if candidate.free_volume > best.free_volume + EPS {
        return false;
    }
    if candidate.largest_remainder < best.largest_remainder - EPS {
        return true;
    }
    if candidate.largest_remainder > best.largest_remainder + EPS {
        return false;
    }
    if candidate.type_rank != best.type_rank {
        return candidate.type_rank < best.type_rank;
    }
    candidate.item_index < best.item_index
}

fn rank_items(items: &[ItemDef], variant: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&left, &right| {
        let a = &items[left];
        let b = &items[right];
        let by = match variant % 6 {
            0 => item_volume(b).partial_cmp(&item_volume(a)),
            1 => (b.l * b.w).partial_cmp(&(a.l * a.w)),
            2 => b.h.partial_cmp(&a.h),
            3 => b.l.max(b.w).max(b.h).partial_cmp(&a.l.max(a.w).max(a.h)),
            4 => {
                let a_density = item_volume(a) / a.weight.max(EPS);
                let b_density = item_volume(b) / b.weight.max(EPS);
                b_density.partial_cmp(&a_density)
            }
            _ => item_volume(a).partial_cmp(&item_volume(b)),
        }
        .unwrap_or(Ordering::Equal);
        by.then_with(|| left.cmp(&right))
    });
    let mut ranks = vec![0; items.len()];
    for (rank, index) in order.into_iter().enumerate() {
        ranks[index] = rank;
    }
    ranks
}

/// Stable size preference used only when both candidates have no explicit
/// cargo priority. Variants still provide geometric diversity after this
/// user-facing preference has been respected.
fn preference_ranks(items: &[ItemDef], loading_preference: &str) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&left, &right| {
        let a = item_volume(&items[left]);
        let b = item_volume(&items[right]);
        let by_volume = if loading_preference == "small" {
            a.partial_cmp(&b)
        } else {
            b.partial_cmp(&a)
        }
        .unwrap_or(Ordering::Equal);
        by_volume.then_with(|| left.cmp(&right))
    });
    let mut ranks = vec![0; items.len()];
    for (rank, index) in order.into_iter().enumerate() {
        ranks[index] = rank;
    }
    ranks
}

fn space_is_valid(space: Space) -> bool {
    finite_positive(space.dx) && finite_positive(space.dy) && finite_positive(space.dz)
}

fn contains_space(outer: Space, inner: Space) -> bool {
    inner.x >= outer.x - EPS
        && inner.y >= outer.y - EPS
        && inner.z >= outer.z - EPS
        && inner.x + inner.dx <= outer.x + outer.dx + EPS
        && inner.y + inner.dy <= outer.y + outer.dy + EPS
        && inner.z + inner.dz <= outer.z + outer.dz + EPS
}

fn residual_spaces(space: Space, item: &PlacedItem, target: &mut Vec<Space>) {
    let sx2 = space.x + space.dx;
    let sy2 = space.y + space.dy;
    let sz2 = space.z + space.dz;
    let ix1 = item.x.max(space.x);
    let iy1 = item.y.max(space.y);
    let iz1 = item.z.max(space.z);
    let ix2 = (item.x + item.dx).min(sx2);
    let iy2 = (item.y + item.dy).min(sy2);
    let iz2 = (item.z + item.dz).min(sz2);

    // Six overlapping projections are the conventional EMS subtraction. Each
    // is empty, and the later containment pass retains only maximal spaces.
    target.push(Space {
        x: space.x,
        y: space.y,
        z: space.z,
        dx: ix1 - space.x,
        dy: space.dy,
        dz: space.dz,
    });
    target.push(Space {
        x: ix2,
        y: space.y,
        z: space.z,
        dx: sx2 - ix2,
        dy: space.dy,
        dz: space.dz,
    });
    target.push(Space {
        x: space.x,
        y: space.y,
        z: space.z,
        dx: space.dx,
        dy: iy1 - space.y,
        dz: space.dz,
    });
    target.push(Space {
        x: space.x,
        y: iy2,
        z: space.z,
        dx: space.dx,
        dy: sy2 - iy2,
        dz: space.dz,
    });
    target.push(Space {
        x: space.x,
        y: space.y,
        z: space.z,
        dx: space.dx,
        dy: space.dy,
        dz: iz1 - space.z,
    });
    target.push(Space {
        x: space.x,
        y: space.y,
        z: iz2,
        dx: space.dx,
        dy: space.dy,
        dz: sz2 - iz2,
    });
}

fn update_empty_spaces(spaces: &mut Vec<Space>, item: &PlacedItem) {
    let mut next = Vec::with_capacity(spaces.len() * 3);
    for space in spaces.iter().copied() {
        if box_intersects_space(item, space) {
            residual_spaces(space, item, &mut next);
        } else {
            next.push(space);
        }
    }
    next.retain(|space| space_is_valid(*space));
    next.sort_by(|a, b| {
        space_volume(*b)
            .partial_cmp(&space_volume(*a))
            .unwrap_or(Ordering::Equal)
    });

    let mut maximal = Vec::with_capacity(next.len().min(MAX_EMPTY_SPACES));
    for candidate in next {
        if maximal
            .iter()
            .any(|existing| contains_space(*existing, candidate))
        {
            continue;
        }
        maximal.retain(|existing| !contains_space(candidate, *existing));
        maximal.push(candidate);
    }
    maximal.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.y.partial_cmp(&b.y).unwrap_or(Ordering::Equal))
            .then_with(|| a.z.partial_cmp(&b.z).unwrap_or(Ordering::Equal))
    });
    if maximal.len() > MAX_EMPTY_SPACES {
        maximal.truncate(MAX_EMPTY_SPACES);
    }
    *spaces = maximal;
}

fn pack_one_container(
    container: &ContainerDef,
    input: &[ItemDef],
    strategy: &str,
    variant: usize,
    loading_preference: &str,
    continuation_key: Option<&str>,
) -> PackResult {
    let mut remaining = input.to_vec();
    let ranks = rank_items(&remaining, variant);
    let load_ranks = preference_ranks(&remaining, loading_preference);
    let mut placed = Vec::new();
    let mut spaces = vec![Space {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        dx: container.length,
        dy: container.height,
        dz: container.width,
    }];
    let mut total_weight = 0.0;

    loop {
        let mut best: Option<Candidate> = None;
        for (item_index, item) in remaining.iter().enumerate() {
            if item.qty == 0 || total_weight + item.weight > container.max_weight + EPS {
                continue;
            }
            for orientation in orientations(item) {
                // Conventional EMS anchor: the lower-front corner of each
                // maximal empty space.
                for space in spaces.iter().copied() {
                    let Some(candidate) = candidate_at(
                        item_index,
                        item,
                        orientation,
                        space.x,
                        space.y,
                        space.z,
                        &spaces,
                        &placed,
                        container,
                        &ranks,
                        &load_ranks,
                        continuation_key,
                    ) else {
                        continue;
                    };
                    if best.as_ref().map_or(true, |current| {
                        candidate_before(&candidate, current, strategy)
                    }) {
                        best = Some(candidate);
                    }
                }

                // An EMS can span over several different top faces. Its
                // origin is not always a usable top position, so also try the
                // four aligned corners of recent supporting top surfaces.
                if strategy != "floor" {
                    for supporter in placed.iter().rev().take(MAX_TOP_SURFACE_ANCHORS) {
                        let top = supporter.y + supporter.dy;
                        let mut anchors = vec![
                            (supporter.x, top, supporter.z),
                            (
                                supporter.x + supporter.dx - orientation.dx,
                                top,
                                supporter.z,
                            ),
                            (
                                supporter.x,
                                top,
                                supporter.z + supporter.dz - orientation.dz,
                            ),
                            (
                                supporter.x + supporter.dx - orientation.dx,
                                top,
                                supporter.z + supporter.dz - orientation.dz,
                            ),
                        ];
                        if let Some(profile) = &supporter.rigid {
                            for face in &profile.faces {
                                let x = supporter.x + face.x;
                                let z = supporter.z + face.z;
                                anchors.extend([
                                    (x, top, z),
                                    (x + face.dx - orientation.dx, top, z),
                                    (x, top, z + face.dz - orientation.dz),
                                    (
                                        x + face.dx - orientation.dx,
                                        top,
                                        z + face.dz - orientation.dz,
                                    ),
                                ]);
                            }
                        }
                        for (x, y, z) in anchors {
                            if x < -EPS || y < -EPS || z < -EPS {
                                continue;
                            }
                            let Some(candidate) = candidate_at(
                                item_index,
                                item,
                                orientation,
                                x,
                                y,
                                z,
                                &spaces,
                                &placed,
                                container,
                                &ranks,
                                &load_ranks,
                                continuation_key,
                            ) else {
                                continue;
                            };
                            if best.as_ref().map_or(true, |current| {
                                candidate_before(&candidate, current, strategy)
                            }) {
                                best = Some(candidate);
                            }
                        }
                    }
                }
            }
        }

        let Some(candidate) = best else {
            break;
        };
        let item = &mut remaining[candidate.item_index];
        item.qty -= 1;
        total_weight += item.weight;
        let parent_indices = contact_indices(
            candidate.x,
            candidate.y,
            candidate.z,
            candidate.orientation,
            item,
            &placed,
        );
        if item.rigid.is_some() {
            for i in support_ancestors(&parent_indices, &placed) {
                if let Some(p) = &mut placed[i].rigid {
                    p.load_above += item.weight;
                }
            }
        }
        let rigid = item.rigid.as_ref().map(|p| {
            let mut profile = rigid_oriented(p, candidate.orientation);
            profile.parents = parent_indices;
            profile
        });
        let newly_placed = PlacedItem {
            rigid,
            stack_ceiling: candidate.support.ceiling,
            name: item.name.clone(),
            cargo_name: item
                .source_name
                .clone()
                .unwrap_or_else(|| item.name.clone()),
            color: item.color.clone(),
            weight: item.weight,
            x: candidate.x,
            y: candidate.y,
            z: candidate.z,
            dx: candidate.orientation.dx,
            dy: candidate.orientation.dy,
            dz: candidate.orientation.dz,
            no_overstack: item.no_overstack,
            stack_level: candidate.support.stack_level,
            max_stack: item.max_stack,
            piece_count: item.piece_count,
            palletized: item.piece_count > 1,
            pallet_columns: if item.piece_count > 1 {
                item.pallet_columns
            } else {
                1
            },
            pallet_layers: if item.piece_count > 1 {
                item.stack_height
            } else {
                1
            },
        };
        update_empty_spaces(&mut spaces, &newly_placed);
        placed.push(newly_placed);
    }

    let volume = container_volume(container);
    let used_volume: f64 = placed.iter().map(placed_volume).sum();
    let used_length = placed
        .iter()
        .map(|item| item.x + item.dx)
        .fold(0.0, f64::max);
    PackResult {
        container_name: container.label.clone(),
        container_width: container.width,
        container_height: container.height,
        container_length: container.length,
        volume_rate: if volume > EPS {
            used_volume / volume * 100.0
        } else {
            0.0
        },
        weight_rate: if container.max_weight > EPS {
            total_weight / container.max_weight * 100.0
        } else {
            0.0
        },
        placed_count: placed.iter().map(|item| item.piece_count as usize).sum(),
        total_weight,
        used_length,
        tail_space: (container.length - used_length).max(0.0),
        placed,
        unplaced: remaining.into_iter().filter(|item| item.qty > 0).collect(),
    }
}

fn result_volume(result: &PackResult) -> f64 {
    result.placed.iter().map(placed_volume).sum()
}

fn result_is_better(candidate: &PackResult, current: &PackResult, priority: &str) -> bool {
    let candidate_complete = candidate.unplaced.is_empty();
    let current_complete = current.unplaced.is_empty();
    let candidate_capacity =
        candidate.container_length * candidate.container_width * candidate.container_height;
    let current_capacity =
        current.container_length * current.container_width * current.container_height;
    if candidate_complete != current_complete {
        return candidate_complete;
    }
    if candidate_complete {
        if (candidate_capacity - current_capacity).abs() > EPS {
            return if priority == "small" {
                candidate_capacity < current_capacity
            } else {
                candidate_capacity > current_capacity
            };
        }
    }
    if priority == "small" {
        if (candidate.volume_rate - current.volume_rate).abs() > EPS {
            return candidate.volume_rate > current.volume_rate;
        }
        if (candidate_capacity - current_capacity).abs() > EPS {
            return candidate_capacity < current_capacity;
        }
    } else {
        let candidate_volume = result_volume(candidate);
        let current_volume = result_volume(current);
        if (candidate_volume - current_volume).abs() > EPS {
            return candidate_volume > current_volume;
        }
        if (candidate_capacity - current_capacity).abs() > EPS {
            return candidate_capacity > current_capacity;
        }
    }
    candidate.placed_count > current.placed_count
}

fn plan_score(results: &[PackResult], remaining: &[ItemDef]) -> PlanScore {
    PlanScore {
        packed_volume: results.iter().map(result_volume).sum(),
        unplaced_pieces: remaining
            .iter()
            .map(|item| item.qty.saturating_mul(item.piece_count))
            .sum(),
        total_container_volume: results
            .iter()
            .map(|result| {
                result.container_length * result.container_width * result.container_height
            })
            .sum(),
        container_count: results.len(),
    }
}

fn plan_is_better(candidate: PlanScore, current: PlanScore, priority: &str) -> bool {
    if (candidate.packed_volume - current.packed_volume).abs() > EPS {
        return candidate.packed_volume > current.packed_volume;
    }
    if candidate.unplaced_pieces != current.unplaced_pieces {
        return candidate.unplaced_pieces < current.unplaced_pieces;
    }
    if (candidate.total_container_volume - current.total_container_volume).abs() > EPS {
        return if priority == "small" {
            candidate.total_container_volume < current.total_container_volume
        } else {
            candidate.total_container_volume > current.total_container_volume
        };
    }
    candidate.container_count < current.container_count
}

/// Select the SKU that should lead the next container. It is deliberately a
/// soft continuation: if it cannot fit that container, normal candidates win
/// and the remaining space is still usable by other cargo.
fn next_continuation_key(selected: &PackResult, remaining: &[ItemDef]) -> Option<String> {
    let mut scores: Vec<(String, u32, f64)> = Vec::new();
    for placed in &selected.placed {
        if !remaining
            .iter()
            .any(|item| item.qty > 0 && cargo_key(item) == placed.cargo_name.as_str())
        {
            continue;
        }
        if let Some(score) = scores.iter_mut().find(|score| score.0 == placed.cargo_name) {
            score.1 = score.1.saturating_add(placed.piece_count);
            score.2 += placed_volume(placed);
        } else {
            scores.push((
                placed.cargo_name.clone(),
                placed.piece_count,
                placed_volume(placed),
            ));
        }
    }
    scores
        .into_iter()
        .max_by(|left, right| {
            left.1
                .cmp(&right.1)
                .then_with(|| left.2.partial_cmp(&right.2).unwrap_or(Ordering::Equal))
                .then_with(|| right.0.cmp(&left.0))
        })
        .map(|score| score.0)
}

fn solve(
    templates: Vec<ContainerDef>,
    items: Vec<ItemDef>,
    priority: &str,
    strategy: &str,
    loading_preference: &str,
) -> String {
    if templates.is_empty() {
        return serde_json::json!({ "error": "No container template was supplied." }).to_string();
    }
    if templates
        .iter()
        .any(|container| !container_is_valid(container))
    {
        return serde_json::json!({ "error": "A container has invalid dimensions or weight capacity." }).to_string();
    }
    let total_units: usize = items.iter().map(|item| item.qty as usize).sum();
    let iterations = match total_units {
        0..=30 => 18,
        31..=150 => 12,
        151..=500 => 8,
        _ => 5,
    };
    let mut best_plan: Option<(Vec<PackResult>, Vec<ItemDef>, PlanScore)> = None;

    for variant in 0..iterations {
        let mut remaining = items.clone();
        let mut results = Vec::new();
        let mut continuation_key: Option<String> = None;
        // Every successful container removes at least one solver unit.
        let mut remaining_attempts = total_units.max(1);
        while remaining.iter().any(|item| item.qty > 0) && remaining_attempts > 0 {
            remaining_attempts -= 1;
            let mut best: Option<PackResult> = None;
            for template in &templates {
                let candidate = pack_one_container(
                    template,
                    &remaining,
                    strategy,
                    variant + results.len(),
                    loading_preference,
                    continuation_key.as_deref(),
                );
                if candidate.placed.is_empty() {
                    continue;
                }
                if best.as_ref().map_or(true, |current| {
                    result_is_better(&candidate, current, priority)
                }) {
                    best = Some(candidate);
                }
            }
            let Some(mut selected) = best else {
                break;
            };
            continuation_key = next_continuation_key(&selected, &selected.unplaced);
            remaining = selected.unplaced.clone();
            selected.unplaced.clear();
            results.push(selected);
        }
        let score = plan_score(&results, &remaining);
        if best_plan.as_ref().map_or(true, |(_, _, current)| {
            plan_is_better(score, *current, priority)
        }) {
            best_plan = Some((results, remaining, score));
        }
    }

    let (results, unplaced, _) = best_plan.unwrap_or_else(|| {
        let score = plan_score(&[], &items);
        (Vec::new(), items, score)
    });
    let mut global_unplaced_agg: HashMap<String, u32> = HashMap::new();
    for item in unplaced {
        let key = item.source_name.unwrap_or(item.name);
        let entry = global_unplaced_agg.entry(key).or_default();
        *entry = entry.saturating_add(item.qty.saturating_mul(item.piece_count));
    }
    serde_json::json!({ "results": results, "globalUnplacedAgg": global_unplaced_agg }).to_string()
}

#[wasm_bindgen]
pub fn run_pack_engine(templates_json: &str, items_json: &str, strategy: &str) -> String {
    run_pack_engine_v2(templates_json, items_json, "large", strategy)
}

fn run_pack_engine_impl(
    templates_json: &str,
    items_json: &str,
    priority: &str,
    strategy: &str,
    loading_preference: &str,
) -> String {
    let templates = match serde_json::from_str::<Vec<ContainerDef>>(templates_json) {
        Ok(value) => value,
        Err(error) => {
            return serde_json::json!({ "error": format!("Invalid containers: {error}") })
                .to_string();
        }
    };
    let items = match serde_json::from_str::<Vec<ItemDef>>(items_json) {
        Ok(value) => value,
        Err(error) => {
            return serde_json::json!({ "error": format!("Invalid items: {error}") }).to_string();
        }
    };
    let prepared = match prepare_items(items) {
        Ok(value) => value,
        Err(error) => return serde_json::json!({ "error": error }).to_string(),
    };
    solve(templates, prepared, priority, strategy, loading_preference)
}

#[wasm_bindgen]
pub fn run_pack_engine_v2(
    templates_json: &str,
    items_json: &str,
    priority: &str,
    strategy: &str,
) -> String {
    run_pack_engine_impl(templates_json, items_json, priority, strategy, "large")
}

#[wasm_bindgen]
pub fn run_pack_engine_v3(
    templates_json: &str,
    items_json: &str,
    priority: &str,
    strategy: &str,
    loading_preference: &str,
) -> String {
    run_pack_engine_impl(
        templates_json,
        items_json,
        priority,
        strategy,
        loading_preference,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(length: f64, width: f64, height: f64) -> ContainerDef {
        ContainerDef {
            key: "test".into(),
            label: "Test".into(),
            length,
            width,
            height,
            max_weight: 10_000.0,
        }
    }

    fn item(name: &str, l: f64, w: f64, h: f64, qty: u32) -> ItemDef {
        ItemDef {
            name: name.into(),
            color: "#000000".into(),
            l,
            w,
            h,
            weight: 1.0,
            qty,
            priority: 0,
            rot0: true,
            rot1: false,
            rot2: false,
            rot3: false,
            rot4: false,
            rot5: false,
            max_stack: 0,
            no_overstack: false,
            no_understack: false,
            palletize: false,
            pallet_columns: 1,
            pallet_layers: 1,
            stack_height: 1,
            piece_count: 1,
            source_name: None,
            rigid: None,
        }
    }

    fn intersects(a: &PlacedItem, b: &PlacedItem) -> bool {
        a.x < b.x + b.dx - EPS
            && a.x + a.dx > b.x + EPS
            && a.y < b.y + b.dy - EPS
            && a.y + a.dy > b.y + EPS
            && a.z < b.z + b.dz - EPS
            && a.z + a.dz > b.z + EPS
    }

    #[test]
    fn fills_a_rectangular_floor_without_overlap() {
        let result = pack_one_container(
            &container(2.0, 2.0, 1.0),
            &[item("cube", 1.0, 1.0, 1.0, 4)],
            "floor",
            0,
            "large",
            None,
        );
        assert_eq!(result.placed_count, 4);
        assert!(result.unplaced.is_empty());
        assert!(result.placed.iter().all(|placed| placed.y.abs() < EPS));
        for (index, a) in result.placed.iter().enumerate() {
            for b in result.placed.iter().skip(index + 1) {
                assert!(!intersects(a, b));
            }
        }
        assert!((result.volume_rate - 100.0).abs() < EPS);
    }

    #[test]
    fn does_not_stack_on_a_non_stackable_carton() {
        let mut carton = item("top-clear", 1.0, 1.0, 1.0, 2);
        carton.no_overstack = true;
        let result = pack_one_container(
            &container(1.0, 1.0, 2.0),
            &[carton],
            "wall",
            0,
            "large",
            None,
        );
        assert_eq!(result.placed_count, 1);
        assert_eq!(result.unplaced[0].qty, 1);
    }

    #[test]
    fn must_be_on_the_floor_is_enforced() {
        let base = item("base", 1.0, 1.0, 1.0, 1);
        let mut floor_only = item("floor-only", 1.0, 1.0, 1.0, 1);
        floor_only.no_understack = true;
        // The ordering places the larger-name tie winner by input order, then
        // the floor-only carton cannot take the only elevated space.
        let result = pack_one_container(
            &container(1.0, 1.0, 2.0),
            &[base, floor_only],
            "wall",
            0,
            "large",
            None,
        );
        assert_eq!(result.placed_count, 1);
        assert_eq!(
            result
                .unplaced
                .iter()
                .find(|item| item.name == "floor-only")
                .unwrap()
                .qty,
            1
        );
    }

    #[test]
    fn stack_limit_applies_to_the_whole_column() {
        let mut carton = item("two-level", 1.0, 1.0, 1.0, 3);
        carton.max_stack = 2;
        let result = pack_one_container(
            &container(1.0, 1.0, 3.0),
            &[carton],
            "wall",
            0,
            "large",
            None,
        );
        assert_eq!(result.placed_count, 2);
        assert_eq!(result.unplaced[0].qty, 1);
    }

    #[test]
    fn maximum_container_weight_is_never_exceeded() {
        let mut heavy = item("heavy", 1.0, 1.0, 1.0, 2);
        heavy.weight = 6.0;
        let mut limited = container(2.0, 1.0, 1.0);
        limited.max_weight = 10.0;
        let result = pack_one_container(&limited, &[heavy], "wall", 0, "large", None);
        assert_eq!(result.placed_count, 1);
        assert!((result.total_weight - 6.0).abs() < EPS);
        assert_eq!(result.unplaced[0].qty, 1);
    }

    #[test]
    fn higher_cargo_priority_is_loaded_before_normal_ranking() {
        let normal = item("normal", 2.0, 1.0, 1.0, 1);
        let mut urgent = item("urgent", 1.0, 1.0, 1.0, 1);
        urgent.priority = 10;

        let result = pack_one_container(
            &container(3.0, 1.0, 1.0),
            &[normal, urgent],
            "wall",
            0,
            "large",
            None,
        );
        assert_eq!(result.placed.len(), 2);
        assert_eq!(result.placed[0].name, "urgent");
    }

    #[test]
    fn loading_preference_orders_unprioritized_cargo_by_volume() {
        let small = item("small", 1.0, 1.0, 1.0, 1);
        let large = item("large", 2.0, 1.0, 1.0, 1);
        let test_container = container(3.0, 1.0, 1.0);

        let large_first = pack_one_container(
            &test_container,
            &[small.clone(), large.clone()],
            "wall",
            0,
            "large",
            None,
        );
        let small_first =
            pack_one_container(&test_container, &[small, large], "wall", 0, "small", None);

        assert_eq!(large_first.placed[0].name, "large");
        assert_eq!(small_first.placed[0].name, "small");
    }

    #[test]
    fn wall_strategy_uses_available_top_space_before_side_space() {
        let spaces = vec![
            Space {
                x: 1.0,
                y: 0.0,
                z: 0.0,
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            },
            Space {
                x: 4.0,
                y: 1.0,
                z: 0.0,
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            },
        ];
        let candidate = |space_index: usize| Candidate {
            item_index: 0,
            x: spaces[space_index].x,
            y: spaces[space_index].y,
            z: spaces[space_index].z,
            priority: 0,
            preference_rank: 0,
            continuation_rank: 1,
            orientation: Orientation {
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            },
            support: SupportInfo {
                stack_level: 1,
                ceiling: 0,
            },
            type_rank: 0,
            slack_axes: 0,
            free_volume: 0.0,
            largest_remainder: 0.0,
            grouping_score: 0.0,
        };

        // Wall mode stacks into the valid elevated space even though its
        // in/out coordinate is farther back than the floor space.
        assert!(candidate_before(&candidate(1), &candidate(0), "wall"));
        // Floor mode deliberately keeps its current layer flat first.
        assert!(candidate_before(&candidate(0), &candidate(1), "floor"));
    }

    #[test]
    fn wall_strategy_applies_size_preference_before_top_anchor_position() {
        let top_candidate = |x, preference_rank| Candidate {
            item_index: preference_rank,
            x,
            y: 1.0,
            z: 0.0,
            priority: 0,
            preference_rank,
            continuation_rank: 1,
            orientation: Orientation {
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            },
            support: SupportInfo {
                stack_level: 2,
                ceiling: 0,
            },
            type_rank: preference_rank,
            slack_axes: 0,
            free_volume: 0.0,
            largest_remainder: 0.0,
            grouping_score: 0.0,
        };

        // With the large-cargo preference, a large carton farther along the
        // same top layer wins over a small carton at the nearer anchor.
        let small_top = top_candidate(0.0, 1);
        let large_top = top_candidate(4.0, 0);
        assert!(candidate_before(&large_top, &small_top, "wall"));

        // The rank is deliberately supplied by preference_ranks(), so the
        // same comparison reverses when the user selects small-cargo first.
        let small_top = top_candidate(0.0, 0);
        let large_top = top_candidate(4.0, 1);
        assert!(candidate_before(&small_top, &large_top, "wall"));
    }

    #[test]
    fn pallet_keeps_piece_accounting() {
        let mut cartons = item("carton", 1.0, 1.0, 1.0, 5);
        cartons.palletize = true;
        cartons.pallet_columns = 2;
        cartons.pallet_layers = 2;
        let prepared = prepare_items(vec![cartons]).unwrap();
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].piece_count, 4);
        assert_eq!(prepared[0].qty, 1);
        assert_eq!(prepared[1].qty, 1);
    }

    #[test]
    fn same_sku_grouping_prefers_an_adjacent_position() {
        let test_container = container(4.0, 1.0, 1.0);
        let sku_a = item("sku-a", 1.0, 1.0, 1.0, 1);
        let placed =
            pack_one_container(&test_container, &[sku_a.clone()], "wall", 0, "large", None).placed;
        let orientation = Orientation {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        };

        let adjacent = grouping_score(1.0, 0.0, 0.0, orientation, &sku_a, &placed, &test_container);
        let distant = grouping_score(3.0, 0.0, 0.0, orientation, &sku_a, &placed, &test_container);
        let other_sku = grouping_score(
            1.0,
            0.0,
            0.0,
            orientation,
            &item("sku-b", 1.0, 1.0, 1.0, 1),
            &placed,
            &test_container,
        );

        assert!(adjacent < distant);
        assert!(adjacent < other_sku);
        assert!((adjacent - 0.0).abs() < EPS);
    }
}

mod pallet_v4;
pub use pallet_v4::{arrange_pallet_v4, run_pack_engine_v4, validate_pallet_v4};
