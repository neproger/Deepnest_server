// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `sep` — the overlap-minimization **separation search** (Phase 2b), ported from sparrow's Guided
//! Local Search (MIT) to ironnest's deterministic, fixed-budget, single-worker loop.
//!
//! Greedy construction can place each part only where it locally fits; it cannot *discover*
//! arrangements where parts must interlock. This module fixes that: it lets parts overlap, then
//! iteratively shoves them apart under GLS weighting until they separate — the only mechanism that
//! rearranges already-placed parts to make room.
//!
//! Layering (leaf → root): [`proxy`] (the smooth overlap signal) → [`tracker`] (per-pair loss + GLS
//! weights) → [`evaluator`] (CDE-arbitrated candidate scoring) → [`search`] (sample + coordinate
//! descent) → [`separator`] (the strike loop) → this module (the **bin-packing insertion driver**).
//!
//! Driver (sparrow's strip-shrink replaced by fixed-container insertion, doc §4.5): after the
//! constructive `improve()` pass, for each still-unplaced part (largest-first) — snapshot the layout,
//! seed the part at its lowest-overlap pose, run the separator over the *whole* layout (it may move
//! neighbours), and keep the part iff the result is feasible (the CDE is the arbiter); otherwise
//! restore the snapshot. All randomness is the seeded [`Prng`]; budgets are fixed (never a clock).

mod evaluator;
mod proxy;
mod search;
mod separator;
mod tracker;

use crate::SeparationEffort;
use crate::prng::Prng;
use ironnest_cde::entities::{Item, Layout};
use ironnest_geo::Scalar;
use search::SampleConfig;
use separator::SepConfig;
use tracker::CollisionTracker;

/// The fixed per-insertion separation budget (sparrow's separator defaults, single worker, scaled up
/// modestly for irregular density — see `docs/02` §10): 80 container + 40 focused samples, 3
/// coordinate descents, 150 no-improvement iterations, 4 strikes. All integers — never a wall clock.
const SEP_CONFIG: SepConfig = SepConfig {
    sample: SampleConfig {
        n_container_samples: 80,
        n_focussed_samples: 40,
        n_coord_descents: 3,
    },
    iter_no_imprv_limit: 150,
    strike_limit: 4,
};

/// Poses sampled when seeding a new (overlapping) part at its lowest-overlap position. Fixed.
const SEED_SAMPLES: usize = 400;

/// [`SeparationEffort::Fast`] per-insertion budget: ~1/8th the work of [`SEP_CONFIG`]. On an
/// over-subscribed pack most inserts fail regardless of budget, and the full budget's marginal
/// placements are rare — so a much smaller budget trades a handful of parts for a large wall-clock cut
/// (docs/04 Tier 0b). All integers; the reduced counts keep the search deterministic.
const SEP_CONFIG_FAST: SepConfig = SepConfig {
    sample: SampleConfig {
        n_container_samples: 30,
        n_focussed_samples: 15,
        n_coord_descents: 2,
    },
    iter_no_imprv_limit: 40,
    strike_limit: 2,
};

/// [`SeparationEffort::Fast`] seed-sample count (vs [`SEED_SAMPLES`]).
const SEED_SAMPLES_FAST: usize = 100;

/// Tries to insert every still-unplaced instance (largest-first) via overlap-then-separate, updating
/// `placed_per_type`. Runs after the constructive `improve()` pass; a no-op when nothing is unplaced
/// (so the dense rectangular cases skip it entirely).
///
/// `effort` selects the tail's aggressiveness. [`SeparationEffort::Full`] is the shipped, byte-stable
/// behavior (full budget, every unplaced part attempted). [`SeparationEffort::Fast`] adds an
/// area-conservation skip of provably-hopeless inserts and runs a reduced per-insert budget — a
/// deliberately different (re-blessed) layout that is far cheaper on over-subscribed packs (docs/04).
pub fn run_separation(
    layout: &mut Layout,
    entities: &[Option<Item>],
    order: &[usize],
    qty: &[usize],
    prng: &mut Prng,
    placed_per_type: &mut [usize],
    effort: SeparationEffort,
) {
    // `Off` skips the tail entirely: the constructive layout is returned as-is
    // (feasible, deterministic, possibly with unplaced demand). Deepnest Server
    // uses this as a cheap preview pass. (MPL-2.0 modification.)
    if effort == SeparationEffort::Off {
        return;
    }
    let fast = effort == SeparationEffort::Fast;
    let (seed_samples, sep_cfg) = if fast {
        (SEED_SAMPLES_FAST, SEP_CONFIG_FAST)
    } else {
        (SEED_SAMPLES, SEP_CONFIG)
    };

    // Area-conservation feasibility bound (Fast only, docs/04 Tier 0a): placed CD footprints are
    // pairwise disjoint and lie inside the container's CD region, so their areas sum to <= the
    // container CD area; a candidate whose CD area would push that sum past the container CD area
    // cannot be placed collision-free by ANY rearrangement, so the separator is guaranteed to fail.
    // Skipping it is SOUND (never skips a placeable part). Using the raw container CD area (ignoring
    // keep-out zones, which only reduce real capacity) keeps the estimate an over-estimate of free
    // area — the safe direction. `pi.shape.area` is rigid-invariant, so it equals the type's CD area.
    let container_cd_area = layout.container.outer_cd.area;
    let mut placed_cd_area: Scalar = layout.placed_items.values().map(|pi| pi.shape.area).sum();

    for &item_id in order {
        let Some(item) = entities[item_id].as_ref() else {
            continue;
        };
        let part_cd_area = item.shape_cd.area;
        while placed_per_type[item_id] < qty[item_id] {
            if fast && placed_cd_area + part_cd_area > container_cd_area {
                // No room by area conservation — a guaranteed separator failure. Every further copy
                // of this type is at least as large, so stop this type (matches the fail-then-break
                // control flow below).
                break;
            }
            if try_insert(layout, entities, item, prng, seed_samples, sep_cfg) {
                placed_per_type[item_id] += 1;
                placed_cd_area += part_cd_area;
            } else {
                // If one more of this type cannot be made to fit, neither can further copies.
                break;
            }
        }
    }
}

/// Runs the full-layout GLS separator over the CURRENT layout (every placed part movable — it samples
/// the allowed rotation set and shoves neighbours), returning `true` iff it reached zero overlap.
///
/// Unlike [`run_separation`] (which seeds *unplaced* parts and only moves their colliding neighbours),
/// this pressures **already-placed, non-overlapping** parts — which the separator otherwise never
/// touches. It is the primitive the densification compaction (docs/04 Tier D) drives *after* shrinking
/// the container: the shrink makes some placed parts overlap the smaller boundary / each other, and
/// this pulls the whole arrangement inward to a tighter feasible packing.
pub fn separate_layout(layout: &mut Layout, entities: &[Option<Item>], prng: &mut Prng) -> bool {
    // Densify re-packs the whole layout against a tightening boundary. Called many times per Max
    // nest (1% shrink steps), so per-call cost multiplies hard. Measured (docs/04 §4e A/B):
    // Full `SEP_CONFIG` / mid-tier budgets held remnant but cost 2–8× this Fast budget; Fast
    // held max_x/remnant on the cone+square corpus and goldens. All integers — never a wall clock.
    const DENSIFY_SEP: SepConfig = SEP_CONFIG_FAST;
    let mut tracker = CollisionTracker::new(layout);
    separator::separate(layout, entities, &mut tracker, prng, DENSIFY_SEP)
}

/// Attempts to add one `item`: seed it (allowing overlap), separate the whole layout, and keep it iff
/// the layout becomes feasible. Returns whether the item was kept. `seed_samples`/`sep_cfg` are the
/// effort-selected budgets ([`SEED_SAMPLES`]/[`SEP_CONFIG`] for `Full`).
fn try_insert(
    layout: &mut Layout,
    entities: &[Option<Item>],
    item: &Item,
    prng: &mut Prng,
    seed_samples: usize,
    sep_cfg: SepConfig,
) -> bool {
    // Pose-only: try_insert never swaps the container (docs/04 0c).
    let snapshot = layout.save_poses();

    let Some(seed_dt) = search::lowest_overlap_pose(layout, item, prng, seed_samples) else {
        return false; // the item does not fit the container in any orientation at all
    };
    layout.place_item(item, seed_dt);

    let mut tracker = CollisionTracker::new(layout);
    let feasible = separator::separate(layout, entities, &mut tracker, prng, sep_cfg);

    // `separate`'s boolean is advisory (it reflects the proxy/CDE tracker's `total_loss`); the exact
    // `Layout::is_feasible` is the sole arbiter we keep on. They can disagree only conservatively —
    // a symmetric `PairMatrix` cell may read 0 for a pair the exact CDE still rejects in an fp edge
    // case — which at worst costs a missed placement, never an infeasible accepted one.
    if feasible && layout.is_feasible() {
        true
    } else {
        layout.restore_poses(&snapshot);
        false
    }
}
