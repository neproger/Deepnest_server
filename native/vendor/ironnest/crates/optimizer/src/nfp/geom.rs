// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Integer-geometry primitives for the exact-NFP mode (docs/03 §3): quantization, exact i128
//! predicates, orientation normalization, canonicalization, and the convex machinery
//! (hull, convexity test, edge-merge Minkowski sum).
//!
//! DETERMINISM(ironnest): everything here is `i64` coordinates with `i128` accumulation — exactly
//! specified two's-complement arithmetic, byte-identical on every platform by construction
//! (docs/03 §8 rules 1–4). The ONLY float↔int conversions in the whole NFP module are
//! [`quantize`]/[`unquantize`] (rule 2).

use i_overlay::i_float::int::point::IntPoint;
use ironnest_geo::Scalar;

/// One integer grid point (i64 engine).
pub type IPt = IntPoint<i64>;
/// One ring (closed contour, implicit last→first edge).
pub type Ring = Vec<IPt>;

/// Grid units per input unit: 2^20 — FIXED and GLOBAL (docs/03 §8 rule 1). `x * NFP_SCALE` is a
/// pure exponent shift (exact); quantization's `.round()` is the single lossy step.
pub const NFP_SCALE: Scalar = 1_048_576.0;
/// 2^-20 — exactly representable, so [`unquantize`] is exact for |n| < 2^53.
pub const NFP_INV_SCALE: Scalar = 1.0 / NFP_SCALE;

/// f64 → grid. THE one rounding site (with pose quantization) in the NFP module (docs/03 §8 rule 2).
#[inline]
#[must_use]
pub fn quantize(v: Scalar) -> i64 {
    #[allow(clippy::cast_possible_truncation)] // domain-validated: |v*S| ≪ 2^61 (NfpError gate)
    {
        (v * NFP_SCALE).round() as i64
    }
}

/// Grid → f64 — exact (power-of-two scale, |n| < 2^53 by the domain gate).
#[inline]
#[must_use]
pub fn unquantize(n: i64) -> Scalar {
    #[allow(clippy::cast_precision_loss)] // exact: |n| < 2^53 (domain gate), 2^-20 exact
    {
        (n as Scalar) * NFP_INV_SCALE
    }
}

/// Exact doubled signed area (shoelace) of a ring, in i128 (docs/03 §8 rule 4: every self-written
/// predicate accumulates in i128 — the orient2d-class worst case exceeds i64).
#[must_use]
pub fn area2(ring: &[IPt]) -> i128 {
    let n = ring.len();
    let mut acc: i128 = 0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        acc += i128::from(a.x) * i128::from(b.y) - i128::from(b.x) * i128::from(a.y);
    }
    acc
}

/// Exact orientation predicate: cross of (o→a) × (o→b), i128.
#[inline]
#[must_use]
pub fn cross(o: IPt, a: IPt, b: IPt) -> i128 {
    i128::from(a.x - o.x) * i128::from(b.y - o.y) - i128::from(a.y - o.y) * i128::from(b.x - o.x)
}

/// Orientation-normalizes a ring to CCW (docs/03 §3.2 — the winding-cancellation fix).
/// `None` for zero-area (degenerate) rings — they are dropped, never unioned.
#[must_use]
pub fn ccw(mut ring: Ring) -> Option<Ring> {
    use std::cmp::Ordering;
    match area2(&ring).cmp(&0) {
        Ordering::Greater => Some(ring),
        Ordering::Less => {
            ring.reverse();
            Some(ring)
        }
        Ordering::Equal => None,
    }
}

/// Removes exactly-collinear and duplicate vertices (quantization manufactures them). Keeps
/// orientation; returns `None` if fewer than 3 vertices survive.
#[must_use]
pub fn strip_collinear(ring: &[IPt]) -> Option<Ring> {
    let n = ring.len();
    if n < 3 {
        return None;
    }
    let mut out: Ring = Vec::with_capacity(n);
    for i in 0..n {
        let prev = ring[(i + n - 1) % n];
        let cur = ring[i];
        let next = ring[(i + 1) % n];
        if cur != prev && cross(prev, cur, next) != 0 {
            out.push(cur);
        }
    }
    (out.len() >= 3).then_some(out)
}

/// Exact convexity test on a CCW ring with no collinear vertices (run [`strip_collinear`] first):
/// every turn strictly left.
#[must_use]
pub fn is_convex_ccw(ring: &[IPt]) -> bool {
    let n = ring.len();
    (0..n).all(|i| cross(ring[i], ring[(i + 1) % n], ring[(i + 2) % n]) > 0)
}

/// Convex hull (Andrew monotone chain), CCW output, collinear points dropped.
#[must_use]
pub fn convex_hull(mut pts: Vec<IPt>) -> Ring {
    pts.sort_unstable_by_key(|p| (p.x, p.y));
    pts.dedup();
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    let mut hull: Ring = Vec::with_capacity(2 * n);
    for &p in &pts {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev() {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// Minkowski sum of two CONVEX CCW rings (no collinear vertices) by the classic sorted-edge merge —
/// O(n+m), no boolean engine (docs/03 §3.2 fast path). Exact integer arithmetic throughout.
#[must_use]
#[allow(clippy::many_single_char_names)] // p/q/i/j/n/m — standard computational-geometry naming
pub fn convex_minkowski(a: &[IPt], b: &[IPt]) -> Ring {
    // Rotate each ring to start at its bottom-most (then leftmost) vertex.
    let start = |r: &[IPt]| -> usize {
        let mut best = 0;
        for (i, p) in r.iter().enumerate() {
            if (p.y, p.x) < (r[best].y, r[best].x) {
                best = i;
            }
        }
        best
    };
    let (sa, sb) = (start(a), start(b));
    let (n, m) = (a.len(), b.len());
    let av = |i: usize| a[(sa + i) % n];
    let bv = |j: usize| b[(sb + j) % m];

    let mut out: Ring = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    while i < n || j < m {
        let p = av(i % n);
        let q = bv(j % m);
        out.push(IPt::new(p.x + q.x, p.y + q.y));
        // Compare edge directions: advance the ring whose current edge turns first (CCW order).
        let ea = {
            let p2 = av((i + 1) % n);
            IPt::new(p2.x - p.x, p2.y - p.y)
        };
        let eb = {
            let q2 = bv((j + 1) % m);
            IPt::new(q2.x - q.x, q2.y - q.y)
        };
        let cr = i128::from(ea.x) * i128::from(eb.y) - i128::from(ea.y) * i128::from(eb.x);
        if j >= m || (i < n && cr > 0) {
            i += 1;
        } else if i >= n || cr < 0 {
            j += 1;
        } else {
            // Parallel edges: advance both (their sum is one combined edge).
            i += 1;
            j += 1;
        }
    }
    // The merge can emit collinear break-points where parallel runs met; strip for canonical output.
    strip_collinear(&out).unwrap_or(out)
}

/// Point reflection through the origin. A CCW ring stays CCW (reflection = rotation by 180°).
#[must_use]
pub fn reflect(ring: &[IPt]) -> Ring {
    ring.iter().map(|p| IPt::new(-p.x, -p.y)).collect()
}

/// Translates a ring by `(tx, ty)` — exact integer addition.
#[must_use]
pub fn translate(ring: &[IPt], tx: i64, ty: i64) -> Ring {
    ring.iter().map(|p| IPt::new(p.x + tx, p.y + ty)).collect()
}

/// Exact point-in-ring test (+x ray cast, crossings compared by i128 cross-multiplication).
/// Boundary points are NOT guaranteed either way — callers use strictly-interior probes
/// ([`interior_point`]).
#[must_use]
pub fn point_in_ring(ring: &[IPt], p: IPt) -> bool {
    let n = ring.len();
    let mut inside = false;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        if (a.y > p.y) != (b.y > p.y) {
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

/// A deterministic strictly-interior point of a simple ring: the deterministic ladder of
/// vertex-diagonal midpoints `(v_i + v_{i+2}) / 2`, first that tests strictly inside (ear
/// existence guarantees one exists for a valid simple ring; every probe is validated by
/// [`point_in_ring`], so truncating integer division is fine). `None` only for degenerate rings.
#[must_use]
pub fn interior_point(ring: &[IPt]) -> Option<IPt> {
    let n = ring.len();
    if n < 3 {
        return None;
    }
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 2) % n];
        let mid = IPt::new(i64::midpoint(a.x, b.x), i64::midpoint(a.y, b.y));
        if point_in_ring(ring, mid) {
            return Some(mid);
        }
    }
    None
}

// -------------------------------------------------------------------------------------------
// Canonicalization (docs/03 §3.4 — applied to every NFP from ANY path)
// -------------------------------------------------------------------------------------------

/// Rotates a ring to its lexicographically-least rotation (Booth-style, restricted to the
/// occurrences of the minimal vertex — occurrence-independent, so pinched rings that visit their
/// minimal vertex twice still canonicalize identically regardless of the producer's start vertex).
#[must_use]
pub fn canonical_rotation(ring: &[IPt]) -> Ring {
    let n = ring.len();
    if n == 0 {
        return Vec::new();
    }
    let key = |p: &IPt| (p.x, p.y);
    let min_key = ring.iter().map(key).min().unwrap();
    let mut best: Option<usize> = None;
    for s in 0..n {
        if key(&ring[s]) != min_key {
            continue;
        }
        let better = match best {
            None => true,
            Some(b) => (0..n).any(|i| {
                let (rb, rs) = (key(&ring[(b + i) % n]), key(&ring[(s + i) % n]));
                rs != rb && {
                    // first difference decides
                    (0..i).all(|j| key(&ring[(b + j) % n]) == key(&ring[(s + j) % n])) && rs < rb
                }
            }),
        };
        if better {
            best = Some(s);
        }
    }
    let s = best.unwrap();
    (0..n).map(|i| ring[(s + i) % n]).collect()
}

/// Canonical shapes-with-holes: winding fixed by exact area (outer CCW first, holes CW after),
/// every ring rotated to its least rotation, holes sorted, zero-area rings dropped.
/// The cache stores ONLY canonical geometry (docs/03 §8 rule 6).
#[must_use]
pub fn canonicalize_shapes(shapes: Vec<Vec<Ring>>) -> Vec<Vec<Ring>> {
    let mut out: Vec<Vec<Ring>> = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let mut rings: Vec<Ring> = Vec::with_capacity(shape.len());
        for (idx, ring) in shape.into_iter().enumerate() {
            let a2 = area2(&ring);
            if a2 == 0 {
                continue; // degenerate — dropped (exact test)
            }
            // Ring 0 is the outer (must be CCW); the rest are holes (must be CW).
            let want_ccw = idx == 0;
            let mut r = ring;
            if (a2 > 0) != want_ccw {
                r.reverse();
            }
            rings.push(canonical_rotation(&r));
        }
        if rings.is_empty() {
            continue;
        }
        // Sort the holes (outer stays first) for producer-order independence.
        rings[1..].sort_unstable_by(|a, b| {
            (a.first().map(|p| (p.x, p.y)), a.len(), a as &[_]).cmp(&(
                b.first().map(|p| (p.x, p.y)),
                b.len(),
                b as &[_],
            ))
        });
        out.push(rings);
    }
    // Sort shapes by their outer ring for producer-order independence.
    out.sort_unstable_by(|a, b| {
        (a[0].first().map(|p| (p.x, p.y)), a[0].len(), &a[0]).cmp(&(
            b[0].first().map(|p| (p.x, p.y)),
            b[0].len(),
            &b[0],
        ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i64, y: i64) -> IPt {
        IPt::new(x, y)
    }

    #[test]
    fn quantize_round_trip_within_half_grid() {
        for v in [-0.3, 0.0, 1.234_567, 99.999_999, -511.5] {
            let back = unquantize(quantize(v));
            assert!((back - v).abs() <= 0.5 * NFP_INV_SCALE, "v={v} back={back}");
        }
    }

    #[test]
    fn ccw_normalizes_and_drops_degenerate() {
        let cw = vec![pt(0, 0), pt(0, 10), pt(10, 0)];
        let r = ccw(cw).unwrap();
        assert!(area2(&r) > 0);
        assert!(ccw(vec![pt(0, 0), pt(5, 5), pt(10, 10)]).is_none());
    }

    #[test]
    fn convex_minkowski_squares() {
        // [0,10]² ⊕ [-2,2]² = [-2,12]² — exact.
        let a = vec![pt(0, 0), pt(10, 0), pt(10, 10), pt(0, 10)];
        let b = vec![pt(-2, -2), pt(2, -2), pt(2, 2), pt(-2, 2)];
        let s = convex_minkowski(&a, &b);
        let hull = convex_hull(s.clone());
        assert_eq!(hull.len(), 4);
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for p in &s {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        assert_eq!((x0, y0, x1, y1), (-2, -2, 12, 12));
        assert_eq!(area2(&s), 2 * 14 * 14);
    }

    #[test]
    fn convex_minkowski_triangle_square_area() {
        // area(A⊕B) = area(A) + area(B) + mixed term; verify against hull-of-sums brute force.
        let a = vec![pt(0, 0), pt(8, 0), pt(0, 6)];
        let b = vec![pt(-1, -1), pt(3, -1), pt(3, 2), pt(-1, 2)];
        let s = convex_minkowski(&a, &b);
        let mut brute: Vec<IPt> = Vec::new();
        for p in &a {
            for q in &b {
                brute.push(pt(p.x + q.x, p.y + q.y));
            }
        }
        let hull = convex_hull(brute);
        assert_eq!(
            area2(&s),
            area2(&hull),
            "edge-merge must equal hull-of-sums"
        );
    }

    #[test]
    fn canonical_rotation_is_start_independent() {
        let base = vec![pt(3, 3), pt(0, 0), pt(5, 0), pt(5, 5)];
        let c1 = canonical_rotation(&base);
        let rotated: Ring = (0..base.len())
            .map(|i| base[(i + 2) % base.len()])
            .collect();
        let c2 = canonical_rotation(&rotated);
        assert_eq!(c1, c2);
        assert_eq!(c1[0], pt(0, 0));
    }

    #[test]
    fn canonical_rotation_pinched_ring() {
        // The minimal vertex (0,0) occurs twice; both starts must canonicalize identically.
        let pinched = vec![pt(0, 0), pt(4, 0), pt(2, 1), pt(0, 0), pt(2, 3)];
        let a = canonical_rotation(&pinched);
        let shifted: Ring = (0..pinched.len())
            .map(|i| pinched[(i + 3) % pinched.len()])
            .collect();
        let b = canonical_rotation(&shifted);
        assert_eq!(a, b);
    }

    #[test]
    fn strip_collinear_removes_midpoints() {
        let r = vec![pt(0, 0), pt(5, 0), pt(10, 0), pt(10, 10), pt(0, 10)];
        let s = strip_collinear(&r).unwrap();
        assert_eq!(s.len(), 4);
    }
}
