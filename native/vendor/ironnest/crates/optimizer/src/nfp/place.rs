// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The NFP constructive placer (docs/03 §5, N2): per part, assemble the feasible region (one flat
//! boolean over pristine cached geometry), walk the integer candidates ascending by the exact
//! linear loss, place the first CDE-verified pose **directly** (no bottom-left slide — the region
//! already encodes contact, and the slide would smear the deliberate □E backoff and burn 60–240
//! CDE queries per placement re-deriving it), and fall back to today's sampling path verbatim
//! (`search::search` + `place_dropped`, slide included — fallback parts carry no backoff) when the
//! region is empty or every candidate is rejected.
//!
//! DETERMINISM(ironnest): candidate order is the exact integer `(cost, rot_idx, x, y)`
//! lexicographic walk; the placer itself draws NO PRNG — only the sampling fallback does, and its
//! triggering is a pure function of byte-deterministic integer regions + CDE verdicts (docs/03
//! §5.3). Verify rejections advance deterministically to the next candidate and are counted.

use ironnest_cde::entities::{Item, Layout};
use ironnest_geo::primitives::SPolygon;
use ironnest_geo::{DTransformation, Scalar};

use super::geom::{quantize, unquantize};
use super::{NfpCache, PlacedObstacle, candidates};
use crate::prng::Prng;
use crate::{improve, search};

/// Diagnostic counters for the NFP pass (docs/03 §9.4 op-count gates; §10 rejection target).
#[derive(Debug, Default, Clone, Copy)]
pub struct NfpPlaceStats {
    /// Candidates rejected by the CDE arbiter (structural target: 0 — each is an ε-budget datum).
    pub verify_rejections: u64,
    /// Parts placed by the sampling fallback (region empty / all candidates rejected).
    pub fallback_placements: u64,
    /// Parts placed at NFP candidates.
    pub nfp_placements: u64,
}

/// One NFP construction pass over `order` (docs/03 §5.3 runs this twice: construction + one
/// refill): for each type, place instances at the best verified NFP candidate until the type's
/// demand is met or nothing fits (NFP and fallback both exhausted ⇒ break the type, matching the
/// sampling `constructive_fill` semantics). Updates `placed`/`placed_per_type` in lockstep with
/// the layout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_pass(
    layout: &mut Layout,
    entities: &[Option<Item>],
    order: &[usize],
    qty: &[usize],
    rotations_rad: &[Vec<Scalar>],
    cache: &NfpCache,
    prng: &mut Prng,
    budget: u64,
    placed: &mut Vec<PlacedObstacle>,
    placed_per_type: &mut [usize],
    stats: &mut NfpPlaceStats,
    column_weight: u32,
) {
    for &t in order {
        let Some(item) = entities[t].as_ref() else {
            continue;
        };
        // Scratch collision shape, cloned ONCE per type (not per instance): every instance of `t`
        // shares the same base geometry, and the buffer is fully overwritten by `transform_from` on
        // each pose test, so reuse is byte-identical — it only removes a per-placement clone.
        let mut buffer = (*item.shape_cd).clone();
        buffer.surrogate = None;
        while placed_per_type[t] < qty[t] {
            if try_place_one(
                layout,
                item,
                t,
                rotations_rad,
                cache,
                prng,
                budget,
                placed,
                stats,
                &mut buffer,
                column_weight,
            ) {
                placed_per_type[t] += 1;
            } else {
                break; // neither NFP nor fallback fits this type — further copies can't either
            }
        }
    }
}

/// Rebuilds the integer obstacle list from the live layout (post-separation / post-densify).
/// Used by the post-sep NFP refill and the local re-pack compact (docs/03 §13 Q5, docs/04).
pub(crate) fn obstacles_from_layout(
    layout: &Layout,
    rotations_rad: &[Vec<Scalar>],
) -> Vec<PlacedObstacle> {
    let mut out = Vec::with_capacity(layout.placed_items.len());
    for pi in layout.placed_items.values() {
        let rot = pi.d_transf.rotation();
        // Rotations on placed items are bit-identical to a member of the allowed set.
        #[allow(clippy::float_cmp)] // exact identity by construction
        let rot_idx = rotations_rad[pi.item_id]
            .iter()
            .position(|&v| v == rot)
            .expect("placed rotation must come from the allowed set");
        let (tx, ty) = pi.d_transf.translation();
        out.push(PlacedObstacle {
            item_id: pi.item_id,
            rot_idx,
            tx: quantize(tx),
            ty: quantize(ty),
        });
    }
    out
}

/// Places one instance of type `t`: NFP candidate walk first, sampling fallback second.
/// Returns whether an instance was placed.
#[allow(clippy::too_many_arguments)]
fn try_place_one(
    layout: &mut Layout,
    item: &Item,
    t: usize,
    rotations_rad: &[Vec<Scalar>],
    cache: &NfpCache,
    prng: &mut Prng,
    budget: u64,
    placed: &mut Vec<PlacedObstacle>,
    stats: &mut NfpPlaceStats,
    buffer: &mut SPolygon,
    column_weight: u32,
) -> bool {
    // Candidates across the allowed rotations, merged under the exact lexicographic tie-break
    // (cost, rot_idx, x, y) — docs/03 §5.2.
    let n_rots = rotations_rad[t].len();
    let mut cands: Vec<(i64, usize, i64, i64)> = Vec::new();
    for r in 0..n_rots {
        let region = cache.feasible_region(t, r, placed);
        let rot_cands = candidates(&region, cache.loss_offsets(t, r), column_weight);
        cands.reserve(rot_cands.len());
        for (cost, x, y) in rot_cands {
            cands.push((cost, r, x, y));
        }
    }
    cands.sort_unstable();

    for &(_, r, x, y) in &cands {
        let (fx, fy) = (unquantize(x), unquantize(y));
        if search::feasible_at(layout.cde(), item, buffer, rotations_rad[t][r], fx, fy) {
            layout.place_item(item, DTransformation::new(rotations_rad[t][r], (fx, fy)));
            placed.push(PlacedObstacle {
                item_id: t,
                rot_idx: r,
                tx: x,
                ty: y,
            });
            stats.nfp_placements += 1;
            return true;
        }
        stats.verify_rejections += 1;
    }

    // Sampling fallback — today's constructive path verbatim (search + drop-slide); the settled
    // pose is quantized ONCE (the second sanctioned quantization site, docs/03 §8 rule 2) so the
    // part participates in subsequent region queries as an obstacle.
    if budget == 0 {
        return false;
    }
    if let Some(transf) = search::search(layout.cde(), item, &rotations_rad[t], prng, budget) {
        let settled = improve::place_dropped(layout, item, transf);
        let rot = settled.rotation();
        // The sampled rotation is bit-identical to a member of the allowed set (the sampler picks
        // FROM the set); map it back to its index for the obstacle key.
        #[allow(clippy::float_cmp)] // exact identity by construction — see comment above
        let rot_idx = rotations_rad[t]
            .iter()
            .position(|&v| v == rot)
            .expect("sampled rotation must come from the allowed set");
        let (tx, ty) = settled.translation();
        placed.push(PlacedObstacle {
            item_id: t,
            rot_idx,
            tx: quantize(tx),
            ty: quantize(ty),
        });
        stats.fallback_placements += 1;
        return true;
    }
    false
}
