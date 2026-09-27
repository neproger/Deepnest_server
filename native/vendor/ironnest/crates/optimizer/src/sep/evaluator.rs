// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Sample evaluation: score one candidate pose for the item being moved.
//!
//! Ported/adapted from sparrow (`src/eval/{sample_eval,sep_evaluator}.rs`, MIT). The **CDE is the
//! feasibility arbiter** — [`CDEngine::collect_poly_collisions`] reports exactly which entities a
//! candidate pose collides with — and the [`super::proxy`] only *ranks* those collisions, weighted
//! by the GLS [`CollisionTracker`] weights. A pose with no collisions is [`SampleEval::Clear`];
//! otherwise [`SampleEval::Collision`] with the total weighted overlap.
//!
//! ironnest adaptation: the item being moved is **removed from the layout before** the search, so
//! there is no self-collision to filter (sparrow keeps it in and excludes it). The summation runs in
//! a **canonical order** (sorted [`HazKey`]) so the weighted loss is byte-identical regardless of
//! CDE traversal order — which is also what re-admits sparrow's `upper_bound` early-bailout
//! deterministically (see [`SeparationEvaluator::evaluate_sample`]). `+ − × ÷ sqrt` only.

use super::proxy::{
    quantify_collision_poly_container, quantify_collision_poly_hole, quantify_collision_poly_poly,
};
use super::tracker::CollisionTracker;
use ironnest_cde::collision_detection::hazards::collector::BasicHazardCollector;
use ironnest_cde::collision_detection::hazards::{HazKey, HazardEntity};
use ironnest_cde::entities::{Item, Layout, PItemKey};
use ironnest_cde::geometry::geo_enums::GeoRelation;
use ironnest_cde::geometry::primitives::{Rect, SPolygon};
use ironnest_geo::geo_traits::TransformableFrom;
use ironnest_geo::{DTransformation, Scalar};
use std::cmp::Ordering;

/// The outcome of evaluating a candidate pose. Ordered worst-last: any [`Self::Clear`] beats any
/// [`Self::Collision`], which beats [`Self::Invalid`]; within a variant, lower `loss` is better.
#[derive(Clone, Copy, Debug)]
pub enum SampleEval {
    /// No collisions — a feasible pose (`loss` is always `0.0`).
    Clear { loss: Scalar },
    /// Collides — `loss` is the total weighted overlap proxy.
    Collision { loss: Scalar },
    /// Not a usable pose (e.g. produced by a degenerate sampler). Sorts as worst.
    Invalid,
}

impl SampleEval {
    /// The comparable loss key (`+∞` for [`Self::Invalid`]). Uses [`Scalar::total_cmp`] downstream
    /// so the order is a deterministic total order (no rounding, no `NaN` ambiguity).
    fn rank(self) -> (u8, Scalar) {
        match self {
            SampleEval::Clear { loss } => (0, loss),
            SampleEval::Collision { loss } => (1, loss),
            SampleEval::Invalid => (2, Scalar::INFINITY),
        }
    }

    /// The weighted-overlap sum above which a `Collision` candidate can no longer rank better than
    /// `self` — the early-terminate bound for [`SeparationEvaluator::evaluate_sample`]. A `Clear`
    /// bound is unbeatable by any collision (threshold 0); an `Invalid` bound rejects nothing
    /// (threshold ∞).
    fn collision_threshold(self) -> Scalar {
        match self {
            SampleEval::Clear { .. } => 0.0,
            SampleEval::Collision { loss } => loss,
            SampleEval::Invalid => Scalar::INFINITY,
        }
    }
}

impl Ord for SampleEval {
    // DETERMINISM(ironnest): we use the exact `Scalar::total_cmp` (a total order over all f64 bit
    // patterns) where sparrow uses an `FPA`-rounded `partial_cmp`. This is *intentionally* different:
    // total_cmp is byte-deterministic and never panics on a hypothetical NaN, but it does change
    // tie-breaks vs the reference, so a future "why doesn't this match sparrow?" is expected here.
    fn cmp(&self, other: &Self) -> Ordering {
        let (a_tag, a_loss) = self.rank();
        let (b_tag, b_loss) = other.rank();
        a_tag.cmp(&b_tag).then_with(|| a_loss.total_cmp(&b_loss))
    }
}

impl PartialOrd for SampleEval {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SampleEval {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SampleEval {}

/// Evaluates candidate poses for one item against a layout the item has been **removed** from.
pub struct SeparationEvaluator<'a> {
    layout: &'a Layout,
    tracker: &'a CollisionTracker,
    item: &'a Item,
    /// The item's (pre-move) key — still valid in the tracker for weight lookups.
    current_pk: PItemKey,
    container_bbox: Rect,
    /// Scratch shape (keeps its surrogate, which the proxy needs), transformed per candidate.
    shape_buff: SPolygon,
    /// Scratch collision collector, reused (cleared, capacity retained) across every
    /// [`Self::evaluate_sample`] call instead of allocating a fresh one per candidate pose — the
    /// separator evaluates ~hundreds of poses per move. Reuse is byte-identical: the canonical
    /// `hazards` sort below makes the summed loss independent of this collector's contents/order.
    collector: BasicHazardCollector,
    /// Scratch `(HazKey, HazardEntity)` buffer, reused per candidate (sorted in place each time so
    /// the quantify-and-sum loop runs in canonical order regardless of CDE traversal order).
    hazards: Vec<(HazKey, HazardEntity)>,
}

impl<'a> SeparationEvaluator<'a> {
    #[must_use]
    pub fn new(
        layout: &'a Layout,
        tracker: &'a CollisionTracker,
        item: &'a Item,
        current_pk: PItemKey,
    ) -> Self {
        let cap = layout.placed_items.len() + 1;
        Self {
            layout,
            tracker,
            item,
            current_pk,
            container_bbox: layout.container.outer_cd.bbox,
            shape_buff: (*item.shape_cd).clone(),
            collector: BasicHazardCollector::with_capacity(cap),
            hazards: Vec::with_capacity(cap),
        }
    }

    /// Scores the pose `dt`: transform the item there, ask the CDE which hazards it hits, and sum
    /// their weighted overlap proxy in canonical [`HazKey`] order.
    ///
    /// `bound` enables the **deterministic early-terminate** (sparrow's `upper_bound` bailout,
    /// re-admitted now that the summation order is canonical): the running partial sum is compared
    /// against the bound's collision threshold after every term and the remaining (expensive)
    /// pole-pair quantifications are skipped once the candidate provably cannot rank better than
    /// `bound`. Because every term is strictly positive and the bailout is **strict-greater**, a
    /// truncated result is returned only when it (and therefore the full sum) already exceeds the
    /// bound — callers compare it against that same bound and discard it, so every downstream
    /// accept/reject/store decision is **identical** to the full summation, and any value that is
    /// actually kept is an exact, fully-summed loss. The truncation point is a pure function of the
    /// canonical term order and byte-identical partial sums, so it is cross-platform deterministic.
    /// Pass [`SampleEval::Invalid`] (threshold ∞) to force an exact evaluation.
    pub fn evaluate_sample(&mut self, dt: DTransformation, bound: SampleEval) -> SampleEval {
        let t = dt.compose();
        self.shape_buff.transform_from(&self.item.shape_cd, &t);

        // Reject any pose the CDE quadtree cannot hold: a shape whose bbox is not fully surrounded by
        // the quadtree root bbox is *unplaceable* (`Layout::place_item` would register a hazard
        // outside all quadrants — a debug panic / invariant break). The coordinate descent moves
        // freely in x/y, so it can wander here; marking it `Invalid` (worst rank) guarantees the
        // search never *returns* such a pose. Poses inside the (inflated-square) quadtree but outside
        // the container are still placeable — they score as an `Exterior` collision below, which the
        // separator then pushes back in. Pure bbox relation → determinism-safe.
        if self.layout.cde().bbox().relation_to(self.shape_buff.bbox) != GeoRelation::Surrounding {
            return SampleEval::Invalid;
        }

        // Reuse the scratch collector (cleared, capacity retained) instead of allocating per sample.
        self.collector.clear();
        self.layout
            .cde()
            .collect_poly_collisions(&self.shape_buff, &mut self.collector);

        if self.collector.is_empty() {
            return SampleEval::Clear { loss: 0.0 };
        }

        // Canonical summation with early terminate: gather the colliding (HazKey, HazardEntity)
        // pairs into the reused `hazards` buffer, sort by key (cheap — no quantification yet), then
        // quantify-and-accumulate in that canonical order, abandoning the candidate once the partial
        // sum strictly exceeds the bound's threshold. The total is independent of the CDE's traversal
        // order and byte-identical across platforms. (Destructure so the loop borrows only the
        // read-only fields, leaving `hazards`/`collector` free.)
        let threshold = bound.collision_threshold();
        let Self {
            layout,
            tracker,
            current_pk,
            container_bbox,
            shape_buff,
            collector,
            hazards,
            ..
        } = self;
        hazards.clear();
        hazards.extend(collector.iter().map(|(hkey, haz)| (hkey, *haz)));
        hazards.sort_unstable_by_key(|(hkey, _)| *hkey);

        let mut loss: Scalar = 0.0;
        for (hkey, haz) in &*hazards {
            let term = match haz {
                HazardEntity::PlacedItem { pk: other_pk, .. } => {
                    let other_shape = &layout.placed_items[*other_pk].shape;
                    let pair_loss = quantify_collision_poly_poly(other_shape, shape_buff);
                    pair_loss * tracker.pair_weight(*current_pk, *other_pk)
                }
                HazardEntity::Exterior => {
                    let cont_loss = quantify_collision_poly_container(shape_buff, *container_bbox);
                    cont_loss * tracker.container_weight(*current_pk)
                }
                // A hole / keep-out zone the part must avoid (the interior-void path); shares the
                // item's single static-hazard GLS weight with the exterior.
                HazardEntity::Hole { .. } | HazardEntity::InferiorQualityZone { .. } => {
                    let hole_shape = &layout.cde().hazards_map[*hkey].shape;
                    let hole_loss = quantify_collision_poly_hole(shape_buff, hole_shape);
                    hole_loss * tracker.container_weight(*current_pk)
                }
            };
            loss += term;
            if loss > threshold {
                // Cannot rank better than `bound` (terms are strictly positive, so the full sum is
                // ≥ this partial). Truncated — valid only for rejection against `bound`.
                return SampleEval::Collision { loss };
            }
        }
        SampleEval::Collision { loss }
    }
}

/// Reusable scratch for [`unweighted_overlap`] — the seed sampler evaluates hundreds of poses per
/// insertion, so the collector and hazard buffer are allocated once by the caller and reused.
#[derive(Debug, Default)]
pub struct OverlapScratch {
    collector: BasicHazardCollector,
    hazards: Vec<(HazKey, HazardEntity)>,
}

/// The **unweighted** overlap of `shape` against everything currently in `layout` (no item is
/// excluded — used to seed a not-yet-placed part at its lowest-overlap pose, where there is no GLS
/// weight row yet). Quantified and summed in canonical [`HazKey`] order. Returns `0.0` for a
/// collision-free pose.
///
/// `bound` is the same **strict-greater early-terminate** as
/// [`SeparationEvaluator::evaluate_sample`]: once the partial sum exceeds `bound` the remaining
/// quantifications are skipped and the (truncated, still `> bound`) partial is returned — callers
/// compare against that same bound, so decisions are identical to the full sum. Pass
/// [`Scalar::INFINITY`] for an exact evaluation.
#[must_use]
pub fn unweighted_overlap(
    layout: &Layout,
    shape: &SPolygon,
    bound: Scalar,
    scratch: &mut OverlapScratch,
) -> Scalar {
    scratch.collector.clear();
    layout
        .cde()
        .collect_poly_collisions(shape, &mut scratch.collector);
    if scratch.collector.is_empty() {
        return 0.0;
    }
    let container_bbox = layout.container.outer_cd.bbox;
    scratch.hazards.clear();
    scratch
        .hazards
        .extend(scratch.collector.iter().map(|(hkey, haz)| (hkey, *haz)));
    scratch.hazards.sort_unstable_by_key(|(hkey, _)| *hkey);

    let mut loss: Scalar = 0.0;
    for (hkey, haz) in &scratch.hazards {
        let term = match haz {
            HazardEntity::PlacedItem { pk: other_pk, .. } => {
                quantify_collision_poly_poly(&layout.placed_items[*other_pk].shape, shape)
            }
            HazardEntity::Exterior => quantify_collision_poly_container(shape, container_bbox),
            HazardEntity::Hole { .. } | HazardEntity::InferiorQualityZone { .. } => {
                quantify_collision_poly_hole(shape, &layout.cde().hazards_map[*hkey].shape)
            }
        };
        loss += term;
        if loss > bound {
            return loss; // truncated — valid only for rejection against `bound`
        }
    }
    loss
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)] // exact-value mapping IS the property under test (determinism)
    fn collision_threshold_maps_bounds() {
        // Clear is unbeatable by any collision → 0; Collision bounds by its loss; Invalid bounds
        // nothing (∞ ⇒ exact evaluation).
        assert_eq!(SampleEval::Clear { loss: 0.0 }.collision_threshold(), 0.0);
        assert_eq!(
            SampleEval::Collision { loss: 3.5 }.collision_threshold(),
            3.5
        );
        assert_eq!(SampleEval::Invalid.collision_threshold(), Scalar::INFINITY);
    }

    #[test]
    fn sample_eval_total_order() {
        let clear = SampleEval::Clear { loss: 0.0 };
        let cheap = SampleEval::Collision { loss: 1.0 };
        let dear = SampleEval::Collision { loss: 2.0 };
        let invalid = SampleEval::Invalid;

        // Clear beats any collision beats Invalid; within collisions, lower loss wins.
        assert!(clear < cheap);
        assert!(cheap < dear);
        assert!(dear < invalid);
        assert!(clear < invalid);
        // Reflexive equality (so dedup / first-min ties are well-defined).
        assert!(cheap == SampleEval::Collision { loss: 1.0 });
        assert_eq!(invalid.cmp(&invalid), Ordering::Equal);
    }
}
