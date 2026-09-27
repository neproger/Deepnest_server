// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `nfp` — the exact no-fit-polygon construction substrate (feature #4, docs/03; N1).
//!
//! Layering (leaf → root): [`geom`] (integer primitives + canonicalization) → [`minkowski`]
//! (edge-merge fast path, orientation-normalized convolution union-fill, □E inflation, frame-trick
//! IFP) → this module (the **[`NfpCache`]**: per-(type, rotation) quantized footprints, IFPs with
//! keep-out zones folded in, eagerly-built pairwise NFPs with mirror derivation, and the
//! per-placement **feasible-region query**). The placer that consumes the region (candidate walk +
//! CDE verification + `PlacementStrategy` wiring) is N2.
//!
//! DETERMINISM(ironnest): pure integer geometry after exactly two quantization sites (shape
//! vertices at build; off-grid obstacle poses at query — docs/03 §8 rule 2); `i128` for every
//! self-written predicate (rule 4); cardinal rotations by exact swap/negate (rule 3); the i64
//! i_overlay engine with default (single-threaded) solver (rule 5); every cached or returned shape
//! canonicalized (rule 6); `BTreeMap` keyed by rotation INDICES; no PRNG anywhere. The cache has
//! no interior mutability: built once per call, it crosses the sanctioned multi-start threads as
//! immutable `&NfpCache` (rule 9).
//!
//! Doc-hidden pub: exposed only so the integration test suite (`tests/nfp_props.rs`) can drive the
//! substrate directly; NOT a stable API.

pub mod geom;
pub mod minkowski;
pub(crate) mod place;

use std::collections::BTreeMap;
use std::fmt;

use ironnest_cde::entities::{Container, Item};
use ironnest_geo::Scalar;
use ironnest_geo::Transformation;
use ironnest_geo::geo_traits::Transformable;

use geom::{IPt, Ring, canonicalize_shapes, quantize, reflect, translate};
use minkowski::{Shapes, difference, difference_query, ifp_frame_trick, inflate_box, nfp_pair};

/// Contact backoff □E half-width in grid units (docs/03 §4): initial 16 ≈ 1.5e-5 input units —
/// ≥ 4× every identified representation-error term, 4 orders below the consumer's part gap, and
/// freely raisable (even 16× stays under the acceptance harness's resolution).
pub const NFP_BACKOFF_E: i64 = 16;

/// Upper bound for the frame-trick ring width, grid units (2^18 = 0.25 input unit). The effective
/// margin is `min(this, min-part-min-dimension / 2)` — strictly thinner than any part, which is
/// what licenses omitting the A-seed for the holed frame shape (docs/03 §11.1 lesson).
pub const NFP_FRAME_MARGIN_MAX: i64 = 1 << 18;

/// Domain gate: every coordinate entering a boolean op must stay far inside the i64 engine's
/// exact-arithmetic range. 2^61 leaves > 30 bits of headroom over any realistic input (docs/03
/// §3.1; the review-corrected bound).
const MAX_OP_COORD: i64 = 1 << 61;

/// NFP-mode build errors. The mode HARD-ERRORS on out-of-domain input rather than silently
/// degrading (mode fallback would create density cliffs — docs/03 §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NfpError {
    /// The derived worst-case boolean-op coordinate exceeds the validated domain.
    InputDomain { worst_case_grid: i64 },
}

impl fmt::Display for NfpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NfpError::InputDomain { worst_case_grid } => write!(
                f,
                "NFP mode input domain exceeded: worst-case scaled coordinate {worst_case_grid} \
                 >= 2^61 grid units — shrink the problem coordinates (docs/03 §3.1)"
            ),
        }
    }
}

/// A placed obstacle for the region query: item type, rotation-set index, and its pose translation
/// on the grid (poses placed by the NFP placer are grid-exact; fallback/sep poses are quantized
/// once by the caller — the second sanctioned quantization site).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedObstacle {
    pub item_id: usize,
    pub rot_idx: usize,
    pub tx: i64,
    pub ty: i64,
}

/// The CONTAINER-INDEPENDENT half of the NFP cache (docs/03 §6): quantized footprints,
/// □E-inflated movers, loss offsets, and the full pairwise NFP table. In a multi-sheet nest this
/// is built ONCE and shared (via `Arc`) across every sheet; only the IFPs are per-sheet.
pub struct PartsGeometry {
    /// `[type][rot_idx]` → quantized CD footprint (canonical CCW). `None` for failed imports or
    /// out-of-range rotation indices.
    foot: Vec<Vec<Option<Ring>>>,
    /// `[type][rot_idx]` → footprint ⊕ □E (the mover form).
    foot_e: Vec<Vec<Option<Ring>>>,
    /// `[type][rot_idx]` → integer `(x_max, y_max)` of the UNINFLATED footprint — the loss offsets.
    bbox_max: Vec<Vec<(i64, i64)>>,
    /// `(placed_type, placed_rot, moving_type, moving_rot)` → NFP with the MOVING side inflated.
    pairs: BTreeMap<(u16, u8, u16, u8), Shapes>,
    /// Max |coordinate| over all footprints (grid) — the parts term of the domain gate.
    max_part: i64,
    /// Min bbox dimension over all footprints (grid) — bounds the frame margin.
    min_part_dim: i64,
}

/// The per-problem NFP cache: shared [`PartsGeometry`] + this container's IFPs (quality-zone
/// keep-outs folded in). Immutable after build; crosses the sanctioned multi-start threads as
/// `&NfpCache` (docs/03 §8 rule 9).
pub struct NfpCache {
    parts: std::sync::Arc<PartsGeometry>,
    /// `[type][rot_idx]` → feasible container region for the □E-inflated mover (zones folded in).
    ifp: Vec<Vec<Option<Shapes>>>,
}

impl PartsGeometry {
    /// Builds the container-independent geometry. `rotations_deg`/`rotations_rad` are the
    /// per-item normalized sets (parallel to `entities`); degrees drive the exact-cardinal fast
    /// path, radians the libm path for arbitrary angles — the SAME radian values the CDE verify
    /// transform uses, so both sides see identical rotated f64 vertices (docs/03 §3.1).
    ///
    /// Deterministic: iteration in ascending (type, rot) / key order throughout; no PRNG.
    #[allow(clippy::too_many_lines)] // one linear build pipeline; splitting would obscure the order
    #[allow(clippy::float_cmp)] // INTENTIONAL exact-literal dispatch: only the exact user-supplied
    // cardinal degree literals take the swap/negate path; anything else (89.999…, 45.0) falls to
    // the libm path — correctness never depends on the comparison matching.
    pub fn build(
        entities: &[Option<Item>],
        rotations_deg: &[Vec<Scalar>],
        rotations_rad: &[Vec<Scalar>],
    ) -> Result<Self, NfpError> {
        let n_types = entities.len();

        // ---- Quantized footprints per (type, rotation index). ----
        let mut foot: Vec<Vec<Option<Ring>>> = Vec::with_capacity(n_types);
        for (t, ent) in entities.iter().enumerate() {
            let n_rots = rotations_deg.get(t).map_or(0, Vec::len);
            let mut per_rot: Vec<Option<Ring>> = Vec::with_capacity(n_rots);
            match ent {
                None => per_rot.resize(n_rots, None),
                Some(item) => {
                    // Cardinal path: quantize the 0° footprint once, derive 90/180/270 by exact
                    // integer swap/negate (docs/03 §8 rule 3).
                    let zero: Ring = item
                        .shape_cd
                        .vertices
                        .iter()
                        .map(|p| IPt::new(quantize(p.0), quantize(p.1)))
                        .collect();
                    for r in 0..n_rots {
                        let deg = rotations_deg[t][r];
                        let ring = if deg == 0.0 {
                            zero.clone()
                        } else if deg == 90.0 {
                            zero.iter().map(|p| IPt::new(-p.y, p.x)).collect()
                        } else if deg == 180.0 {
                            zero.iter().map(|p| IPt::new(-p.x, -p.y)).collect()
                        } else if deg == 270.0 {
                            zero.iter().map(|p| IPt::new(p.y, -p.x)).collect()
                        } else {
                            // Arbitrary angle: libm-rotate the f64 vertices (the existing
                            // deterministic trig rule), then quantize.
                            let rot = Transformation::from_rotation(rotations_rad[t][r]);
                            item.shape_cd
                                .vertices
                                .iter()
                                .map(|p| {
                                    let mut q = *p;
                                    q.transform(&rot);
                                    IPt::new(quantize(q.0), quantize(q.1))
                                })
                                .collect()
                        };
                        per_rot.push(Some(ring));
                    }
                }
            }
            foot.push(per_rot);
        }

        // ---- Parts-side domain gate (docs/03 §3.1); the container side is checked in
        // `NfpCache::assemble` (per sheet). Must run BEFORE any Minkowski arithmetic below.
        let mut max_part = 0i64;
        let mut min_part_dim = i64::MAX;
        for per_rot in &foot {
            for ring in per_rot.iter().flatten() {
                let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
                for p in ring {
                    max_part = max_part.max(p.x.abs()).max(p.y.abs());
                    x0 = x0.min(p.x);
                    y0 = y0.min(p.y);
                    x1 = x1.max(p.x);
                    y1 = y1.max(p.y);
                }
                min_part_dim = min_part_dim.min((x1 - x0).min(y1 - y0));
            }
        }
        let parts_worst = (2i64.saturating_mul(max_part)).saturating_add(NFP_BACKOFF_E);
        if parts_worst >= MAX_OP_COORD {
            return Err(NfpError::InputDomain {
                worst_case_grid: parts_worst,
            });
        }

        // ---- □E-inflated movers + integer loss offsets. ----
        let mut foot_e: Vec<Vec<Option<Ring>>> = Vec::with_capacity(n_types);
        let mut bbox_max: Vec<Vec<(i64, i64)>> = Vec::with_capacity(n_types);
        for per_rot in &foot {
            let mut fe: Vec<Option<Ring>> = Vec::with_capacity(per_rot.len());
            let mut bm: Vec<(i64, i64)> = Vec::with_capacity(per_rot.len());
            for ring in per_rot {
                match ring {
                    None => {
                        fe.push(None);
                        bm.push((0, 0));
                    }
                    Some(r) => {
                        fe.push(Some(inflate_box(r, NFP_BACKOFF_E)));
                        let (mut mx, mut my) = (i64::MIN, i64::MIN);
                        for p in r {
                            mx = mx.max(p.x);
                            my = my.max(p.y);
                        }
                        bm.push((mx, my));
                    }
                }
            }
            foot_e.push(fe);
            bbox_max.push(bm);
        }

        // ---- Pairwise NFPs, eager, ascending key order; mirrors derived by reflection. ----
        let mut pairs: BTreeMap<(u16, u8, u16, u8), Shapes> = BTreeMap::new();
        #[allow(clippy::cast_possible_truncation)] // type/rot counts are tiny (u16/u8 by contract)
        #[allow(clippy::needless_range_loop)]
        // indices key FOUR parallel tables + the BTreeMap key
        for ta in 0..n_types {
            for ra in 0..foot[ta].len() {
                if foot[ta][ra].is_none() {
                    continue;
                }
                for tb in 0..n_types {
                    for rb in 0..foot[tb].len() {
                        if foot_e[tb][rb].is_none() {
                            continue;
                        }
                        let key = (ta as u16, ra as u8, tb as u16, rb as u8);
                        let mirror = (tb as u16, rb as u8, ta as u16, ra as u8);
                        if pairs.contains_key(&key) {
                            continue;
                        }
                        let nfp = nfp_pair(
                            foot[ta][ra].as_ref().unwrap(),
                            foot_e[tb][rb].as_ref().unwrap(),
                        );
                        // Mirror: NFP(B←A) = −NFP(A←B), exact for the symmetric □E (docs/03 §6).
                        if mirror != key && !pairs.contains_key(&mirror) {
                            let reflected: Shapes = nfp
                                .iter()
                                .map(|shape| shape.iter().map(|ring| reflect(ring)).collect())
                                .collect();
                            pairs.insert(mirror, canonicalize_shapes(reflected));
                        }
                        pairs.insert(key, nfp);
                    }
                }
            }
        }

        Ok(Self {
            foot,
            foot_e,
            bbox_max,
            pairs,
            max_part,
            min_part_dim,
        })
    }
}

impl NfpCache {
    /// Builds the full cache for one container: [`PartsGeometry::build`] + [`NfpCache::assemble`].
    pub fn build(
        entities: &[Option<Item>],
        rotations_deg: &[Vec<Scalar>],
        rotations_rad: &[Vec<Scalar>],
        container: &Container,
    ) -> Result<Self, NfpError> {
        let parts = std::sync::Arc::new(PartsGeometry::build(
            entities,
            rotations_deg,
            rotations_rad,
        )?);
        Self::assemble(parts, container)
    }

    /// Assembles the per-container cache over shared [`PartsGeometry`] — this container's domain
    /// gate, frame margin, and IFPs (quality-zone keep-outs folded in). In a multi-sheet nest the
    /// parts are built once and this runs per sheet (docs/03 §6).
    pub fn assemble(
        parts: std::sync::Arc<PartsGeometry>,
        container: &Container,
    ) -> Result<Self, NfpError> {
        let container_int: Ring = container
            .outer_cd
            .vertices
            .iter()
            .map(|p| IPt::new(quantize(p.0), quantize(p.1)))
            .collect();
        let mut max_container = 0i64;
        for p in &container_int {
            max_container = max_container.max(p.x.abs()).max(p.y.abs());
        }
        let frame_margin = NFP_FRAME_MARGIN_MAX.min((parts.min_part_dim / 2).max(1));
        let worst_case = max_container
            .saturating_add(2i64.saturating_mul(parts.max_part))
            .saturating_add(NFP_BACKOFF_E)
            .saturating_add(frame_margin);
        if worst_case >= MAX_OP_COORD {
            return Err(NfpError::InputDomain {
                worst_case_grid: worst_case,
            });
        }

        // IFPs (quads-only face-classified frame trick), zone keep-outs folded in per (type, rot).
        let zones_int: Vec<Ring> = container.quality_zones[0]
            .iter()
            .flat_map(|z| &z.shapes_cd)
            .map(|s| {
                s.vertices
                    .iter()
                    .map(|p| IPt::new(quantize(p.0), quantize(p.1)))
                    .collect()
            })
            .collect();
        let mut ifp: Vec<Vec<Option<Shapes>>> = Vec::with_capacity(parts.foot_e.len());
        for per_rot in &parts.foot_e {
            let mut per: Vec<Option<Shapes>> = Vec::with_capacity(per_rot.len());
            for mover in per_rot {
                per.push(mover.as_ref().map(|m| {
                    let base = ifp_frame_trick(&container_int, m, frame_margin);
                    if zones_int.is_empty() {
                        base
                    } else {
                        // Each keep-out zone is an ordinary obstacle NFP subtracted once, at build.
                        let zone_nfps: Vec<Vec<Ring>> = zones_int
                            .iter()
                            .map(|z| nfp_pair(z, m).into_iter().flatten().collect::<Vec<Ring>>())
                            .collect();
                        difference(&base, &zone_nfps)
                    }
                }));
            }
            ifp.push(per);
        }

        Ok(Self { parts, ifp })
    }

    /// The feasible region for `(item_id, rot_idx)` given the placed obstacles: IFP ∖ ⋃ translated
    /// pair NFPs — ONE flat boolean pass over pristine cached geometry (docs/03 §5.1). Query-path
    /// cleanup only (zero-area drop) — the placer's [`candidates`] re-sorts vertices by cost, so
    /// full Booth canonicalization is waste on this throwaway region (docs/04 1c). Empty when the
    /// part cannot be placed at this rotation.
    #[must_use]
    pub fn feasible_region(
        &self,
        item_id: usize,
        rot_idx: usize,
        placed: &[PlacedObstacle],
    ) -> Shapes {
        let Some(Some(ifp)) = self.ifp.get(item_id).map(|per| &per[rot_idx]) else {
            return Vec::new();
        };
        #[allow(clippy::cast_possible_truncation)]
        let clips: Vec<Vec<Ring>> = placed
            .iter()
            .filter_map(|pl| {
                let key = (
                    pl.item_id as u16,
                    pl.rot_idx as u8,
                    item_id as u16,
                    rot_idx as u8,
                );
                self.parts.pairs.get(&key).map(|nfp| {
                    nfp.iter()
                        .flatten()
                        .map(|ring| translate(ring, pl.tx, pl.ty))
                        .collect::<Vec<Ring>>()
                })
            })
            .collect();
        if clips.is_empty() {
            return ifp.clone();
        }
        difference_query(ifp, &clips)
    }

    /// Integer LBF loss offsets for `(item_id, rot_idx)`: the uninflated footprint's `(x_max, y_max)`.
    #[must_use]
    pub fn loss_offsets(&self, item_id: usize, rot_idx: usize) -> (i64, i64) {
        self.parts.bbox_max[item_id][rot_idx]
    }

    /// The quantized footprint (test/diagnostic access).
    #[must_use]
    pub fn footprint(&self, item_id: usize, rot_idx: usize) -> Option<&Ring> {
        self.parts
            .foot
            .get(item_id)
            .and_then(|per| per.get(rot_idx))
            .and_then(Option::as_ref)
    }

    /// The □E-inflated mover footprint (test/diagnostic access).
    #[must_use]
    pub fn mover(&self, item_id: usize, rot_idx: usize) -> Option<&Ring> {
        self.parts
            .foot_e
            .get(item_id)
            .and_then(|per| per.get(rot_idx))
            .and_then(Option::as_ref)
    }

    /// Number of stored pair entries (diagnostic; the op-count-ceiling gates read this).
    #[must_use]
    pub fn pair_count(&self) -> usize {
        self.parts.pairs.len()
    }
}

/// Default LBF horizontal weight (jagua's `X_MULTIPLIER`). Heavy columns; see [`candidates`].
pub const DEFAULT_COLUMN_WEIGHT: u32 = 10;

/// Sorted candidate poses for a region: every vertex of every ring (outer AND hole — hole vertices
/// can never win the strictly-linear loss but are harmless), as `(cost, x, y)` ascending with the
/// deterministic lexicographic tie-break (docs/03 §5.2). The caller walks in order and CDE-verifies.
///
/// `column_weight` is the LBF horizontal multiplier (`cost = w·x_max + y_max`). The historical
/// default of 10 over-forces strict left-columns and leaves vertical gaps between irregular parts;
/// measured on consumer-like sector packs, `w = 3` gains ~2–3 pp utilization (docs/04 §4c). Weight
/// `0` is pure bottom-first (allowed; not recommended as a general default).
#[must_use]
pub fn candidates(
    region: &Shapes,
    loss_offsets: (i64, i64),
    column_weight: u32,
) -> Vec<(i64, i64, i64)> {
    let (bx, by) = loss_offsets;
    // Pre-size to the exact vertex count (nested flatten's size_hint is imprecise, so `collect` would
    // otherwise re-allocate as the region grows) — byte-identical, just fewer allocations.
    let cap = region.iter().flatten().map(Vec::len).sum();
    let mut out: Vec<(i64, i64, i64)> = Vec::with_capacity(cap);
    let w = i64::from(column_weight);
    out.extend(
        region
            .iter()
            .flatten()
            .flatten()
            .map(|p| (w * (p.x + bx) + (p.y + by), p.x, p.y)),
    );
    out.sort_unstable();
    out.dedup();
    out
}
