// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ironnest-optimizer — OUR deterministic placement search (new code; the brain).
//!
//! Milestone 2a (this module) is a **deterministic constructive nester**: order items by descending
//! size, then for each item sample candidate poses over the container, fail-fast via the CDE
//! surrogate, keep the lowest-[`loss`](crate::loss) feasible pose (a Left-Bottom-Fill preference),
//! and place it. Driven by a **fixed sample budget, never a wall clock**; all randomness comes from
//! the self-contained portable [`Prng`](crate::prng::Prng). Rotations are a caller-supplied discrete
//! set (default `{0,90,180,270}`, configurable per call). Built on [`ironnest_cde`].
//!
//! Milestone 2b (next) adds a sparrow-style separation / overlap-minimization local search on top.
#![warn(
    clippy::pedantic,
    clippy::correctness,
    clippy::suspicious,
    clippy::complexity,
    clippy::perf,
    clippy::style,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]

mod improve;
mod loss;
/// The exact-NFP construction substrate (feature #4, docs/03). Doc-hidden pub: exposed only so
/// `tests/nfp_props.rs` can drive it directly; NOT a stable API (the `multistart_start` precedent).
#[doc(hidden)]
pub mod nfp;
mod prng;
mod search;
mod sep;

pub use ironnest_geo::Scalar;
pub use prng::Prng;

/// Number of improvement rounds run after the constructive fill (each round compacts every placed
/// item, then tries to place any still-unplaced items in the freed space). Fixed — never a wall clock.
const IMPROVE_ROUNDS: u32 = 3;

/// Douglas–Peucker tolerance for collision-footprint decimation, as a fraction of `min_sep`. Every
/// hazard's footprint (items inflate, container/holes deflate/inflate) is offset by `min_sep/2 + tol`
/// instead of `min_sep/2` on the high-vertex curved path (the per-shape vertex gate lives in
/// `ironnest_cde::geometry`'s `DECIMATION_MIN_VERTICES`). So `tol` is the over-reservation margin
/// that (a) compensates DP's bounded inward deviation and (b) comfortably exceeds the offsetter's
/// sub-mil polygonal curve deficit (`tol` is ~100× it) — keeping the placed *original* outlines ≥
/// `min_sep` apart from each other AND from a curved container boundary. `min_sep/16` ≈ 0.023 at the
/// consumer-validated 0.375-in `min_sep` (their validated 0.02-in tol), a negligible density cost
/// relative to the curved-part speedup.
const DECIMATION_TOL_FRACTION: Scalar = 1.0 / 16.0;

use std::cmp::Ordering;

use ironnest_cde::collision_detection::CDEConfig;
use ironnest_cde::entities::{Container, Item, Layout};
use ironnest_cde::geometry::fail_fast::SPSurrogateConfig;
use ironnest_cde::geometry::primitives::Rect;
use ironnest_cde::io::ext_repr::{ExtContainer, ExtItem, ExtQualityZone, ExtSPolygon, ExtShape};
use ironnest_cde::io::import::Importer;

/// A single resolved placement — the only thing the oracle emits.
///
/// The pose maps the item's *original* (caller-supplied) outline into container coordinates:
/// `placed_point = Rot(rotation_deg)·original_point + (x, y)`. No anchor knowledge is required by
/// the consumer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Index into the caller's `items` slice.
    pub item: usize,
    /// Placement origin X, in the container's coordinate space.
    pub x: Scalar,
    /// Placement origin Y, in the container's coordinate space.
    pub y: Scalar,
    /// Rotation in degrees; always a member of the caller's allowed set.
    pub rotation_deg: Scalar,
}

/// The result of a nest: every placed instance, plus the item-type index of every instance that did
/// not fit (one entry per unplaced instance).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NestSolution {
    pub placements: Vec<Placement>,
    pub unplaced: Vec<usize>,
}

/// Keep-best score for multi-start / densify. **Product goal: maximize usable remnant at the end of
/// the sheet.** Lexicographic order (all `total_cmp`, determinism-safe):
///
/// 1. **higher** placed area — never drop a placeable part to "make remnant" (demand first);
/// 2. **lower** `used_x_max` — LBF packs toward the low-x corner, so the free strip lives at high-x
///    (the sheet end). Smaller right-edge ⇒ larger end remnant;
/// 3. **lower** `used_y_max` — secondary packing-down.
///
/// On a full tie the earliest multi-start *k* wins (caller-side). Empty layouts score as zeros.
#[derive(Clone, Copy, Debug)]
struct NestScore {
    placed_area: Scalar,
    used_x_max: Scalar,
    used_y_max: Scalar,
}

impl NestScore {
    fn empty() -> Self {
        Self {
            placed_area: 0.0,
            used_x_max: 0.0,
            used_y_max: 0.0,
        }
    }

    /// `true` iff `self` is a strictly better nest than `other` under the remnant-first order.
    fn better_than(self, other: Self) -> bool {
        match self.placed_area.total_cmp(&other.placed_area) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => match self.used_x_max.total_cmp(&other.used_x_max) {
                Ordering::Less => true,
                Ordering::Greater => false,
                Ordering::Equal => self.used_y_max.total_cmp(&other.used_y_max) == Ordering::Less,
            },
        }
    }

    /// Score a live layout: original-outline `areas` ranked by placement item-id, plus the packing's
    /// high-side AABB edge from CD footprints (the remnant metric).
    fn from_layout(layout: &Layout, areas: &[Scalar]) -> Self {
        let placed_area: Scalar = layout
            .placed_items
            .values()
            .map(|pi| areas.get(pi.item_id).copied().unwrap_or(0.0))
            .sum();
        let ((_, x_max), (_, y_max)) = used_extent(layout);
        if layout.placed_items.is_empty() {
            Self::empty()
        } else {
            Self {
                placed_area,
                used_x_max: x_max,
                used_y_max: y_max,
            }
        }
    }
}

/// One sheet for a multi-sheet nest: a boundary outline plus optional keep-out holes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    /// The sheet boundary outline.
    pub outline: Vec<[Scalar; 2]>,
    /// Keep-out polygons inside the sheet that no part may overlap (see [`nest`]'s `holes`).
    pub holes: Vec<Vec<[Scalar; 2]>>,
}

/// The result of a multi-sheet nest: the placements on each sheet (parallel to the input `sheets`),
/// plus the item-type index of every instance that fit on no sheet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MultiSheetSolution {
    /// `per_sheet[i]` are the placements on `sheets[i]`.
    pub per_sheet: Vec<Vec<Placement>>,
    pub unplaced: Vec<usize>,
}

/// The collision-detection configuration. Mirrors `lbf`'s reference defaults; internal for 2a.
///
/// `quadtree_depth`/`cd_threshold` are acceleration-only (they change neither the collision boolean
/// nor the collected hazard set → byte-identical placements). A wall-clock sweep of depth ∈ {4..8} ×
/// `cd_threshold` ∈ {8,16,32} on sep-heavy NFP packs (perf Tier 1b, docs/04 §4) found these defaults
/// already near-optimal (variation within best-of-2 noise; depth ≥ 7 is slower), so they are kept.
fn default_cde_config() -> CDEConfig {
    CDEConfig {
        quadtree_depth: 5,
        cd_threshold: 16,
        item_surrogate_config: SPSurrogateConfig {
            n_pole_limits: [(100, 0.0), (20, 0.75), (10, 0.90)],
            n_ff_poles: 2,
            n_ff_piers: 0,
        },
    }
}

fn ext_spolygon(outline: &[[Scalar; 2]]) -> ExtSPolygon {
    ExtSPolygon(outline.iter().map(|p| (p[0], p[1])).collect())
}

/// Clips polygon `poly` to the axis-aligned rectangle `r` (Sutherland–Hodgman against the four
/// half-planes). Returns `None` if the result has fewer than 3 vertices (the polygon lay outside
/// `r`). Deterministic — only `+ − × ÷` and comparisons; division is reached only on a segment that
/// straddles an edge (non-parallel), so it never divides by zero. A polygon already inside `r` is
/// returned vertex-for-vertex unchanged.
fn clip_polygon_to_rect(poly: &[[Scalar; 2]], r: Rect) -> Option<Vec<[Scalar; 2]>> {
    // 0=left (x≥x_min), 1=right (x≤x_max), 2=bottom (y≥y_min), 3=top (y≤y_max).
    let inside = |p: [Scalar; 2], edge: u8| -> bool {
        match edge {
            0 => p[0] >= r.x_min,
            1 => p[0] <= r.x_max,
            2 => p[1] >= r.y_min,
            _ => p[1] <= r.y_max,
        }
    };
    let intersect = |a: [Scalar; 2], b: [Scalar; 2], edge: u8| -> [Scalar; 2] {
        match edge {
            0 => {
                let t = (r.x_min - a[0]) / (b[0] - a[0]);
                [r.x_min, a[1] + t * (b[1] - a[1])]
            }
            1 => {
                let t = (r.x_max - a[0]) / (b[0] - a[0]);
                [r.x_max, a[1] + t * (b[1] - a[1])]
            }
            2 => {
                let t = (r.y_min - a[1]) / (b[1] - a[1]);
                [a[0] + t * (b[0] - a[0]), r.y_min]
            }
            _ => {
                let t = (r.y_max - a[1]) / (b[1] - a[1]);
                [a[0] + t * (b[0] - a[0]), r.y_max]
            }
        }
    };

    let mut out = poly.to_vec();
    for edge in 0..4u8 {
        if out.len() < 3 {
            return None;
        }
        let input = std::mem::take(&mut out);
        let n = input.len();
        for i in 0..n {
            let cur = input[i];
            let prev = input[(i + n - 1) % n];
            let (cur_in, prev_in) = (inside(cur, edge), inside(prev, edge));
            if cur_in {
                if !prev_in {
                    out.push(intersect(prev, cur, edge));
                }
                out.push(cur);
            } else if prev_in {
                out.push(intersect(prev, cur, edge));
            }
        }
    }
    (out.len() >= 3).then_some(out)
}

/// Imports the container with `holes` as quality-0 keep-out zones, **clipping each hole to the
/// quadtree root** so an out-of-bounds hole cannot trip the CDE's constrict invariant. Returns `None`
/// if the container — or any in-bounds hole — is malformed (⇒ the caller places nothing). In-bounds
/// holes pass through unchanged (so the determinism golden is untouched); an enclosing hole clips to
/// a full-sheet keep-out (⇒ nothing fits); an entirely-outside hole is dropped (unreachable space).
fn import_container_with_holes(
    importer: &Importer,
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
) -> Option<Container> {
    let bare_ext = ExtContainer {
        id: 0,
        shape: ExtShape::SimplePolygon(ext_spolygon(container)),
        zones: vec![],
    };
    let bare = importer.import_container(&bare_ext).ok()?;
    if holes.is_empty() {
        return Some(bare);
    }

    let root = bare.base_cde.bbox();
    let zones: Vec<ExtQualityZone> = holes
        .iter()
        .filter_map(|hole| {
            let in_bounds = hole.iter().all(|p| {
                p[0] >= root.x_min && p[0] <= root.x_max && p[1] >= root.y_min && p[1] <= root.y_max
            });
            let shape = if in_bounds {
                ext_spolygon(hole)
            } else {
                ext_spolygon(&clip_polygon_to_rect(hole, root)?)
            };
            Some(ExtQualityZone {
                quality: 0,
                shape: ExtShape::SimplePolygon(shape),
            })
        })
        .collect();

    let ext = ExtContainer {
        id: 0,
        shape: ExtShape::SimplePolygon(ext_spolygon(container)),
        zones,
    };
    importer.import_container(&ext).ok()
}

/// Nests `items` (each with a demand `qty`) into a single irregular `container` with optional
/// `holes` (keep-out zones the parts must avoid), applying **one** allowed-rotation set to every item.
///
/// * `items` — one outline per item *type*, in item-local coordinates.
/// * `qty` — demand per item type (`qty.len() == items.len()`).
/// * `container` — the container boundary outline.
/// * `holes` — keep-out polygons inside the container that no part may overlap (interior voids,
///   sheet defects, or — to "nest inside a part" — the solid region of an already-placed part,
///   leaving its void nestable). Empty ⇒ no holes. Imported as quality-0 zones.
/// * `min_sep` — minimum separation between any two parts (and part↔boundary↔hole); `0.0` to disable.
/// * `rotations_deg` — allowed discrete orientations in degrees (e.g. `[0.0, 90.0, 180.0, 270.0]`),
///   applied to **every** item; empty ⇒ no rotation. For a distinct set per item type use
///   [`nest_per_item`].
/// * `seed` — explicit PRNG seed (no implicit/entropy fallback, ever).
/// * `budget` — samples per item placement (fixed; never a wall clock).
///
/// Determinism: the same arguments always produce a byte-identical [`NestSolution`].
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Scalar],
    seed: u64,
    budget: u64,
) -> NestSolution {
    // Broadcast the single rotation set to every item, then run the per-item path. Broadcasting is
    // byte-for-byte identical to threading one global set through the search — every item sees the
    // same orientations and so consumes the same PRNG draws — which is what keeps the existing
    // determinism golden valid through this refactor.
    let rotations_per_item = vec![rotations_deg.to_vec(); items.len()];
    nest_per_item(
        items,
        qty,
        container,
        holes,
        min_sep,
        &rotations_per_item,
        seed,
        budget,
    )
}

/// Like [`nest`], but with a **distinct allowed-rotation set per item type**: `rotations_deg[k]` is
/// the orientation set (degrees) for `items[k]`, so `rotations_deg.len() == items.len()`. An empty
/// inner set ⇒ that part is not rotated (the same `{0}` normalization [`nest`] applies, now per item).
/// Lets different shapes use different orientations in one nest — e.g. rectangles pinned axis-aligned
/// (`[0.0, 90.0]`), triangles free to interlock (`[0, 45, …, 315]`). Same determinism contract as
/// [`nest`]; arbitrary (non-cardinal) angles route through the same `libm` trig and stay byte-stable.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_per_item(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
) -> NestSolution {
    nest_core(
        items,
        qty,
        container,
        holes,
        min_sep,
        rotations_deg,
        seed,
        budget,
        InsertionOrder::Canonical,
    )
}

/// How a nest run orders item types for insertion. Construction, refill, and the separation
/// insertion driver all follow this one order.
#[derive(Clone, Copy, Debug)]
enum InsertionOrder {
    /// Largest-first (descending CD-shape diameter; stable on ties) — the single-start canonical
    /// order. [`nest`] / [`nest_per_item`] always use this, as does multi-start's start `k = 0`.
    Canonical,
    /// Largest-first under seeded multiplicative noise on the sort keys — the multi-start
    /// **order-diversification** lever (the trailing start block). A single greedy order explores only one
    /// basin per PRNG stream; jittering the order lets different starts try genuinely different
    /// insertion sequences (the dimension Deepnest's GA searches). One factor in
    /// `[1 − ORDER_JITTER, 1 + ORDER_JITTER)` is drawn per successfully-imported item type from the
    /// start's own seeded PRNG (ascending-id draw order — fixed; import success is itself a pure
    /// function of the input), so the order is a pure function of `(seed, inputs)`.
    Jittered,
}

/// Multiplicative half-width of the [`InsertionOrder::Jittered`] sort-key noise. `0.3` lets a part
/// up to ~2× smaller in diameter occasionally insert first, which is enough to reorder heterogeneous
/// mixes without degenerating into a random shuffle (largest-first remains the strong prior).
const ORDER_JITTER: Scalar = 0.3;

/// The insertion-order policy for multi-start start `k` of `starts`: the K starts are **split
/// between the two diversity axes in contiguous blocks** — the first `⌈K/2⌉` starts (including the
/// canonical `k = 0`, so `n_starts == 1` ≡ [`nest`]) keep the largest-first order and diversify
/// through sampling noise alone; the remaining starts additionally jitter the insertion order. A
/// union of both axes measurably beats spending every start on either one (jittering all `k > 0`
/// regressed the mixed-corpus probe — order noise *replaced* the proven sampling-noise breadth
/// instead of adding to it). Pure function of `(k, starts)` — deterministic.
///
/// **Why contiguous blocks, not `k`-parity interleaving:** the canonical half must occupy the seed
/// streams `seed..seed+⌈K/2⌉` *consecutively*, so `nest_multistart(seed, 2K)`'s canonical block
/// covers exactly the stream range `nest_multistart(seed, K)` covered in an all-canonical build —
/// making the old result a **guaranteed floor** when K is doubled. Parity interleaving (tried
/// first) aliased "canonical" with every-other absolute stream, so for a fixed base seed half the
/// canonical streams became permanently unreachable at any K — measured on the consumer corpus as
/// a floor drop (82.76 % → 81.9 %) that raising K could not repair.
///
/// Cross-platform byte-stability of the jittered path is proven by the `multistart-overflow`
/// golden case (`bin/golden_dump.rs`), whose winning start is a jittered one.
fn start_order_policy(k: u64, starts: u64) -> InsertionOrder {
    if k < starts.div_ceil(2) {
        InsertionOrder::Canonical
    } else {
        InsertionOrder::Jittered
    }
}

/// One multi-start start, exactly as [`nest_multistart_per_item`] runs it: seed
/// `seed.wrapping_add(k)` plus the per-`(k, starts)` insertion-order policy
/// ([`start_order_policy`]). The single source of truth for the start wiring — both `run_starts`
/// builds call this, which is what makes the sequential and parallel builds byte-identical by
/// construction.
///
/// Hidden from docs: public only so the integration tests can reconstruct the engine's exact
/// per-start behaviour; NOT a stable API.
#[doc(hidden)]
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn multistart_start(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
    k: u64,
    starts: u64,
) -> NestSolution {
    let Some(problem) = import_problem(items, container, holes, min_sep, rotations_deg) else {
        return NestSolution {
            placements: vec![],
            unplaced: all_unplaced(qty),
        };
    };
    // Public reconstruct helper — area-only score is fine (callers that care about remnant use
    // nest_with_config / multi-start entry points that thread original-outline areas).
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();
    multistart_start_imported(
        &problem,
        qty,
        seed,
        budget,
        k,
        starts,
        PlacementStrategy::Sampling,
        None,
        SeparationEffort::Full,
        nfp::DEFAULT_COLUMN_WEIGHT,
        &areas,
    )
    .0
}

/// [`multistart_start`] plus the engine's remnant-first keep-best score, as the tuple
/// `(placed_area, used_x_max, used_y_max)` — the exact [`NestScore`] fields the multi-start reduction
/// ranks on. Doc-hidden test hook so `parallel_multistart_equals_independent_sequential_best` can
/// mirror the engine's *actual* reduction (not an area-only proxy). NOT a stable API.
#[doc(hidden)]
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn multistart_start_scored(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
    k: u64,
    starts: u64,
) -> (NestSolution, (Scalar, Scalar, Scalar)) {
    let Some(problem) = import_problem(items, container, holes, min_sep, rotations_deg) else {
        return (
            NestSolution {
                placements: vec![],
                unplaced: all_unplaced(qty),
            },
            (0.0, 0.0, 0.0),
        );
    };
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();
    let (sol, score) = multistart_start_imported(
        &problem,
        qty,
        seed,
        budget,
        k,
        starts,
        PlacementStrategy::Sampling,
        None,
        SeparationEffort::Full,
        nfp::DEFAULT_COLUMN_WEIGHT,
        &areas,
    );
    (sol, (score.placed_area, score.used_x_max, score.used_y_max))
}

/// [`multistart_start`] over an already-imported [`Problem`] — the form `run_starts` uses so the K
/// starts share ONE import (the N1a hoist). Import is a pure function, so this is byte-identical
/// to `multistart_start`'s own import for the same raw inputs; the per-start seed and order-policy
/// wiring still has exactly one definition (here).
#[allow(clippy::too_many_arguments)]
fn multistart_start_imported(
    problem: &Problem,
    qty: &[usize],
    seed: u64,
    budget: u64,
    k: u64,
    starts: u64,
    strategy: PlacementStrategy,
    nfp_cache: Option<&nfp::NfpCache>,
    effort: SeparationEffort,
    column_weight: u32,
    areas: &[Scalar],
) -> (NestSolution, NestScore) {
    nest_core_imported(
        problem,
        qty,
        seed.wrapping_add(k),
        budget,
        start_order_policy(k, starts),
        strategy,
        nfp_cache,
        effort,
        column_weight,
        areas,
    )
}

/// Which constructive placement engine a nest run uses (feature #4, docs/03).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlacementStrategy {
    /// The shipped sampling pipeline (uniform sampling + bottom-left slide + improvement rounds).
    /// The default — byte-identical to every pre-NFP release.
    #[default]
    Sampling,
    /// Exact-NFP constructive placement (docs/03): feasible region = IFP ∖ ⋃NFP in scaled integer
    /// arithmetic, candidates at region vertices, the CDE as the verification arbiter, sampling
    /// fallback per part. The separation tail runs unchanged.
    Nfp,
}

/// How hard the post-construction separation tail (`sep::run_separation`) tries on the still-unplaced
/// parts. The separation tail dominates the wall-clock on **over-subscribed** packs — it runs the full
/// GLS separator on each unplaced part, most of which cannot be made to fit (docs/04). `Fast` prunes
/// that waste, at the cost of a **deliberately different (re-blessed) placement** — it is PRNG-coupled,
/// so it changes the layout, not just the speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SeparationEffort {
    /// The shipped behavior: attempt every unplaced part at the full separator budget. **The default —
    /// byte-identical to every prior release** (the committed cross-platform golden runs this).
    #[default]
    Full,
    /// Prune provably-hopeless separation attempts by an **area-conservation** necessary condition: if
    /// the placed CD-footprint area plus the candidate's CD-footprint area already exceeds the
    /// container's CD area, no collision-free arrangement can exist (disjoint footprints inside a
    /// region sum to ≤ its area), so the separator *must* fail — skip it. Sound (never skips a
    /// placeable part), but **placement-changing**: a skipped attempt no longer draws from the shared
    /// PRNG stream, so subsequently-attempted parts shift. Opt-in; has its own golden.
    Fast,
    /// `Full` PLUS a **densification / compaction** pass (docs/04 Tier D): after the normal tail, the
    /// sheet is repeatedly shrunk toward its corner and the whole layout re-separated (the GLS
    /// separator moves every placed part and samples its rotations), pulling the arrangement inward to
    /// close inter-part gaps; freed space is then re-filled. Highest density, highest cost. Strictly
    /// placement-changing (it *rearranges* a feasible layout); opt-in.
    ///
    /// **Multi-start policy (densify-once):** with `restarts > 1`, all K starts explore at Fast
    /// leftover cost (construction + Fast leftover sep + post-sep NFP refill, **no densify**);
    /// `NestScore` keep-best picks the winner; **only that start** is re-run with densify. Avoids
    /// K independent densify loops and the Full leftover tax under Max (docs/04 §4e).
    /// `restarts == 1` densifies that single start after a Fast leftover pass.
    Max,
    /// Skip the post-construction separation tail entirely. The result is the constructive
    /// (sampling/NFP) layout: feasible and deterministic, but it may leave parts unplaced that
    /// `Full`/`Fast` would have packed. Intended as a cheap "preview" pass — the caller can post it
    /// immediately and then refine with `Fast`/`Full` seeds. Not the default.
    ///
    /// Added for Deepnest Server (MPL-2.0 modification).
    Off,
}

/// Configuration for [`nest_with_config`] — the consolidation point for run knobs (docs/03 §7).
/// All fields carry their [`nest`] / [`nest_multistart`] meanings.
#[derive(Clone, Debug)]
pub struct NestConfig {
    pub min_sep: Scalar,
    pub seed: u64,
    pub budget: u64,
    /// Best-of-K multi-start count (clamped to ≥ 1; `1` = a single canonical start).
    pub restarts: usize,
    pub strategy: PlacementStrategy,
    /// Separation-tail effort (see [`SeparationEffort`]); [`SeparationEffort::Full`] preserves the
    /// byte-identical shipped behavior. Added with a `Default` so existing `NestConfig { .. }`
    /// literals need `..Default::default()` — or set it explicitly.
    pub separation_effort: SeparationEffort,
    /// LBF horizontal weight for NFP candidate ranking (`cost = column_weight · x_max + y_max`).
    /// Historical default **10** (jagua) is **byte-identical** to every pre-column-weight golden.
    /// Measured on irregular/sector packs, **3** packs tighter (~+2–3 pp util) by not over-forcing
    /// strict left-columns (docs/04 §4c). Sampling mode currently ignores this (its LBF is still 10).
    pub column_weight: u32,
}

impl Default for NestConfig {
    fn default() -> Self {
        Self {
            min_sep: 0.0,
            seed: 0,
            budget: 1000,
            restarts: 1,
            strategy: PlacementStrategy::Sampling,
            separation_effort: SeparationEffort::Full,
            column_weight: nfp::DEFAULT_COLUMN_WEIGHT,
        }
    }
}

/// Recommended column weight for density-first NFP packs (docs/04 §4c). Not the default — the default
/// stays at 10 for golden byte-identity; consumers that care about remnant/gaps should pass 3.
pub const DENSITY_COLUMN_WEIGHT: u32 = 3;

/// Errors from the strategy-bearing entry points. The sampling paths are infallible (malformed
/// input degrades to an all-unplaced solution, unchanged); NFP mode HARD-ERRORS on out-of-domain
/// coordinates instead of silently degrading (docs/03 §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NestError {
    /// The NFP integer domain gate: worst-case scaled boolean-op coordinate too large.
    NfpInputDomain { worst_case_grid: i64 },
}

impl std::fmt::Display for NestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NestError::NfpInputDomain { worst_case_grid } => write!(
                f,
                "NFP input domain exceeded (worst-case scaled coordinate {worst_case_grid}); \
                 shrink the problem coordinates or use PlacementStrategy::Sampling"
            ),
        }
    }
}

impl std::error::Error for NestError {}

impl From<nfp::NfpError> for NestError {
    fn from(e: nfp::NfpError) -> Self {
        match e {
            nfp::NfpError::InputDomain { worst_case_grid } => {
                NestError::NfpInputDomain { worst_case_grid }
            }
        }
    }
}

/// Nests with an explicit [`NestConfig`] — the strategy-bearing entry point (per-item rotation
/// semantics, best-of-K multi-start). With `strategy: Sampling` this is exactly
/// [`nest_multistart_per_item`] (byte-identical; locked by test). With `strategy: Nfp` the
/// constructive phase is the exact-NFP placer over ONE cache built per call and shared read-only
/// by all K starts (docs/03 §6); everything else — the separation tail, the multi-start block
/// policy, the keep-best reduction — is unchanged.
///
/// # Errors
/// [`NestError::NfpInputDomain`] iff `strategy == Nfp` and the scaled coordinates exceed the
/// integer domain gate.
#[allow(clippy::too_many_arguments)]
pub fn nest_with_config(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    rotations_deg: &[Vec<Scalar>],
    config: &NestConfig,
) -> Result<NestSolution, NestError> {
    assert_eq!(
        items.len(),
        qty.len(),
        "items and qty must be the same length"
    );
    assert_eq!(
        items.len(),
        rotations_deg.len(),
        "items and per-item rotations must be the same length"
    );
    let starts = config.restarts.max(1);
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();

    let Some(problem) = import_problem(items, container, holes, config.min_sep, rotations_deg)
    else {
        return Ok(NestSolution {
            placements: vec![],
            unplaced: all_unplaced(qty),
        });
    };
    // The NFP cache: built ONCE per call, before the starts fork; shared read-only (docs/03 §6).
    let cache = match config.strategy {
        PlacementStrategy::Sampling => None,
        PlacementStrategy::Nfp => Some(nfp::NfpCache::build(
            &problem.entities,
            &problem.rotations_deg,
            &problem.rotations_rad,
            &problem.container,
        )?),
    };

    Ok(reduce_starts(
        &problem,
        qty,
        config.seed,
        config.budget,
        starts,
        &areas,
        config.strategy,
        cache.as_ref(),
        config.separation_effort,
        config.column_weight,
    ))
}

/// Multi-sheet nest with an explicit [`NestConfig`] (the [`nest_multi_multistart_per_item`]
/// semantics: sheets fill in order, leftovers spill forward, per-sheet best-of-K). With
/// `strategy: Sampling` this is exactly [`nest_multi_multistart_per_item`] (byte-identical;
/// test-locked). With `strategy: Nfp` the container-independent [`nfp::PartsGeometry`] —
/// footprints and the pairwise NFP table — is built **once** and shared across every sheet
/// (docs/03 §6); only the per-sheet IFPs are assembled per container.
///
/// # Errors
/// [`NestError::NfpInputDomain`] iff `strategy == Nfp` and any sheet (or the parts) exceeds the
/// integer domain gate.
#[allow(clippy::too_many_arguments)]
pub fn nest_multi_with_config(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    sheets: &[Sheet],
    rotations_deg: &[Vec<Scalar>],
    config: &NestConfig,
) -> Result<MultiSheetSolution, NestError> {
    assert_eq!(
        items.len(),
        qty.len(),
        "items and qty must be the same length"
    );
    assert_eq!(
        items.len(),
        rotations_deg.len(),
        "items and per-item rotations must be the same length"
    );
    let starts = config.restarts.max(1);
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();

    let mut parts: Option<std::sync::Arc<nfp::PartsGeometry>> = None;
    let mut remaining = qty.to_vec();
    let mut per_sheet = Vec::with_capacity(sheets.len());

    for (i, sheet) in sheets.iter().enumerate() {
        // Same per-sheet seed derivation as `nest_multi_multistart_per_item`.
        let sheet_seed = config.seed.wrapping_add(i as u64);
        let Some(problem) = import_problem(
            items,
            &sheet.outline,
            &sheet.holes,
            config.min_sep,
            rotations_deg,
        ) else {
            // Malformed sheet: nothing places on it; demand spills forward (the existing multi
            // semantics — a malformed container yields an all-unplaced sheet).
            per_sheet.push(Vec::new());
            continue;
        };
        let cache = match config.strategy {
            PlacementStrategy::Sampling => None,
            PlacementStrategy::Nfp => {
                // Parts built once (items are identical on every sheet — import is a pure
                // function, so sheet 0's footprints are bit-identical to any sheet's).
                let p = if let Some(p) = &parts {
                    p.clone()
                } else {
                    let built = std::sync::Arc::new(nfp::PartsGeometry::build(
                        &problem.entities,
                        &problem.rotations_deg,
                        &problem.rotations_rad,
                    )?);
                    parts = Some(built.clone());
                    built
                };
                Some(nfp::NfpCache::assemble(p, &problem.container)?)
            }
        };

        let sol = reduce_starts(
            &problem,
            &remaining,
            sheet_seed,
            config.budget,
            starts,
            &areas,
            config.strategy,
            cache.as_ref(),
            config.separation_effort,
            config.column_weight,
        );

        for p in &sol.placements {
            remaining[p.item] -= 1;
        }
        per_sheet.push(sol.placements);
        if remaining.iter().all(|&r| r == 0) {
            break;
        }
    }

    per_sheet.resize_with(sheets.len(), Vec::new);
    let unplaced = remaining
        .iter()
        .enumerate()
        .flat_map(|(i, &n)| std::iter::repeat_n(i, n))
        .collect();
    Ok(MultiSheetSolution {
        per_sheet,
        unplaced,
    })
}

/// A pre-imported, container-INDEPENDENT part library, reusable across many [`nest_with_prepared`]
/// calls with different containers (the "part-in-part" workload: one part set nested into many
/// parent shapes). Built ONCE by [`prepare`], it holds the imported item entities (with CDE
/// surrogates) and — for [`PlacementStrategy::Nfp`] — the shared `Arc<PartsGeometry>`: the eager
/// O((P·R)²) pairwise NFP table (docs/03 §6, docs/04 Tier 2). A plain per-call `nest()` rebuilds all
/// of this every time; a workflow that nests the same parts into `M` containers pays the precompute
/// once instead of `M` times.
///
/// Byte-identity: import is a pure function of `(items, rotations, min_sep)` and `PartsGeometry` is
/// container-independent (the exact property [`nest_multi_with_config`] already relies on to share it
/// across sheets), so `prepare` + `nest_with_prepared` produces the SAME placements as
/// [`nest_with_config`] with the matching [`NestConfig`] — locked by a test. Immutable after build;
/// `Item`/`PartsGeometry` are `Send + Sync`, so a `&PreparedParts` shares across the sanctioned
/// multi-start threads exactly like `&Problem`.
pub struct PreparedParts {
    entities: Vec<Option<Item>>,
    rotations_deg: Vec<Vec<Scalar>>,
    rotations_rad: Vec<Vec<Scalar>>,
    /// Original-outline areas, for the multi-start keep-best reduction.
    areas: Vec<Scalar>,
    /// The separation offset the entities were imported with — a container imported for reuse MUST use
    /// the same `min_sep`, or its footprints would not match the parts. Captured here so the caller
    /// cannot silently desync it.
    min_sep: Scalar,
    strategy: PlacementStrategy,
    /// `Some` iff `strategy == Nfp`: the shared container-independent NFP geometry.
    parts: Option<std::sync::Arc<nfp::PartsGeometry>>,
}

/// Pre-imports a part library for reuse across containers (docs/04 Tier 2). `min_sep` and `strategy`
/// are fixed here (they determine the container-independent precompute); `seed` / `budget` /
/// `restarts` / `separation_effort` / `qty` / the container are chosen per [`nest_with_prepared`]
/// call. `rotations_deg` uses the per-item [`nest_per_item`] semantics (one set per item type, or a
/// single broadcast set — the caller flattens as for [`nest_with_config`]).
///
/// # Errors
/// [`NestError::NfpInputDomain`] iff `strategy == Nfp` and the scaled part coordinates exceed the
/// integer domain gate (the container side is re-checked per call in [`nest_with_prepared`]).
pub fn prepare(
    items: &[Vec<[Scalar; 2]>],
    rotations_deg: &[Vec<Scalar>],
    min_sep: Scalar,
    strategy: PlacementStrategy,
) -> Result<PreparedParts, NestError> {
    assert_eq!(
        items.len(),
        rotations_deg.len(),
        "items and per-item rotations must be the same length"
    );
    let importer = make_importer(min_sep);
    let (rotations_deg_per_item, rotations_rad) = normalize_rotations(rotations_deg);
    let entities = import_entities(&importer, items, &rotations_deg_per_item);
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();
    let parts = match strategy {
        PlacementStrategy::Sampling => None,
        PlacementStrategy::Nfp => Some(std::sync::Arc::new(nfp::PartsGeometry::build(
            &entities,
            &rotations_deg_per_item,
            &rotations_rad,
        )?)),
    };
    Ok(PreparedParts {
        entities,
        rotations_deg: rotations_deg_per_item,
        rotations_rad,
        areas,
        min_sep,
        strategy,
        parts,
    })
}

/// Nests a [`prepare`]d part library into one `container` (with `holes`), reusing the pre-built
/// entities + pairwise NFP table. Byte-identical to [`nest_with_config`] with a matching
/// [`NestConfig`] (`min_sep`/`strategy` from `prepared`, the rest from here) — the reuse just moves
/// the container-independent precompute out of the per-call hot path (docs/04 Tier 2).
///
/// `qty` is per call (`qty.len()` must equal the prepared item count), so the same library can fill
/// containers with different demand.
///
/// # Errors
/// [`NestError::NfpInputDomain`] iff `strategy == Nfp` and this container exceeds the integer domain
/// gate.
#[allow(clippy::too_many_arguments)]
pub fn nest_with_prepared(
    prepared: &PreparedParts,
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    seed: u64,
    budget: u64,
    restarts: usize,
    separation_effort: SeparationEffort,
) -> Result<NestSolution, NestError> {
    nest_with_prepared_weighted(
        prepared,
        qty,
        container,
        holes,
        seed,
        budget,
        restarts,
        separation_effort,
        nfp::DEFAULT_COLUMN_WEIGHT,
    )
}

/// Like [`nest_with_prepared`] but with an explicit LBF [`NestConfig::column_weight`].
#[allow(clippy::too_many_arguments)]
pub fn nest_with_prepared_weighted(
    prepared: &PreparedParts,
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    seed: u64,
    budget: u64,
    restarts: usize,
    separation_effort: SeparationEffort,
    column_weight: u32,
) -> Result<NestSolution, NestError> {
    assert_eq!(
        prepared.entities.len(),
        qty.len(),
        "prepared item count and qty must be the same length"
    );
    let starts = restarts.max(1);

    // Import ONLY the container (with holes), with an importer configured identically to the one that
    // imported the parts — a pure function of `(container, holes, min_sep)`, so it matches what
    // `import_problem` would have produced.
    let importer = make_importer(prepared.min_sep);
    let container_outline = container.to_vec();
    let holes_vec = holes.to_vec();
    let Some(container) = import_container_with_holes(&importer, container, holes) else {
        return Ok(NestSolution {
            placements: vec![],
            unplaced: all_unplaced(qty),
        });
    };

    // Rebuild the `Problem` view over the shared entities. The clone is Arc-cheap (Item wraps
    // `Arc<SPolygon>`) and runs ONCE per container, not per start — negligible against the O(P²) table
    // it lets us skip.
    let problem = Problem {
        entities: prepared.entities.clone(),
        container,
        rotations_deg: prepared.rotations_deg.clone(),
        rotations_rad: prepared.rotations_rad.clone(),
        container_outline,
        holes: holes_vec,
        min_sep: prepared.min_sep,
    };

    // Reassemble only this container's IFPs over the shared parts (the per-sheet step
    // `nest_multi_with_config` already does — here across calls).
    let cache = match prepared.strategy {
        PlacementStrategy::Sampling => None,
        PlacementStrategy::Nfp => Some(nfp::NfpCache::assemble(
            prepared
                .parts
                .clone()
                .expect("NFP-prepared parts always carry PartsGeometry"),
            &problem.container,
        )?),
    };

    Ok(reduce_starts(
        &problem,
        qty,
        seed,
        budget,
        starts,
        &prepared.areas,
        prepared.strategy,
        cache.as_ref(),
        separation_effort,
        column_weight,
    ))
}

/// The imported, placement-ready problem (the N1a import hoist — docs/03 §6): items, container,
/// and normalized rotation sets, built ONCE per public call by [`import_problem`] and shared
/// read-only by every multi-start start (and, later, the NFP cache precompute). Import is a pure
/// function of its inputs — no PRNG, no ambient state — so "import once, reuse per start" is
/// byte-identical to the previous "import inside every start"; the untouched goldens are the
/// proof. No interior mutability anywhere inside, so a `&Problem` crossing the sanctioned
/// multi-start threads is shared IMMUTABLE state (the parallel-exception argument is unchanged);
/// each start clones the container into its own `Layout`.
struct Problem {
    entities: Vec<Option<Item>>,
    container: Container,
    rotations_deg: Vec<Vec<Scalar>>,
    rotations_rad: Vec<Vec<Scalar>>,
    /// The raw container outline + holes + separation, kept so the densification pass
    /// (`SeparationEffort::Max`) can build *shrunk* containers to create compaction pressure.
    container_outline: Vec<[Scalar; 2]>,
    holes: Vec<Vec<[Scalar; 2]>>,
    min_sep: Scalar,
}

/// The [`Importer`] configured for `min_sep` (default CDE config + auto collision-footprint
/// decimation). Single definition shared by [`import_problem`] and [`prepare`] so items and
/// containers import identically regardless of which entry point built them.
///
/// Auto decimation is enabled whenever a separation is requested (`min_sep == 0` needs no offset and
/// so no superset margin). Applied symmetrically to every imported hazard — items (Inflate), the
/// container boundary and holes (Deflate/Inflate) — but only takes effect on HIGH-VERTEX shapes
/// (`convert_to_internal` gates per-shape on `DECIMATION_MIN_VERTICES`), so simple parts and
/// axis-aligned sheets keep the exact offset path (and the determinism golden) unchanged.
fn make_importer(min_sep: Scalar) -> Importer {
    let cde_config = default_cde_config();
    let min_item_separation = (min_sep > 0.0).then_some(min_sep);
    let mut importer = Importer::new(cde_config, None, min_item_separation, None);
    importer.shape_modify_config.collision_decimation =
        (min_sep > 0.0).then_some(min_sep * DECIMATION_TOL_FRACTION);
    importer
}

/// Normalizes per-item rotations into `(degrees, radians)`: an empty set for an item ⇒ `{0}` (no
/// rotation), applied independently per item type. Degrees drive the importer metadata / exact-
/// cardinal path, radians the sampler and libm path.
fn normalize_rotations(rotations_deg: &[Vec<Scalar>]) -> (Vec<Vec<Scalar>>, Vec<Vec<Scalar>>) {
    let deg: Vec<Vec<Scalar>> = rotations_deg
        .iter()
        .map(|r| if r.is_empty() { vec![0.0] } else { r.clone() })
        .collect();
    let rad: Vec<Vec<Scalar>> = deg
        .iter()
        .map(|r| r.iter().map(|d| d.to_radians()).collect())
        .collect();
    (deg, rad)
}

/// Imports the item entities (with surrogates) — the CONTAINER-INDEPENDENT half of an import. A
/// malformed item ⇒ that whole type is unplaced (`None`). High-vertex curved parts get their
/// collision footprint decimated by the importer's `collision_decimation`; `shape_orig` (which drives
/// the reported placement frame) is always the untouched original outline. Each item carries its own
/// `allowed_orientations`, so the separation search honours the per-item set automatically.
///
/// `import_item` takes `&self`, so this is a pure function of `(importer config, items, rotations)` —
/// which is exactly why [`prepare`] can run it once and reuse the result across containers.
fn import_entities(
    importer: &Importer,
    items: &[Vec<[Scalar; 2]>],
    rotations_deg_per_item: &[Vec<Scalar>],
) -> Vec<Option<Item>> {
    let mut entities: Vec<Option<Item>> = Vec::with_capacity(items.len());
    for (i, outline) in items.iter().enumerate() {
        let ext_item = ExtItem {
            id: i as u64,
            allowed_orientations: Some(rotations_deg_per_item[i].clone()),
            shape: ExtShape::SimplePolygon(ext_spolygon(outline)),
            min_quality: None,
        };
        entities.push(importer.import_item(&ext_item).ok());
    }
    entities
}

/// Imports items + container (with holes) into a [`Problem`]. Returns `None` iff the container (or
/// an in-bounds hole) is malformed — the caller then places nothing (conservative — never silently
/// place into an intended keep-out). Byte-identical to the pre-refactor inline version: the same
/// importer config, the same container-then-items order over an `&self` importer.
fn import_problem(
    items: &[Vec<[Scalar; 2]>],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
) -> Option<Problem> {
    let importer = make_importer(min_sep);
    let (rotations_deg_per_item, rotations_rad) = normalize_rotations(rotations_deg);
    let container_outline = container.to_vec();
    let holes_vec = holes.to_vec();
    let container = import_container_with_holes(&importer, container, holes)?;
    let entities = import_entities(&importer, items, &rotations_deg_per_item);
    Some(Problem {
        entities,
        container,
        rotations_deg: rotations_deg_per_item,
        rotations_rad,
        container_outline,
        holes: holes_vec,
        min_sep,
    })
}

/// The full nest pipeline behind [`nest`] / [`nest_per_item`] / the multi-start drivers.
#[must_use]
#[allow(clippy::too_many_arguments)]
fn nest_core(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
    insertion_order: InsertionOrder,
) -> NestSolution {
    assert_eq!(
        items.len(),
        qty.len(),
        "items and qty must be the same length"
    );
    assert_eq!(
        items.len(),
        rotations_deg.len(),
        "items and per-item rotations must be the same length"
    );

    let Some(problem) = import_problem(items, container, holes, min_sep, rotations_deg) else {
        return NestSolution {
            placements: vec![],
            unplaced: all_unplaced(qty),
        };
    };
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();
    nest_core_imported(
        &problem,
        qty,
        seed,
        budget,
        insertion_order,
        PlacementStrategy::Sampling,
        None,
        SeparationEffort::Full,
        nfp::DEFAULT_COLUMN_WEIGHT,
        &areas,
    )
    .0
}

/// The placement pipeline over an already-imported [`Problem`] — one multi-start start's worth of
/// work. Byte-identical to the pre-hoist `nest_core` for the same inputs (Sampling strategy).
/// Under [`PlacementStrategy::Nfp`] the constructive phase is the NFP placer (docs/03 §5) over the
/// shared prebuilt `nfp_cache`; the separation tail runs unchanged in both modes.
///
/// Returns the solution plus a [`NestScore`] (placed area + packing far-edge) so multi-start can
/// keep the layout that leaves the largest end-of-sheet remnant.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn nest_core_imported(
    problem: &Problem,
    qty: &[usize],
    seed: u64,
    budget: u64,
    insertion_order: InsertionOrder,
    strategy: PlacementStrategy,
    nfp_cache: Option<&nfp::NfpCache>,
    effort: SeparationEffort,
    column_weight: u32,
    areas: &[Scalar],
) -> (NestSolution, NestScore) {
    let Problem {
        entities,
        container,
        rotations_deg: _,
        rotations_rad: rotations_rad_per_item,
        ..
    } = problem;
    let items_len = entities.len();

    let mut layout = Layout::new(container.clone());
    let mut prng = Prng::seed_from_u64(seed);

    // Placement order: largest items first (descending CD-shape diameter); stable on ties. Under
    // `Jittered` the diameters are first scaled by seeded per-type noise factors — drawn here,
    // BEFORE any placement sampling, in ascending-id order (a fixed draw order is part of the
    // determinism contract). `Canonical` draws nothing, so the single-start PRNG stream (and the
    // existing goldens) are untouched.
    let sort_keys: Vec<Scalar> = entities
        .iter()
        .map(|e| {
            e.as_ref().map_or(0.0, |item| {
                let d = item.shape_cd.diameter;
                match insertion_order {
                    InsertionOrder::Canonical => d,
                    InsertionOrder::Jittered => {
                        d * prng.range(1.0 - ORDER_JITTER, 1.0 + ORDER_JITTER)
                    }
                }
            })
        })
        .collect();
    let mut order: Vec<usize> = (0..items_len).filter(|&i| entities[i].is_some()).collect();
    // Descending; `total_cmp` is a deterministic total order (keys are finite for valid polygons)
    // and the stable sort keeps ascending-id order on exact ties.
    order.sort_by(|&a, &b| sort_keys[b].total_cmp(&sort_keys[a]));

    let mut placed_per_type = vec![0usize; items_len];
    match strategy {
        PlacementStrategy::Sampling => {
            constructive_fill(
                &mut layout,
                entities,
                &order,
                qty,
                rotations_rad_per_item,
                &mut prng,
                budget,
                &mut placed_per_type,
            );

            // Improvement: compact placed items toward the bottom-left, then fill freed space.
            // Fixed number of rounds (no wall clock); deterministic via the same seeded PRNG stream.
            improve::improve(
                &mut layout,
                entities,
                &order,
                qty,
                rotations_rad_per_item,
                &mut prng,
                budget,
                IMPROVE_ROUNDS,
                &mut placed_per_type,
            );
        }
        PlacementStrategy::Nfp => {
            let cache = nfp_cache.expect("NFP strategy requires a prebuilt cache");
            let mut placed_obs: Vec<nfp::PlacedObstacle> = Vec::new();
            let mut stats = nfp::place::NfpPlaceStats::default();
            // Construction + exactly ONE refill pass (docs/03 §5.3): the slide compaction is
            // skipped (it would smear the □E backoff), so a second identical pass — which can
            // exploit space opened by later types' fallback placements — is all a refill can do.
            for _ in 0..2 {
                nfp::place::place_pass(
                    &mut layout,
                    entities,
                    &order,
                    qty,
                    rotations_rad_per_item,
                    cache,
                    &mut prng,
                    budget,
                    &mut placed_obs,
                    &mut placed_per_type,
                    &mut stats,
                    column_weight,
                );
            }
            // Belt-and-braces (docs/03 §9.2): an NFP construction can never leave an infeasible
            // layout — every pose passed the CDE arbiter.
            debug_assert!(
                layout.is_feasible(),
                "NFP construction left an infeasible layout"
            );
            // NOTE: a post-NFP `improve::compact` (BL slide) was measured and REJECTED for the
            // default path (2026-07-10): it regressed over-subscribed util (86.6 → 85.8 % on the
            // sector corpus) by rearranging a feasible construction into a worse basin for the sep
            // tail. Remnant consolidation of already-feasible layouts is `SeparationEffort::Max`
            // (densify) + multi-start remnant-first keep-best — not a free compact.
        }
    }

    // Separation search (Phase 2b): for any still-unplaced part, allow overlap then shove neighbours
    // apart (sparrow GLS) to discover interlocking arrangements greedy construction cannot reach. A
    // no-op when everything already placed, so the dense rectangular cases pay nothing.
    //
    // Max uses the **Fast leftover budget** here (area-skip + reduced sep), then densifies below.
    // Full leftover under Max was the 97 s over-sub tax with almost no densify benefit — densify
    // only rearranges already-placed parts (docs/04 §4e). Placement-changing vs old Max leftover.
    let leftover_effort = match effort {
        SeparationEffort::Max => SeparationEffort::Fast,
        other => other,
    };
    sep::run_separation(
        &mut layout,
        entities,
        &order,
        qty,
        &mut prng,
        &mut placed_per_type,
        leftover_effort,
    );

    // Post-separation NFP refill (docs/03 §13 Q5 — the GO density lever): separation can open
    // contacts / free pockets by rearranging neighbours. Rebuild the integer obstacle list from the
    // live layout and run one more NFP place_pass for still-unplaced demand. Cheap (region walk only
    // for unplaced types) and placement-changing only when it actually seats more parts.
    if strategy == PlacementStrategy::Nfp {
        let cache = nfp_cache.expect("NFP strategy requires a prebuilt cache");
        let mut placed_obs = nfp::place::obstacles_from_layout(&layout, rotations_rad_per_item);
        let mut stats = nfp::place::NfpPlaceStats::default();
        nfp::place::place_pass(
            &mut layout,
            entities,
            &order,
            qty,
            rotations_rad_per_item,
            cache,
            &mut prng,
            budget,
            &mut placed_obs,
            &mut placed_per_type,
            &mut stats,
            column_weight,
        );
    }

    // Densification (`SeparationEffort::Max`, docs/04 Tier D): a REVISED compaction. An earlier
    // wall-squeeze keep-out was reverted (docs/02 §11.2, zero gain on rect/brick/pentagon/mixed) —
    // but it was measured on rotation-invariant / no-slack cases. This version shrinks the actual
    // container and re-separates the WHOLE layout (moving already-placed parts, which the leftover
    // separator never touches), then refills — the mechanism a human uses to close gaps between
    // irregular parts. Opt-in and rearranges a feasible layout, so it is placement-changing.
    if effort == SeparationEffort::Max {
        densify(
            &mut layout,
            problem,
            entities,
            &order,
            qty,
            &mut prng,
            &mut placed_per_type,
        );
        // After densify free-space opens on the real sheet — one more NFP refill if applicable.
        if strategy == PlacementStrategy::Nfp {
            let cache = nfp_cache.expect("NFP strategy requires a prebuilt cache");
            let mut placed_obs = nfp::place::obstacles_from_layout(&layout, rotations_rad_per_item);
            let mut stats = nfp::place::NfpPlaceStats::default();
            nfp::place::place_pass(
                &mut layout,
                entities,
                &order,
                qty,
                rotations_rad_per_item,
                cache,
                &mut prng,
                budget,
                &mut placed_obs,
                &mut placed_per_type,
                &mut stats,
                column_weight,
            );
        }
    }

    // Score BEFORE extract — CD footprints give the true packing far-edge (remnant metric).
    let score = NestScore::from_layout(&layout, areas);

    let placements = extract_placements(&layout, entities);

    let mut unplaced = vec![];
    for (i, (&want, &got)) in qty.iter().zip(&placed_per_type).enumerate() {
        for _ in got..want {
            unplaced.push(i);
        }
    }

    (
        NestSolution {
            placements,
            unplaced,
        },
        score,
    )
}

/// Densification steps: how many times the used span is shrunk (compounding [`DENSIFY_SHRINK`]). The
/// loop also stops on the first infeasible shrink and on a `NestScore` plateau ([`DENSIFY_PLATEAU`]),
/// so this is an upper bound. Fixed integer — never a wall clock. Many small steps let the local GLS
/// separator compact *incrementally* — each a nudge it can resolve — rather than one jump it gets
/// stuck on.
const DENSIFY_STEPS: u32 = 60;
/// Per-step shrink of the used span along the compaction axis (1 % steps). Gentle by design: the
/// separator is a *local* search, so it can only follow a slowly-tightening boundary. (2 % was
/// measured ~20 % faster densify but +0.2 `max_x` on the remnant corpus — keep 1 %.)
const DENSIFY_SHRINK: Scalar = 0.99;
/// Stop densify after this many consecutive *successful* shrinks that do not beat the `NestScore`
/// incumbent. Later steps still run a full `separate_layout` even when the keep-best already plateaus,
/// which dominated wall-clock on the production Max path (docs/04 §4e). Placement-changing only if a
/// later post-plateau step would have improved the score (rare on measured corpora); the incumbent
/// best is always restored.
const DENSIFY_PLATEAU: u32 = 5;

/// Densification / compaction (`SeparationEffort::Max`, docs/04 Tier D). Repeatedly shrink the sheet
/// toward its bottom-left corner and re-separate the WHOLE layout — the GLS separator moves every
/// placed part (sampling its allowed rotations), so the arrangement is pulled inward, closing the
/// inter-part gaps a greedy LBF construction leaves. After each successful (feasible) compaction the
/// freed space is re-filled with still-unplaced parts. Keeps the densest arrangement found (by total
/// placed area, then earliest). A no-op when nothing is placed.
///
/// This is the mechanism that moves *already-feasible* parts — which the leftover-only
/// [`sep::run_separation`] never touches (it only shoves neighbours of an overlapping seed). Cost is
/// high (a full separation per shrink step), which is why it is gated behind the opt-in `Max` tier.
/// Placement-changing: it deliberately rearranges a feasible layout.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn densify(
    layout: &mut Layout,
    problem: &Problem,
    entities: &[Option<Item>],
    order: &[usize],
    qty: &[usize],
    prng: &mut Prng,
    placed_per_type: &mut [usize],
) {
    if layout.placed_items.is_empty() {
        return;
    }
    let bbox = layout.container.outer_cd.bbox;
    let importer = make_importer(problem.min_sep);

    // Compact along the axis with the most SLACK, keeping the other full. LBF packs full-height (or
    // full-width) columns that grow along one axis, so the gaps to close live along that growth axis;
    // shrinking it leaves a clean rectangular remnant at that end (docs/04 Tier D). Slack per axis =
    // 1 − used_extent / sheet_extent; shrink the slacker one toward its low edge (the corner LBF packs
    // toward), leaving the other axis at full extent.
    let (ux, uy) = used_extent(layout);
    let slack_x = 1.0 - (ux.1 - ux.0) / (bbox.x_max - bbox.x_min).max(f64::MIN_POSITIVE);
    let slack_y = 1.0 - (uy.1 - uy.0) / (bbox.y_max - bbox.y_min).max(f64::MIN_POSITIVE);
    let shrink_x = slack_x >= slack_y;
    let (ax, ay) = (bbox.x_min, bbox.y_min); // anchor = the low corner

    // Scales the raw outline + holes toward the anchor by `factor` (∈ (0,1]) along the chosen axis
    // only.
    let scale = |factor: Scalar| -> (Vec<[Scalar; 2]>, Vec<Vec<[Scalar; 2]>>) {
        let s = |p: &[Scalar; 2]| {
            if shrink_x {
                [ax + (p[0] - ax) * factor, p[1]]
            } else {
                [p[0], ay + (p[1] - ay) * factor]
            }
        };
        (
            problem.container_outline.iter().map(s).collect(),
            problem
                .holes
                .iter()
                .map(|h| h.iter().map(s).collect())
                .collect(),
        )
    };

    // Pose-only keep-best: densify always evaluates on the real sheet (swap back before score), so
    // the container in a full `Layout::save` would be unused on restore (docs/04 0c).
    let mut best = layout.save_poses();
    let mut best_ppt = placed_per_type.to_vec();
    // Keep-best: same remnant-first order as multi-start ([`NestScore`]) — max placed area, then
    // minimize the packing's high-side edge so the free strip at the sheet end is as large as
    // possible. `areas` for densify use CD footprint areas (rigid-invariant; same ranking intent).
    let densify_areas: Vec<Scalar> = entities
        .iter()
        .map(|e| e.as_ref().map_or(0.0, |it| it.shape_cd.area))
        .collect();
    let mut best_score = NestScore::from_layout(layout, &densify_areas);

    // Shrink RELATIVE TO THE USED SPAN, not the whole sheet: `base` is the scale factor that
    // reproduces the current used extent along the compaction axis, so the first step already lands
    // just inside it (immediate pressure) instead of wasting steps crossing the empty remnant.
    let (anchor, sheet_span) = if shrink_x {
        (ax, bbox.x_max - ax)
    } else {
        (ay, bbox.y_max - ay)
    };
    let used_far = if shrink_x { ux.1 } else { uy.1 };
    let base = ((used_far - anchor) / sheet_span.max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);

    // Demand already met? Then the per-step leftover `run_separation` is a pure no-op that still
    // walks the order — skip it (byte-identical when nothing is unplaced).
    let demand_open = |ppt: &[usize]| order.iter().any(|&id| ppt[id] < qty[id]);

    let mut mult: Scalar = 1.0;
    let mut plateau: u32 = 0;
    for _ in 0..DENSIFY_STEPS {
        mult *= DENSIFY_SHRINK;
        let factor = base * mult;
        let (out, hls) = scale(factor);
        let Some(shrunk) = import_container_with_holes(&importer, &out, &hls) else {
            break;
        };
        // Shrink the sheet: placed parts that no longer fit now overlap the boundary / each other.
        // `swap_container` returns the real sheet so we restore without re-cloning it every step.
        let real_sheet = layout.swap_container(shrunk);
        let feasible = sep::separate_layout(layout, entities, prng);
        // Evaluate against the REAL sheet (the shrunk one was only a compaction jig).
        let _shrunk = layout.swap_container(real_sheet);

        if feasible && layout.is_feasible() {
            // Compacted into the smaller region → free space opened in the real sheet. Refill any
            // still-unplaced REQUIRED parts (skip when demand is already met — all-fit remnant jobs).
            if demand_open(placed_per_type) {
                sep::run_separation(
                    layout,
                    entities,
                    order,
                    qty,
                    prng,
                    placed_per_type,
                    SeparationEffort::Full,
                );
            }
            let score = NestScore::from_layout(layout, &densify_areas);
            if score.better_than(best_score) {
                best = layout.save_poses();
                best_ppt.copy_from_slice(placed_per_type);
                best_score = score;
                plateau = 0;
            } else {
                plateau += 1;
                if plateau >= DENSIFY_PLATEAU {
                    // Keep-best has stalled; further shrinks thrash the separator without improving
                    // the remnant metric. Restore the incumbent and stop.
                    layout.restore_poses(&best);
                    placed_per_type.copy_from_slice(&best_ppt);
                    break;
                }
            }
            // keep shrinking for more pressure (while plateau < limit)
        } else {
            // This shrink was too aggressive to separate cleanly — revert and stop.
            layout.restore_poses(&best);
            placed_per_type.copy_from_slice(&best_ppt);
            break;
        }
    }
    layout.restore_poses(&best);
    placed_per_type.copy_from_slice(&best_ppt);
}

/// The `((x_min, x_max), (y_min, y_max))` extent of every placed part's footprint — the "used
/// region". `((0,0),(0,0))` when nothing is placed.
fn used_extent(layout: &Layout) -> ((Scalar, Scalar), (Scalar, Scalar)) {
    let (mut x0, mut y0, mut x1, mut y1) = (
        Scalar::INFINITY,
        Scalar::INFINITY,
        Scalar::NEG_INFINITY,
        Scalar::NEG_INFINITY,
    );
    let mut any = false;
    for pi in layout.placed_items.values() {
        let b = pi.shape.bbox;
        x0 = x0.min(b.x_min);
        y0 = y0.min(b.y_min);
        x1 = x1.max(b.x_max);
        y1 = y1.max(b.y_max);
        any = true;
    }
    if any {
        ((x0, x1), (y0, y1))
    } else {
        ((0.0, 0.0), (0.0, 0.0))
    }
}

/// Runs the nest from `n_starts` decorrelated seeds and returns the **best** result under the
/// remnant-first order ([`NestScore`]: max placed area, then min packing far-edge so the free strip
/// at the sheet end is as large as possible). A single greedy construction lands in a
/// *seed-dependent basin*; sweeping several seeds and keeping the best lifts packing on
/// heterogeneous parts (measured: 13×7 bricks 91.9 % → 95.5 % at K=16) at no determinism cost.
/// Applies **one** rotation set to every item; for a per-item set use [`nest_multistart_per_item`].
///
/// * `n_starts` — number of independent constructions (clamped to ≥ 1). **`n_starts == 1` is
///   byte-identical to [`nest`]** with the same seed, so the existing determinism golden is untouched.
/// * every other argument carries its [`nest`] meaning. Start *k* uses `seed.wrapping_add(k)`; the
///   PRNG's `SplitMix64` expansion decorrelates adjacent seeds into well-separated streams.
///
/// The K starts are split between two diversity axes in contiguous blocks (v0.5, a
/// placement-changing change for `n_starts > 1` vs v0.4): the **first `⌈K/2⌉` starts** keep the
/// canonical largest-first insertion order and diversify through sampling noise alone; the
/// **remaining starts** additionally re-sort the insertion order under seeded multiplicative key
/// noise — breadth over the order dimension, which pays on mixes where largest-diameter-first is
/// misleading (e.g. long skinny parts; measured +1.0 pp mean on a strips+squares corpus at K=8,
/// ~no cost on similar-diameter mixes — `examples/orderdiv.rs`). Because the canonical block is
/// contiguous from `seed`, doubling `n_starts` keeps every canonical stream the smaller run
/// explored — the old best-of-K is a floor for the new best-of-2K at the same seed.
///
/// Determinism: each start is a byte-stable pipeline whose insertion order is a pure function of
/// `(seed, k, inputs)`; the keep-best reduction is **remnant-first** ([`NestScore`]: max placed area,
/// then min packing far-edge `used_x_max` / `used_y_max` so the free strip at the sheet end is as
/// large as possible) and keeps the earliest *k* on a full tie — the chosen layout is byte-identical
/// for the same arguments.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_multistart(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Scalar],
    seed: u64,
    budget: u64,
    n_starts: usize,
) -> NestSolution {
    let rotations_per_item = vec![rotations_deg.to_vec(); items.len()];
    nest_multistart_per_item(
        items,
        qty,
        container,
        holes,
        min_sep,
        &rotations_per_item,
        seed,
        budget,
        n_starts,
    )
}

/// [`nest_multistart`] with a **distinct allowed-rotation set per item type** (the [`nest_per_item`]
/// semantics): `rotations_deg[k]` applies to `items[k]`. Same best-of-K determinism contract.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_multistart_per_item(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container: &[[Scalar; 2]],
    holes: &[Vec<[Scalar; 2]>],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
    n_starts: usize,
) -> NestSolution {
    assert_eq!(
        items.len(),
        qty.len(),
        "items and qty must be the same length"
    );
    assert_eq!(
        items.len(),
        rotations_deg.len(),
        "items and per-item rotations must be the same length"
    );
    let starts = n_starts.max(1);
    let areas: Vec<Scalar> = items.iter().map(|o| polygon_area(o)).collect();

    // Import ONCE (the N1a hoist); all K starts share the immutable `Problem` read-only.
    let Some(problem) = import_problem(items, container, holes, min_sep, rotations_deg) else {
        return NestSolution {
            placements: vec![],
            unplaced: all_unplaced(qty),
        };
    };

    // Run the K independent starts (sequentially by default; on threads under the `parallel` feature —
    // byte-identical either way). Returned as `(solution, NestScore)` in k = 0..starts order.
    let runs = run_starts(&problem, qty, seed, budget, starts, &areas);
    keep_best_start(runs)
}

/// Keep-best reduction in canonical *k* order: remnant-first [`NestScore`] (max area, then min
/// packing far-edge). An exact score tie keeps the earliest *k*. Pure function of the run list —
/// no cross-thread float reduction.
fn keep_best_start(runs: Vec<(NestSolution, NestScore)>) -> NestSolution {
    let best_k = keep_best_start_index(&runs);
    runs.into_iter()
        .nth(best_k)
        .expect("n_starts is clamped to >= 1, so at least one solution is produced")
        .0
}

/// Index of the [`NestScore`] winner in canonical *k* order (earliest *k* on a full tie).
fn keep_best_start_index(runs: &[(NestSolution, NestScore)]) -> usize {
    assert!(
        !runs.is_empty(),
        "n_starts is clamped to >= 1, so at least one solution is produced"
    );
    let mut best_i = 0;
    let mut best_score = runs[0].1;
    for (i, (_, score)) in runs.iter().enumerate().skip(1) {
        if score.better_than(best_score) {
            best_i = i;
            best_score = *score;
        }
    }
    best_i
}

/// Multi-start reduction with **Max densify-once** (production path: `nfp + restarts=8 + max`).
///
/// When `effort == Max` and `starts > 1`:
/// 1. Explore all K starts at [`SeparationEffort::Fast`] leftover cost (construction + Fast leftover
///    sep + post-sep NFP refill — **no densify**). Fast avoids the Full leftover thrash on
///    over-subscribed packs while still ranking starts by remnant-first [`NestScore`].
/// 2. Keep best by remnant-first [`NestScore`].
/// 3. Re-run **only** the winning start with Max densify (Fast leftover + densify).
///
/// This cuts densify from K× to 1× and Full leftover from K× to ~0 while keeping multi-start
/// diversity. `restarts == 1` and non-Max efforts keep the previous reduce-everything path
/// (byte-identical for Full/Fast; Max K=1 is placement-changing vs pre-Fast-leftover Max only when
/// leftover actually runs).
#[allow(clippy::too_many_arguments)]
fn reduce_starts(
    problem: &Problem,
    qty: &[usize],
    seed: u64,
    budget: u64,
    starts: usize,
    areas: &[Scalar],
    strategy: PlacementStrategy,
    nfp_cache: Option<&nfp::NfpCache>,
    effort: SeparationEffort,
    column_weight: u32,
) -> NestSolution {
    if effort == SeparationEffort::Max && starts > 1 {
        let runs = run_starts_with(
            problem,
            qty,
            seed,
            budget,
            starts,
            areas,
            strategy,
            nfp_cache,
            SeparationEffort::Fast,
            column_weight,
        );
        let best_k = keep_best_start_index(&runs);
        // Densify the NestScore winner only. Same per-start seed + order policy as exploration.
        multistart_start_imported(
            problem,
            qty,
            seed,
            budget,
            best_k as u64,
            starts as u64,
            strategy,
            nfp_cache,
            SeparationEffort::Max,
            column_weight,
            areas,
        )
        .0
    } else {
        keep_best_start(run_starts_with(
            problem,
            qty,
            seed,
            budget,
            starts,
            areas,
            strategy,
            nfp_cache,
            effort,
            column_weight,
        ))
    }
}

/// Runs `starts` independent constructions over one shared imported [`Problem`], start *k* at
/// `seed.wrapping_add(k)`, returning `(solution, NestScore)` in `0..starts` order.
/// **Sequential** build: a plain loop.
#[cfg(not(feature = "parallel"))]
#[allow(clippy::too_many_arguments)]
fn run_starts_with(
    problem: &Problem,
    qty: &[usize],
    seed: u64,
    budget: u64,
    starts: usize,
    areas: &[Scalar],
    strategy: PlacementStrategy,
    nfp_cache: Option<&nfp::NfpCache>,
    effort: SeparationEffort,
    column_weight: u32,
) -> Vec<(NestSolution, NestScore)> {
    (0..starts as u64)
        .map(|k| {
            multistart_start_imported(
                problem,
                qty,
                seed,
                budget,
                k,
                starts as u64,
                strategy,
                nfp_cache,
                effort,
                column_weight,
                areas,
            )
        })
        .collect()
}

/// The Sampling-mode shim (the pre-NFP `run_starts` semantics, byte-frozen for K=1).
fn run_starts(
    problem: &Problem,
    qty: &[usize],
    seed: u64,
    budget: u64,
    starts: usize,
    areas: &[Scalar],
) -> Vec<(NestSolution, NestScore)> {
    run_starts_with(
        problem,
        qty,
        seed,
        budget,
        starts,
        areas,
        PlacementStrategy::Sampling,
        None,
        SeparationEffort::Full,
        nfp::DEFAULT_COLUMN_WEIGHT,
    )
}

/// Runs `starts` independent constructions concurrently on scoped threads (one per start), returning
/// `(solution, NestScore)` in `0..starts` order — **byte-identical** to the sequential build: each
/// start is the self-contained [`multistart_start_imported`] (own PRNG, own `Layout`; the shared
/// [`Problem`] is IMMUTABLE — no interior mutability — so no shared *mutable* state crosses threads;
/// the identical per-start seed + order policy as the sequential build), the results are collected
/// in `k` order via ordered join handles, and the caller's keep-best reduction runs afterwards in
/// that same fixed order, so completion order is irrelevant and no float sum ever crosses a thread.
/// This is the one sanctioned use of threads (the multi-start meta-loop only — never inside a
/// placement search); the cross-platform golden run under `--features parallel` is the standing
/// proof it matches the sequential snapshot.
#[cfg(feature = "parallel")]
#[allow(clippy::too_many_arguments)]
fn run_starts_with(
    problem: &Problem,
    qty: &[usize],
    seed: u64,
    budget: u64,
    starts: usize,
    areas: &[Scalar],
    strategy: PlacementStrategy,
    nfp_cache: Option<&nfp::NfpCache>,
    effort: SeparationEffort,
    column_weight: u32,
) -> Vec<(NestSolution, NestScore)> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..starts as u64)
            .map(|k| {
                scope.spawn(move || {
                    // The same `multistart_start_imported` as the sequential build — the per-start
                    // seed and order policy have a single definition, so the two builds cannot
                    // drift. The `Problem` and the optional `NfpCache` cross the threads as
                    // immutable references (no interior mutability — the sanctioned-exception
                    // argument, docs/03 §6/§8 rule 9). `effort` is `Copy`, captured per start.
                    multistart_start_imported(
                        problem,
                        qty,
                        seed,
                        budget,
                        k,
                        starts as u64,
                        strategy,
                        nfp_cache,
                        effort,
                        column_weight,
                        areas,
                    )
                })
            })
            .collect();
        // Join in spawn (k) order — the returned Vec is ordered by k regardless of finish order.
        handles
            .into_iter()
            .map(|h| h.join().expect("a multi-start nest thread panicked"))
            .collect()
    })
}

/// Nests `items` (demand `qty`) across several `sheets` in order: each sheet is filled with whatever
/// demand remains, then the rest spills to the next. Returns the placements per sheet plus the global
/// unplaced. Applies **one** allowed-rotation set to every item; for a distinct set per item type use
/// [`nest_multi_per_item`].
///
/// Determinism: each sheet is nested with a seed derived deterministically from `seed` and the sheet
/// index, so the whole result is byte-identical for the same arguments. `min_sep`, `rotations_deg`,
/// and `budget` carry the same meaning as in [`nest`].
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_multi(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    sheets: &[Sheet],
    min_sep: Scalar,
    rotations_deg: &[Scalar],
    seed: u64,
    budget: u64,
) -> MultiSheetSolution {
    nest_multi_multistart(items, qty, sheets, min_sep, rotations_deg, seed, budget, 1)
}

/// Like [`nest_multi`], but with a **distinct allowed-rotation set per item type** — `rotations_deg[k]`
/// applies to `items[k]` (`rotations_deg.len() == items.len()`). Carries the per-item semantics of
/// [`nest_per_item`] across the multi-sheet spill. Same determinism contract as [`nest_multi`].
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_multi_per_item(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    sheets: &[Sheet],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
) -> MultiSheetSolution {
    nest_multi_multistart_per_item(items, qty, sheets, min_sep, rotations_deg, seed, budget, 1)
}

/// [`nest_multi`] with **deterministic best-of-K multi-start applied to every sheet**: each sheet is
/// packed with the densest of `n_starts` decorrelated-seed runs *before* the leftover demand spills to
/// the next. Maximising each sheet's fill directly reduces what spills forward, so fewer sheets / less
/// material are used on a mixed-parts job. Applies **one** rotation set to every item; for a per-item
/// set use [`nest_multi_multistart_per_item`].
///
/// `n_starts` is clamped to ≥ 1; **`n_starts == 1` is byte-identical to [`nest_multi`]**, so existing
/// behaviour is unchanged. Same determinism contract — the per-sheet best-of-K is byte-deterministic
/// (see [`nest_multistart`]); under the `parallel` feature each sheet's K starts run on threads.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_multi_multistart(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    sheets: &[Sheet],
    min_sep: Scalar,
    rotations_deg: &[Scalar],
    seed: u64,
    budget: u64,
    n_starts: usize,
) -> MultiSheetSolution {
    let rotations_per_item = vec![rotations_deg.to_vec(); items.len()];
    nest_multi_multistart_per_item(
        items,
        qty,
        sheets,
        min_sep,
        &rotations_per_item,
        seed,
        budget,
        n_starts,
    )
}

/// [`nest_multi_multistart`] with a **distinct allowed-rotation set per item type** (the
/// [`nest_per_item`] semantics). Each sheet runs a best-of-`n_starts` pack against the remaining
/// demand; `n_starts == 1` reduces to [`nest_multi_per_item`] byte-for-byte.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nest_multi_multistart_per_item(
    items: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    sheets: &[Sheet],
    min_sep: Scalar,
    rotations_deg: &[Vec<Scalar>],
    seed: u64,
    budget: u64,
    n_starts: usize,
) -> MultiSheetSolution {
    assert_eq!(
        items.len(),
        qty.len(),
        "items and qty must be the same length"
    );
    assert_eq!(
        items.len(),
        rotations_deg.len(),
        "items and per-item rotations must be the same length"
    );

    let mut remaining = qty.to_vec();
    let mut per_sheet = Vec::with_capacity(sheets.len());

    for (i, sheet) in sheets.iter().enumerate() {
        // A distinct, deterministic per-sheet seed (SplitMix64 in the PRNG de-correlates adjacent
        // seeds, so `seed + i` gives well-separated streams). Each sheet's K best-of-K runs then
        // derive from that seed (`sheet_seed + k`); the K runs of a given sheet are decorrelated from
        // each other — the property best-of-K relies on — independent of the cross-sheet stride.
        let sheet_seed = seed.wrapping_add(i as u64);
        let sol = nest_multistart_per_item(
            items,
            &remaining,
            &sheet.outline,
            &sheet.holes,
            min_sep,
            rotations_deg,
            sheet_seed,
            budget,
            n_starts,
        );

        for p in &sol.placements {
            remaining[p.item] -= 1;
        }
        per_sheet.push(sol.placements);

        if remaining.iter().all(|&r| r == 0) {
            break; // everything placed — no need to touch the remaining sheets
        }
    }

    // Keep `per_sheet` index-parallel with `sheets`: any trailing sheets we broke out of (because
    // demand ran out) get an empty placement list, so `per_sheet[i]` is always valid for every sheet.
    per_sheet.resize_with(sheets.len(), Vec::new);

    let unplaced = remaining
        .iter()
        .enumerate()
        .flat_map(|(i, &n)| std::iter::repeat_n(i, n))
        .collect();

    MultiSheetSolution {
        per_sheet,
        unplaced,
    }
}

/// Greedily places every requested instance (largest-first) at its lowest-loss feasible pose.
/// `rotations_rad` is indexed by item id, so each type samples from its own orientation set.
#[allow(clippy::too_many_arguments)]
fn constructive_fill(
    layout: &mut Layout,
    entities: &[Option<Item>],
    order: &[usize],
    qty: &[usize],
    rotations_rad: &[Vec<Scalar>],
    prng: &mut Prng,
    budget: u64,
    placed_per_type: &mut [usize],
) {
    for &item_id in order {
        let item = entities[item_id].as_ref().unwrap();
        while placed_per_type[item_id] < qty[item_id] {
            match search::search(layout.cde(), item, &rotations_rad[item_id], prng, budget) {
                Some(d_transf) => {
                    improve::place_dropped(layout, item, d_transf);
                    placed_per_type[item_id] += 1;
                }
                None => break, // no remaining instance of this type fits anywhere
            }
        }
    }
}

/// Converts the layout's placed items into anchor-free [`Placement`]s.
fn extract_placements(layout: &Layout, entities: &[Option<Item>]) -> Vec<Placement> {
    layout
        .placed_items
        .values()
        .map(|pi| {
            let item = entities[pi.item_id].as_ref().unwrap();
            // The original outline's centroid = -(centering pre-transform translation).
            let (px, py) = item.shape_orig.pre_transform.translation();
            let centroid = (-px, -py);
            let (x, y, rotation_deg) = search::original_to_placed(&pi.d_transf, centroid);
            Placement {
                item: pi.item_id,
                x,
                y,
                rotation_deg,
            }
        })
        .collect()
}

/// Absolute polygon area (shoelace) of an item-local `outline`. Pure `+ − ×` ⇒ deterministic and
/// byte-identical cross-platform; used only to rank multi-start results by total placed area.
fn polygon_area(outline: &[[Scalar; 2]]) -> Scalar {
    let n = outline.len();
    if n < 3 {
        return 0.0;
    }
    let mut acc: Scalar = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        acc += outline[i][0] * outline[j][1] - outline[j][0] * outline[i][1];
    }
    (0.5 * acc).abs()
}

/// Every requested instance, marked unplaced (used when the container itself fails to import).
fn all_unplaced(qty: &[usize]) -> Vec<usize> {
    qty.iter()
        .enumerate()
        .flat_map(|(i, &n)| std::iter::repeat_n(i, n))
        .collect()
}

/// Re-export for callers that want to drive the per-item search directly.
pub use search::search;
