// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! NFP construction (docs/03 §3.2): the orientation-normalized convolution union-fill (the
//! Boost.Polygon / Deepnest lineage, corrected per the adversarial review), the convex×convex
//! edge-merge fast path, the □E contact inflation, and the frame-trick IFP.
//!
//! Correctness frame: every inserted piece — each edge-pair quad and each solid seed translate —
//! is a SUBSET of the true Minkowski sum, and every piece is CCW-normalized before the union, so
//! the overlay is an honest set union that never over-approximates. Under-fill (the only failure
//! direction on the obstacle side) yields over-permissive candidate regions whose bad candidates
//! the CDE rejects (docs/03 §2). Completeness for hole-free simple pairs follows from the case
//! split in docs/03 §3.2; holed inputs occur only in the frame-trick IFP, where the A-seed is
//! omitted (a frame ring thinner than any part admits no strictly-interior pose — see
//! [`ifp_frame_trick`]).

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;

use super::geom::{
    IPt, Ring, area2, canonicalize_shapes, ccw, convex_minkowski, is_convex_ccw, reflect,
    strip_collinear,
};

/// Shapes-with-holes: `[shape][ring]`, ring 0 = outer CCW, rings 1.. = holes CW (canonical form).
pub type Shapes = Vec<Vec<Ring>>;

/// Union of already-CCW-normalized solid pieces: all pieces run as `Subject` under `NonZero` and
/// the Subject fill is extracted. Output is canonicalized (docs/03 §8 rule 6).
fn union_pieces(pieces: &[Ring]) -> Shapes {
    let cap = pieces.iter().map(Vec::len).sum();
    let mut ov: Overlay<i64> = Overlay::new(cap);
    for p in pieces {
        ov.add_contour(p, ShapeType::Subject);
    }
    canonicalize_shapes(ov.overlay(OverlayRule::Subject, FillRule::NonZero))
}

/// The convolution union-fill Minkowski sum `A ⊕ (−B′)` for possibly-concave, possibly-holed `A`
/// (cycles: outer CCW + holes CW) against a SIMPLE moving footprint `B′` (docs/03 §3.2):
///
/// 1. every edge-pair quad `{a1+b2, a1+b1, a2+b1, a2+b2}` over all cycles of A × edges of −B′,
///    **CCW-normalized** (the winding-cancellation fix; zero-area quads dropped exactly);
/// 2. solid seed translates: A's SOLID cycles at −B′'s first vertex, −B′ at A's first vertex —
///    hole cycles never seed, and when A has holes the A-seed is omitted entirely (the translated
///    outer would flood the holes; see the module doc / the N0 spike lessons in docs/03 §11.1);
/// 3. one integer union.
///
/// The moving part's reference point is its origin: pose `t` collides ⟺ `t ∈ NFP + t_A`.
#[must_use]
pub fn nfp_union_fill(a_cycles: &[Ring], b_prime: &[IPt]) -> Shapes {
    let rb = reflect(b_prime);
    let mut pieces: Vec<Ring> = Vec::new();

    for cycle in a_cycles {
        let n = cycle.len();
        let m = rb.len();
        for i in 0..n {
            let a1 = cycle[i];
            let a2 = cycle[(i + 1) % n];
            for j in 0..m {
                let b1 = rb[j];
                let b2 = rb[(j + 1) % m];
                let quad = vec![
                    IPt::new(a1.x + b2.x, a1.y + b2.y),
                    IPt::new(a1.x + b1.x, a1.y + b1.y),
                    IPt::new(a2.x + b1.x, a2.y + b1.y),
                    IPt::new(a2.x + b2.x, a2.y + b2.y),
                ];
                if let Some(qd) = ccw(quad) {
                    pieces.push(qd);
                }
            }
        }
    }

    let a_has_holes = a_cycles.iter().any(|c| area2(c) < 0);
    let b0 = rb[0];
    if !a_has_holes {
        for cycle in a_cycles {
            if let Some(s) = ccw(cycle
                .iter()
                .map(|p| IPt::new(p.x + b0.x, p.y + b0.y))
                .collect())
            {
                pieces.push(s);
            }
        }
    }
    let a0 = a_cycles[0][0];
    if let Some(s) = ccw(rb
        .iter()
        .map(|p| IPt::new(p.x + a0.x, p.y + a0.y))
        .collect())
    {
        pieces.push(s);
    }

    union_pieces(&pieces)
}

/// NFP of a moving footprint `b_prime` orbiting a SIMPLE (hole-free) obstacle footprint `a`:
/// convex×convex takes the exact O(n+m) edge-merge (no boolean); anything else takes the
/// convolution union-fill. Both paths emit canonical shapes.
#[must_use]
pub fn nfp_pair(a: &[IPt], b_prime: &[IPt]) -> Shapes {
    let (a_stripped, b_stripped) = (strip_collinear(a), strip_collinear(b_prime));
    if let (Some(sa), Some(sb)) = (&a_stripped, &b_stripped)
        && is_convex_ccw(sa)
        && is_convex_ccw(sb)
    {
        let sum = convex_minkowski(sa, &reflect(sb));
        return canonicalize_shapes(vec![vec![sum]]);
    }
    let cycles = vec![a.to_vec()];
    nfp_union_fill(&cycles, b_prime)
}

/// `B ⊕ □E` — the contact-backoff inflation (docs/03 §4), one per (type, rotation).
/// Convex footprints go through the exact edge-merge; concave through the union-fill (□E is
/// symmetric, so reflecting it is a no-op). Returns the single outer ring.
#[must_use]
pub fn inflate_box(footprint: &[IPt], e: i64) -> Ring {
    let square = vec![
        IPt::new(-e, -e),
        IPt::new(e, -e),
        IPt::new(e, e),
        IPt::new(-e, e),
    ];
    if let Some(stripped) = strip_collinear(footprint)
        && is_convex_ccw(&stripped)
    {
        return convex_minkowski(&stripped, &square);
    }
    let ring = ccw(footprint.to_vec()).expect("footprint must have area");
    let sum = nfp_union_fill(std::slice::from_ref(&ring), &square);
    // A simple polygon ⊕ a square is one hole-free shape; take the (largest) outer defensively.
    sum.into_iter()
        .max_by_key(|shape| shape.first().map_or(0i128, |r| area2(r)))
        .and_then(|shape| shape.into_iter().next())
        .expect("B ⊕ □E must be nonempty")
}

/// The inner-fit polygon via a **quads-only frame sum + exact face classification** (the N1
/// revision of the frame trick — docs/03 §3.2 as amended by the P1 counterexample on concave
/// containers, where the naive holed-frame seed rules under-fill any pocket thicker than the part
/// and leak it into the IFP):
///
/// The edge-pair quads of `(frame ∪ container-hole) × −B′` alone cover EVERY boundary-touching
/// pose (`t = a − b` with `a ∈ ∂C_frame`, `b ∈ ∂B′` is by definition in `∂A ⊕ −∂B′`), so each
/// hole of the quads-union is a face of constant containment status. Classify each hole by one
/// exact integer test — a deterministic strictly-interior probe `t`, then "is `t + b₀` inside the
/// container?" (single-point containment suffices inside a crossing-free face) — and keep only
/// the truly-inside loops. Nested quads-union islands (containers with slots narrower than the
/// part) are subtracted at the end. All-integer, seed-free, correct for any simple container.
#[must_use]
pub fn ifp_frame_trick(container: &[IPt], b_prime: &[IPt], margin: i64) -> Shapes {
    use super::geom::{interior_point, point_in_ring, reflect};

    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for p in container {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let frame_outer = vec![
        IPt::new(x0 - margin, y0 - margin),
        IPt::new(x1 + margin, y0 - margin),
        IPt::new(x1 + margin, y1 + margin),
        IPt::new(x0 - margin, y1 + margin),
    ];
    // Container as the frame's hole (CW): both cycles contribute quads.
    let mut hole = container.to_vec();
    if area2(&hole) > 0 {
        hole.reverse();
    }
    let cycles = [frame_outer, hole];

    // Quads only — no seeds (they exist to fill solid-interior pose pockets, which classification
    // makes unnecessary; a holed-A seed is also exactly what leaked the concave pocket).
    let rb = reflect(b_prime);
    let mut pieces: Vec<Ring> = Vec::new();
    for cycle in &cycles {
        let n = cycle.len();
        let m = rb.len();
        for i in 0..n {
            let a1 = cycle[i];
            let a2 = cycle[(i + 1) % n];
            for j in 0..m {
                let b1 = rb[j];
                let b2 = rb[(j + 1) % m];
                let quad = vec![
                    IPt::new(a1.x + b2.x, a1.y + b2.y),
                    IPt::new(a1.x + b1.x, a1.y + b1.y),
                    IPt::new(a2.x + b1.x, a2.y + b1.y),
                    IPt::new(a2.x + b2.x, a2.y + b2.y),
                ];
                if let Some(qd) = ccw(quad) {
                    pieces.push(qd);
                }
            }
        }
    }
    let sum = union_pieces(&pieces);
    // Classify every hole of the union: keep iff its face is a "B′ fully inside the container"
    // face (probe + b₀ inside the container, tested exactly).
    let container_ccw = ccw(container.to_vec()).expect("container must have area");
    let b0 = b_prime[0];
    let mut kept: Shapes = Vec::new();
    for shape in &sum {
        for ring in shape.iter().skip(1) {
            let mut solid = ring.clone();
            solid.reverse(); // CW hole → CCW loop
            let Some(probe) = interior_point(&solid) else {
                continue;
            };
            if point_in_ring(&container_ccw, IPt::new(probe.x + b0.x, probe.y + b0.y)) {
                kept.push(vec![solid]);
            }
        }
    }
    if kept.is_empty() {
        return Vec::new();
    }

    // Subtract nested quads-union islands — forbidden-pose components floating INSIDE a kept
    // loop (slots narrower than the part). Only shapes whose outer lies strictly inside a kept
    // loop qualify (one-point containment suffices: faces never cross the union boundary); the
    // principal ring outer CONTAINS the loops and must not be subtracted.
    let islands: Vec<Vec<Ring>> = sum
        .iter()
        .filter(|shape| {
            let v = shape[0][0];
            kept.iter().any(|k| point_in_ring(&k[0], v))
        })
        .map(|shape| vec![shape[0].clone()])
        .collect();
    if islands.is_empty() {
        return canonicalize_shapes(kept);
    }
    difference(&canonicalize_shapes(kept), &islands)
}

/// `subject ∖ ⋃ clips` as ONE flat boolean pass over pristine inputs (docs/03 §8 rule 7 — region
/// queries never re-union snapped output). Subject and clip rings are added with their native
/// windings; `NonZero` resolves holes on both sides. Fully canonicalized (cache / test path).
#[must_use]
pub fn difference(subject: &Shapes, clips: &[Vec<Ring>]) -> Shapes {
    canonicalize_shapes(difference_raw(subject, clips))
}

/// Query-path difference for the NFP placer (docs/04 1c): same boolean as [`difference`], but only
/// drops zero-area rings — no Booth rotation / hole / shape sorts. [`crate::nfp::candidates`] only
/// needs the vertex multiset (it re-sorts by cost and dedups), so the candidate multiset is
/// byte-identical to the fully-canonicalized path while avoiding O(n²) canonicalization on every
/// placement query.
#[must_use]
pub fn difference_query(subject: &Shapes, clips: &[Vec<Ring>]) -> Shapes {
    drop_zero_area_shapes(difference_raw(subject, clips))
}

fn difference_raw(subject: &Shapes, clips: &[Vec<Ring>]) -> Shapes {
    let cap = subject.iter().flatten().map(Vec::len).sum::<usize>()
        + clips.iter().flatten().map(Vec::len).sum::<usize>();
    let mut ov: Overlay<i64> = Overlay::new(cap);
    for shape in subject {
        for ring in shape {
            ov.add_contour(ring, ShapeType::Subject);
        }
    }
    for shape in clips {
        for ring in shape {
            ov.add_contour(ring, ShapeType::Clip);
        }
    }
    ov.overlay(OverlayRule::Difference, FillRule::NonZero)
}

/// Drops zero-area rings/shapes only (exact shoelace). No winding fix / Booth / sorts.
fn drop_zero_area_shapes(shapes: Vec<Vec<Ring>>) -> Vec<Vec<Ring>> {
    let mut out = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let rings: Vec<Ring> = shape.into_iter().filter(|r| area2(r) != 0).collect();
        if !rings.is_empty() {
            out.push(rings);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i64, y: i64) -> IPt {
        IPt::new(x, y)
    }
    fn sq(x0: i64, y0: i64, s: i64) -> Ring {
        vec![
            pt(x0, y0),
            pt(x0 + s, y0),
            pt(x0 + s, y0 + s),
            pt(x0, y0 + s),
        ]
    }

    #[test]
    fn nfp_pair_convex_equals_union_fill() {
        // P4 dual-run (docs/03 §9.1): on convex pairs the fast path and the convolution
        // union-fill must agree exactly (canonical forms equal).
        let cases: [(Ring, Ring); 3] = [
            (sq(0, 0, 100), sq(-10, -10, 20)),
            (vec![pt(0, 0), pt(80, 0), pt(0, 60)], sq(-5, -5, 10)),
            (
                vec![pt(0, 0), pt(50, 0), pt(70, 30), pt(20, 50)],
                vec![pt(-10, 0), pt(0, -8), pt(10, 0), pt(0, 8)],
            ),
        ];
        for (a, b) in cases {
            let fast = nfp_pair(&a, &b);
            let general = nfp_union_fill(std::slice::from_ref(&a), &b);
            assert_eq!(fast, general, "fast path and union-fill disagree");
        }
    }

    #[test]
    fn nfp_concave_pocket_has_hole_or_pocket_region() {
        // A C-shaped obstacle and a small square that fits its pocket: the union-fill NFP of the
        // C must NOT cover the pocket-interior poses (the winding-cancellation fix at work).
        // C: outer ring of a 100×100 square with a 60-wide notch cut from the right side.
        let c_part = vec![
            pt(0, 0),
            pt(100, 0),
            pt(100, 20),
            pt(40, 20),
            pt(40, 80),
            pt(100, 80),
            pt(100, 100),
            pt(0, 100),
        ];
        let small = sq(-5, -5, 10);
        let nfp = nfp_pair(&c_part, &small);
        // Pose (70, 50): the square sits strictly inside the notch (feasible ⇒ NOT in the NFP).
        // Point-in-shapes test by winding (even-odd on canonical shapes-with-holes).
        let inside = point_in_shapes(&nfp, pt(70, 50));
        assert!(!inside, "pocket pose must be outside the NFP");
        // Pose (20, 50): the square overlaps the C's solid left wall (must be IN the NFP).
        assert!(
            point_in_shapes(&nfp, pt(20, 50)),
            "overlapping pose must be inside the NFP"
        );
    }

    fn point_in_ring(ring: &[IPt], p: IPt) -> bool {
        // Exact ray-cast (odd crossings of the +x ray); on-boundary counts as inside here (tests
        // avoid boundary points).
        let n = ring.len();
        let mut inside = false;
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            if (a.y > p.y) != (b.y > p.y) {
                // x-coordinate of the crossing, compared exactly via cross-multiplication.
                let lhs = i128::from(b.x - a.x) * i128::from(p.y - a.y);
                let rhs = i128::from(p.x - a.x) * i128::from(b.y - a.y);
                let crosses = if b.y > a.y { lhs > rhs } else { lhs < rhs };
                if crosses {
                    inside = !inside;
                }
            }
        }
        inside
    }

    fn point_in_shapes(shapes: &Shapes, p: IPt) -> bool {
        for shape in shapes {
            if point_in_ring(&shape[0], p) {
                let in_hole = shape[1..].iter().any(|h| point_in_ring(h, p));
                if !in_hole {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn ifp_rect_oracle_exact() {
        // P6 (docs/03 §9.1): for a rectangle container the IFP must equal the bbox-shrunken
        // rectangle EXACTLY, per footprint bbox.
        let container = sq(0, 0, 1000);
        let b = vec![pt(-30, -20), pt(50, -20), pt(50, 40), pt(-30, 40)]; // bbox [-30,50]×[-20,40]
        let ifp = ifp_frame_trick(&container, &b, 16);
        assert_eq!(ifp.len(), 1, "one IFP loop for a rect container");
        let hull = super::super::geom::convex_hull(ifp[0][0].clone());
        // Feasible t: t ≥ (0−(−30), 0−(−20)) and t ≤ (1000−50, 1000−40) → [30,950]×[20,960].
        assert_eq!(
            hull,
            vec![pt(30, 20), pt(950, 20), pt(950, 960), pt(30, 960)]
        );
    }

    #[test]
    fn inflate_box_grows_bbox_by_e() {
        let tri = vec![pt(0, 0), pt(100, 0), pt(0, 100)];
        let inflated = inflate_box(&tri, 16);
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for p in &inflated {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        assert_eq!((x0, y0, x1, y1), (-16, -16, 116, 116));
    }

    #[test]
    fn difference_carves_hole() {
        let subject: Shapes = vec![vec![sq(0, 0, 100)]];
        let clips = vec![vec![sq(40, 40, 20)]];
        let d = difference(&subject, &clips);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].len(), 2, "one outer + one hole");
    }
}
