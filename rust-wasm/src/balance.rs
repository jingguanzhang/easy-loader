//! Centre-of-gravity and support (axle) load limits.
//!
//! First principles used here:
//! * The cargo centre of gravity is the weight-averaged centre of every
//!   placed unit. Two supports share the weight by the lever rule, so each
//!   support limit is equivalent to a bound on the longitudinal centre of
//!   gravity.
//! * Moving the whole finished load by one common offset keeps every
//!   non-overlap and support relation intact, so it is a free way to move the
//!   centre of gravity as long as the load stays inside the container.
//! * A unit placed later never carries a unit placed earlier, so removing
//!   units from the end of the placement sequence always leaves a valid load.
//!
//! The limits are therefore enforced after a container is packed: shift the
//! load by the smallest amount that satisfies them, and if no shift can,
//! return the last placed units to the remaining cargo until one does.

use super::*;

pub(crate) struct Balance {
    pub weight: f64,
    pub cog: [f64; 3],
}

pub(crate) fn has_limits(container: &ContainerDef) -> bool {
    container.max_cog_offset_x > EPS
        || container.max_cog_offset_z > EPS
        || (has_supports(container)
            && (container.max_front_axle_load > EPS || container.max_rear_axle_load > EPS))
}

fn has_supports(container: &ContainerDef) -> bool {
    container.rear_axle_pos - container.front_axle_pos > EPS
}

fn unit_cog(item: &PlacedItem) -> [f64; 3] {
    match &item.rigid {
        Some(profile) => [
            item.x + profile.cog[0],
            item.y + profile.cog[1],
            item.z + profile.cog[2],
        ],
        None => [
            item.x + item.dx / 2.0,
            item.y + item.dy / 2.0,
            item.z + item.dz / 2.0,
        ],
    }
}

pub(crate) fn measure(placed: &[PlacedItem]) -> Balance {
    let weight: f64 = placed.iter().map(|item| item.weight).sum();
    let mut cog = [0.0; 3];
    if weight > EPS {
        for item in placed {
            let centre = unit_cog(item);
            for axis in 0..3 {
                cog[axis] += item.weight * centre[axis] / weight;
            }
        }
    } else if !placed.is_empty() {
        // Weightless cargo: fall back to the plain average of the centres.
        for item in placed {
            let centre = unit_cog(item);
            for axis in 0..3 {
                cog[axis] += centre[axis] / placed.len() as f64;
            }
        }
    }
    Balance { weight, cog }
}

/// Cargo load on the (front, rear) support for a given longitudinal centre
/// of gravity.
pub(crate) fn support_loads(
    container: &ContainerDef,
    weight: f64,
    cog_x: f64,
) -> Option<(f64, f64)> {
    if !has_supports(container) {
        return None;
    }
    let span = container.rear_axle_pos - container.front_axle_pos;
    let rear = weight * (cog_x - container.front_axle_pos) / span;
    Some((weight - rear, rear))
}

/// Permitted interval for the longitudinal centre of gravity.
fn cog_window_x(container: &ContainerDef, weight: f64) -> (f64, f64) {
    let (mut low, mut high) = (f64::NEG_INFINITY, f64::INFINITY);
    if container.max_cog_offset_x > EPS {
        low = container.length / 2.0 - container.max_cog_offset_x;
        high = container.length / 2.0 + container.max_cog_offset_x;
    }
    if has_supports(container) && weight > EPS {
        let span = container.rear_axle_pos - container.front_axle_pos;
        if container.max_rear_axle_load > EPS {
            high =
                high.min(container.front_axle_pos + container.max_rear_axle_load * span / weight);
        }
        if container.max_front_axle_load > EPS {
            low = low.max(container.rear_axle_pos - container.max_front_axle_load * span / weight);
        }
    }
    (low, high)
}

/// Smallest shift that brings `value` into `[low, high]` while the load,
/// which currently spans `[min, max]`, stays inside `[0, size]`.
fn shift_into(value: f64, low: f64, high: f64, min: f64, max: f64, size: f64) -> Option<f64> {
    if low > high + EPS {
        return None;
    }
    let shift = if value < low {
        low - value
    } else if value > high {
        high - value
    } else {
        0.0
    };
    (shift >= -min - EPS && shift <= size - max + EPS).then_some(shift.clamp(-min, size - max))
}

/// The smallest (x, z) shift of the whole load that satisfies every enabled
/// limit, or `None` when no shift inside the container can.
pub(crate) fn required_shift(
    container: &ContainerDef,
    placed: &[PlacedItem],
) -> Option<(f64, f64)> {
    if placed.is_empty() {
        return Some((0.0, 0.0));
    }
    let balance = measure(placed);
    let extent = |start: fn(&PlacedItem) -> f64, size: fn(&PlacedItem) -> f64| {
        placed
            .iter()
            .fold((f64::INFINITY, 0.0_f64), |(min, max), item| {
                (min.min(start(item)), max.max(start(item) + size(item)))
            })
    };
    let (min_x, max_x) = extent(|item| item.x, |item| item.dx);
    let (min_z, max_z) = extent(|item| item.z, |item| item.dz);
    let (low, high) = cog_window_x(container, balance.weight);
    let shift_x = shift_into(balance.cog[0], low, high, min_x, max_x, container.length)?;
    let shift_z = if container.max_cog_offset_z > EPS {
        shift_into(
            balance.cog[2],
            container.width / 2.0 - container.max_cog_offset_z,
            container.width / 2.0 + container.max_cog_offset_z,
            min_z,
            max_z,
            container.width,
        )?
    } else {
        0.0
    };
    Some((shift_x, shift_z))
}
