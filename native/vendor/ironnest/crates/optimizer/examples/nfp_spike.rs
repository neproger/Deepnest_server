// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! **N0 spike** (docs/03 §11): THROWAWAY end-to-end probe of the exact-NFP constructive mode.
//! Quantize → orientation-normalized convolution union-fill NFP → frame-trick IFP → □E backoff →
//! single-start greedy vertex placement, CDE-verified. Measures the docs/03 §10 kill criterion
//! (pentagon ≥ 24/40 by construction) plus bricks / interlock / contact-columns and wall-clock.
//! No cache sharing, no mode switch, no canonicalization polish — N1/N2 own those.
//! `cargo run -p ironnest-optimizer --release --example nfp_spike`
//!
//! DETERMINISM(ironnest): spike only — `Instant` here is benchmark timing (bench.rs waiver), never
//! a placement input. The placement pipeline itself is deterministic (integer geometry + fixed
//! orders; zero PRNG).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::i_shape::int::shape::IntShapes;

use ironnest_cde::collision_detection::CDEConfig;
use ironnest_cde::collision_detection::hazards::filter::NoFilter;
use ironnest_cde::entities::{Item, Layout};
use ironnest_cde::geometry::fail_fast::SPSurrogateConfig;
use ironnest_cde::geometry::geo_traits::TransformableFrom;
use ironnest_cde::io::ext_repr::{ExtContainer, ExtItem, ExtSPolygon, ExtShape};
use ironnest_cde::io::import::Importer;
use ironnest_geo::{DTransformation, Scalar};

type I64Pt = IntPoint<i64>;

/// 2^20 grid units per input unit (docs/03 §3.1).
const SCALE: Scalar = 1_048_576.0;
const INV_SCALE: Scalar = 1.0 / SCALE;
/// Contact backoff □E half-width, grid units (docs/03 §4).
const E: i64 = 16;
/// Frame-trick ring width, grid units (any nonzero works; 1 full unit for slack).
const FRAME_MARGIN: i64 = 1 << 20;

fn q(v: Scalar) -> i64 {
    (v * SCALE).round() as i64
}
fn unq(n: i64) -> Scalar {
    (n as Scalar) * INV_SCALE
}

/// Exact i128 doubled signed area (shoelace) of an integer ring.
fn area2(ring: &[I64Pt]) -> i128 {
    let n = ring.len();
    let mut acc: i128 = 0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        acc += (a.x as i128) * (b.y as i128) - (b.x as i128) * (a.y as i128);
    }
    acc
}

/// Orientation-normalize a ring to CCW (docs/03 §3.2 — the winding-cancellation fix).
/// Returns `None` for zero-area (degenerate) rings.
fn ccw(mut ring: Vec<I64Pt>) -> Option<Vec<I64Pt>> {
    match area2(&ring).cmp(&0) {
        Ordering::Greater => Some(ring),
        Ordering::Less => {
            ring.reverse();
            Some(ring)
        }
        Ordering::Equal => None,
    }
}

/// Convex hull (Andrew monotone chain) of integer points — used for B ⊕ □E on the spike's convex
/// parts (Minkowski of a convex polygon with a square = hull of the four corner-translates).
fn convex_hull(mut pts: Vec<I64Pt>) -> Vec<I64Pt> {
    pts.sort_by_key(|a| (a.x, a.y));
    pts.dedup();
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    let cross = |o: I64Pt, a: I64Pt, b: I64Pt| -> i128 {
        ((a.x - o.x) as i128) * ((b.y - o.y) as i128)
            - ((a.y - o.y) as i128) * ((b.x - o.x) as i128)
    };
    let mut hull: Vec<I64Pt> = Vec::with_capacity(2 * n);
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

/// B ⊕ □E for a CONVEX integer polygon: hull of the 4 corner-translates.
fn inflate_box_convex(poly: &[I64Pt], e: i64) -> Vec<I64Pt> {
    let mut pts = Vec::with_capacity(poly.len() * 4);
    for &p in poly {
        for (dx, dy) in [(-e, -e), (e, -e), (e, e), (-e, e)] {
            pts.push(I64Pt::new(p.x + dx, p.y + dy));
        }
    }
    convex_hull(pts)
}

/// Exact convexity test on an integer CCW ring (zero-crosses = collinear vertices allowed).
fn is_convex(poly: &[I64Pt]) -> bool {
    let n = poly.len();
    (0..n).all(|i| {
        let o = poly[i];
        let a = poly[(i + 1) % n];
        let b = poly[(i + 2) % n];
        ((a.x - o.x) as i128) * ((b.y - o.y) as i128)
            - ((a.y - o.y) as i128) * ((b.x - o.x) as i128)
            >= 0
    })
}

/// B ⊕ □E for ANY simple integer polygon: convex → hull shortcut; concave → the convolution
/// union-fill itself (□E is symmetric, so nfp_union_fill(B, □E) = B ⊕ (−□E) = B ⊕ □E).
/// Returns the outer ring of the sum (its holes cannot exist for a simple B ⊕ square).
fn inflate_box(poly: &[I64Pt], e: i64) -> Vec<I64Pt> {
    let ring = ccw(poly.to_vec()).expect("footprint must have area");
    if is_convex(&ring) {
        return inflate_box_convex(&ring, e);
    }
    let square = vec![
        I64Pt::new(-e, -e),
        I64Pt::new(e, -e),
        I64Pt::new(e, e),
        I64Pt::new(-e, e),
    ];
    let sum = nfp_union_fill(std::slice::from_ref(&ring), &square);
    sum.into_iter()
        .max_by_key(|shape| shape.first().map_or(0i128, |r| area2(r)))
        .and_then(|shape| shape.into_iter().next())
        .expect("B ⊕ □E must be nonempty")
}

/// Union-of-subject-pieces: run all contours as Subject under NonZero, extract Subject fill.
fn union_pieces(pieces: Vec<Vec<I64Pt>>) -> IntShapes<i64> {
    let cap = pieces.iter().map(Vec::len).sum();
    let mut ov: Overlay<i64> = Overlay::new(cap);
    for p in &pieces {
        ov.add_contour(p, ShapeType::Subject);
    }
    ov.overlay(OverlayRule::Subject, FillRule::NonZero)
}

/// Convolution union-fill NFP (docs/03 §3.2): cycles of A (outer CCW + optional CW holes) convolved
/// with the REFLECTED moving footprint −B'. Every quad and seed is CCW-normalized before the union.
/// Output: the Minkowski sum A ⊕ (−B′) as shapes-with-holes. The moving part's reference point is
/// its (centroid) origin, so "pose t collides" ⟺ t ∈ NFP translated by A's position.
fn nfp_union_fill(a_cycles: &[Vec<I64Pt>], b_prime: &[I64Pt]) -> IntShapes<i64> {
    let rb: Vec<I64Pt> = b_prime.iter().map(|p| I64Pt::new(-p.x, -p.y)).collect();
    let mut pieces: Vec<Vec<I64Pt>> = Vec::new();

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
                    I64Pt::new(a1.x + b2.x, a1.y + b2.y),
                    I64Pt::new(a1.x + b1.x, a1.y + b1.y),
                    I64Pt::new(a2.x + b1.x, a2.y + b1.y),
                    I64Pt::new(a2.x + b2.x, a2.y + b2.y),
                ];
                if let Some(qd) = ccw(quad) {
                    pieces.push(qd);
                }
            }
        }
    }
    // Interior seeds: A translated to −B′'s first vertex, −B′ translated to A's first vertex.
    // The A-seed exists to fill "B strictly inside A's solid" pose pockets. When A HAS HOLES
    // (the frame-trick ring), the translated OUTER contour would flood the hole region (this bug
    // emptied the IFP twice in this spike) — and no B-pose fits strictly inside a ring thinner
    // than B anyway (FRAME_MARGIN ≪ part dims), so for holed A the A-seed is omitted entirely.
    // Omission can only under-fill (safe direction: over-permissive region, CDE rejects). N1 does
    // winding-correct shape-group seeding properly.
    let a_has_holes = a_cycles.iter().any(|c| area2(c) < 0);
    let b0 = rb[0];
    if !a_has_holes {
        for cycle in a_cycles {
            let seed: Vec<I64Pt> = cycle
                .iter()
                .map(|p| I64Pt::new(p.x + b0.x, p.y + b0.y))
                .collect();
            if let Some(s) = ccw(seed) {
                pieces.push(s);
            }
        }
    }
    let a0 = a_cycles[0][0];
    let seed_b: Vec<I64Pt> = rb
        .iter()
        .map(|p| I64Pt::new(p.x + a0.x, p.y + a0.y))
        .collect();
    if let Some(s) = ccw(seed_b) {
        pieces.push(s);
    }

    union_pieces(pieces)
}

/// IFP via the frame trick (docs/03 §3.2): frame = container bbox expanded by FRAME_MARGIN with the
/// container as a hole; the HOLES of NFP(frame, B′) are the IFP loops (returned CCW).
fn ifp_frame_trick(container: &[I64Pt], b_prime: &[I64Pt]) -> Vec<Vec<I64Pt>> {
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for p in container {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let (fx0, fy0, fx1, fy1) = (
        x0 - FRAME_MARGIN,
        y0 - FRAME_MARGIN,
        x1 + FRAME_MARGIN,
        y1 + FRAME_MARGIN,
    );
    let frame_outer = vec![
        I64Pt::new(fx0, fy0),
        I64Pt::new(fx1, fy0),
        I64Pt::new(fx1, fy1),
        I64Pt::new(fx0, fy1),
    ];
    // Container as the frame's hole: CW cycle.
    let mut hole = container.to_vec();
    if area2(&hole) > 0 {
        hole.reverse();
    }
    let cycles = vec![frame_outer, hole];
    let nfp = nfp_union_fill(&cycles, b_prime);

    // IFP = holes of the result (inner contours, emitted CW by i_overlay) → reverse to CCW.
    let mut loops = Vec::new();
    for shape in &nfp {
        for ring in shape.iter().skip(1) {
            let mut r = ring.clone();
            r.reverse();
            loops.push(r);
        }
    }
    loops
}

// -------------------------------------------------------------------------------------------
// Spike harness
// -------------------------------------------------------------------------------------------

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

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

/// Cardinal rotation of centered f64 vertices by exact swap/negate, then quantize.
/// rot_idx: 0 → 0°, 1 → 90° CCW, 2 → 180°, 3 → 270°.
fn quantized_rotated(item: &Item, rot_idx: usize) -> Vec<I64Pt> {
    item.shape_cd
        .vertices
        .iter()
        .map(|p| {
            let (x, y) = (p.0, p.1);
            let (rx, ry) = match rot_idx {
                0 => (x, y),
                1 => (-y, x),
                2 => (-x, -y),
                _ => (y, -x),
            };
            I64Pt::new(q(rx), q(ry))
        })
        .collect()
}

struct Placed {
    item_id: usize,
    rot_idx: usize,
    x: i64,
    y: i64,
}

struct ProbeResult {
    placed: usize,
    rejected_candidates: usize,
    ms: f64,
}

/// Shoelace area of an input outline.
fn outline_area(o: &[[Scalar; 2]]) -> Scalar {
    let n = o.len();
    let mut acc = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        acc += o[i][0] * o[j][1] - o[j][0] * o[i][1];
    }
    (0.5 * acc).abs()
}

#[allow(clippy::too_many_lines)]
fn run_probe(
    label: &str,
    items_outlines: &[Vec<[Scalar; 2]>],
    qty: &[usize],
    container_outline: &[[Scalar; 2]],
    container_area: Scalar,
    min_sep: Scalar,
) -> ProbeResult {
    #[allow(clippy::disallowed_methods)] // spike timing only — never a placement input
    let t0 = std::time::Instant::now();

    let item_areas: Vec<Scalar> = items_outlines.iter().map(|o| outline_area(o)).collect();
    let mut importer = Importer::new(
        default_cde_config(),
        None,
        (min_sep > 0.0).then_some(min_sep),
        None,
    );
    // Mirror lib.rs: collision-footprint decimation for high-vertex curved parts at min_sep > 0.
    importer.shape_modify_config.collision_decimation = (min_sep > 0.0).then_some(min_sep / 16.0);
    let ext_container = ExtContainer {
        id: 0,
        shape: ExtShape::SimplePolygon(ExtSPolygon(
            container_outline.iter().map(|p| (p[0], p[1])).collect(),
        )),
        zones: vec![],
    };
    let container = importer.import_container(&ext_container).unwrap();
    let container_int: Vec<I64Pt> = container
        .outer_cd
        .vertices
        .iter()
        .map(|p| I64Pt::new(q(p.0), q(p.1)))
        .collect();

    let cardinal = [0.0, 90.0, 180.0, 270.0];
    let items: Vec<Item> = items_outlines
        .iter()
        .enumerate()
        .map(|(i, o)| {
            importer
                .import_item(&ExtItem {
                    id: i as u64,
                    allowed_orientations: Some(cardinal.to_vec()),
                    shape: ExtShape::SimplePolygon(ExtSPolygon(
                        o.iter().map(|p| (p[0], p[1])).collect(),
                    )),
                    min_quality: None,
                })
                .unwrap()
        })
        .collect();
    let rot_rads: [Scalar; 4] = [
        0.0,
        90f64.to_radians(),
        180f64.to_radians(),
        270f64.to_radians(),
    ];

    // Per (type, rot): quantized footprint and its □E inflation; integer bbox max for the loss.
    let mut foot: Vec<[Vec<I64Pt>; 4]> = Vec::new();
    let mut foot_e: Vec<[Vec<I64Pt>; 4]> = Vec::new();
    let mut bbox_max: Vec<[(i64, i64); 4]> = Vec::new();
    for item in &items {
        let mut f: [Vec<I64Pt>; 4] = Default::default();
        let mut fe: [Vec<I64Pt>; 4] = Default::default();
        let mut bm = [(0i64, 0i64); 4];
        for r in 0..4 {
            let p = quantized_rotated(item, r);
            let pe = inflate_box(&p, E);
            let (mut mx, mut my) = (i64::MIN, i64::MIN);
            for v in &p {
                mx = mx.max(v.x);
                my = my.max(v.y);
            }
            bm[r] = (mx, my);
            f[r] = p;
            fe[r] = pe;
        }
        foot.push(f);
        foot_e.push(fe);
        bbox_max.push(bm);
    }

    // IFPs per (type, rot) via the frame trick (rect-container oracle asserted below).
    let mut ifp: Vec<[Vec<Vec<I64Pt>>; 4]> = Vec::new();
    for fe in &foot_e {
        let mut per_rot: [Vec<Vec<I64Pt>>; 4] = Default::default();
        for (r, slot) in per_rot.iter_mut().enumerate() {
            *slot = ifp_frame_trick(&container_int, &fe[r]);
        }
        ifp.push(per_rot);
    }

    // Pairwise NFP cache: (placed_type, placed_rot, moving_type, moving_rot) → shapes.
    let mut nfp_cache: BTreeMap<(usize, usize, usize, usize), IntShapes<i64>> = BTreeMap::new();

    // Greedy largest-first placement.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        items[b]
            .shape_cd
            .diameter
            .total_cmp(&items[a].shape_cd.diameter)
    });

    let mut layout = Layout::new(container);
    let mut placed: Vec<Placed> = Vec::new();
    let mut placed_count = 0usize;
    let mut rejected = 0usize;
    let mut buffers: Vec<_> = items
        .iter()
        .map(|it| {
            let mut b = (*it.shape_cd).clone();
            b.surrogate = None;
            b
        })
        .collect::<Vec<_>>();

    for &t in &order {
        'instance: for _ in 0..qty[t] {
            // Candidates across rotations: (cost, rot, x, y).
            let mut cands: Vec<(i64, usize, i64, i64)> = Vec::new();
            for r in 0..4 {
                // Region = IFP(t, r) ∖ ⋃ placed NFPs (one flat difference).
                let cap = ifp[t][r].iter().map(Vec::len).sum::<usize>() + placed.len() * 16;
                let mut ov: Overlay<i64> = Overlay::new(cap);
                for loop_ in &ifp[t][r] {
                    ov.add_contour(loop_, ShapeType::Subject);
                }
                for pl in &placed {
                    let key = (pl.item_id, pl.rot_idx, t, r);
                    let nfp = nfp_cache.entry(key).or_insert_with(|| {
                        nfp_union_fill(
                            std::slice::from_ref(&foot[pl.item_id][pl.rot_idx]),
                            &foot_e[t][r],
                        )
                    });
                    for shape in nfp.iter() {
                        for (ci, ring) in shape.iter().enumerate() {
                            let tr: Vec<I64Pt> = ring
                                .iter()
                                .map(|p| I64Pt::new(p.x + pl.x, p.y + pl.y))
                                .collect();
                            let _ = ci;
                            ov.add_contour(&tr, ShapeType::Clip);
                        }
                    }
                }
                let region = ov.overlay(OverlayRule::Difference, FillRule::NonZero);
                for shape in &region {
                    for ring in shape {
                        for p in ring {
                            let cost = 10 * (p.x + bbox_max[t][r].0) + (p.y + bbox_max[t][r].1);
                            cands.push((cost, r, p.x, p.y));
                        }
                    }
                }
            }
            cands.sort_unstable();

            for &(_, r, x, y) in &cands {
                let dt = DTransformation::new(rot_rads[r], (unq(x), unq(y)));
                let transf = dt.compose();
                let buffer = &mut buffers[t];
                buffer.transform_from(&items[t].shape_cd, &transf);
                if !layout.cde().detect_poly_collision(buffer, &NoFilter) {
                    layout.place_item(&items[t], dt);
                    placed.push(Placed {
                        item_id: t,
                        rot_idx: r,
                        x,
                        y,
                    });
                    placed_count += 1;
                    continue 'instance;
                }
                rejected += 1;
            }
            // No candidate worked for this instance; larger instances of the same type won't
            // either (identical geometry) — stop this type.
            break;
        }
    }

    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let placed_area: Scalar = placed.iter().map(|p| item_areas[p.item_id]).sum();
    let util = placed_area / container_area * 100.0;
    let total: usize = qty.iter().sum();
    println!(
        "{label:<26} placed {placed_count:>4}/{total:<4}  util {util:>5.1}%  {ms:>9.1} ms  (verify-rejected {rejected})"
    );
    ProbeResult {
        placed: placed_count,
        rejected_candidates: rejected,
        ms,
    }
}

fn main() {
    println!("--- N0 spike: exact-NFP constructive placement (single start, no PRNG) ---");
    let pentagon = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 12.0],
        [12.0, 20.0],
        [0.0, 20.0],
    ];
    let right_tri = vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]];

    let r1 = run_probe(
        "pentagon ~344 (KILL>=24)",
        &[pentagon],
        &[40],
        &rect(100.0, 100.0),
        10_000.0,
        0.0,
    );
    let r2 = run_probe(
        "13x7 bricks",
        &[rect(13.0, 7.0)],
        &[120],
        &rect(100.0, 100.0),
        10_000.0,
        0.0,
    );
    let r3 = run_probe(
        "2 right-tris pair->sq",
        &[right_tri],
        &[2],
        &rect(11.0, 11.0),
        121.0,
        0.0,
    );
    let r4 = run_probe(
        "10x10 x100 contact-cols",
        &[rect(10.0, 10.0)],
        &[100],
        &rect(100.01, 100.01),
        100.01 * 100.01,
        0.0,
    );

    // The consumer acceptance corpus (nest_benchmark.py mixed_cutlist, geometry verbatim; qty =
    // base × 2 → 122 instances, the harness's own scale for a 120×60 sheet at 1.3× oversub),
    // min_sep = the consumer's 0.375-in part gap. Exercises concave L-brackets (concave×concave
    // convolution) and 48-gon discs (decimation path). Utilization here is not apples-to-apples
    // with the harness (no edge-margin erosion, no restarts) — the signal is density-per-time.
    let mixed_items: Vec<Vec<[Scalar; 2]>> = vec![
        rect(24.0, 8.0),
        rect(30.0, 3.0),
        rect(12.0, 12.0),
        vec![
            [0.0, 0.0],
            [16.0, 0.0],
            [16.0, 5.0],
            [5.0, 5.0],
            [5.0, 14.0],
            [0.0, 14.0],
        ],
        vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 3.0],
            [3.0, 3.0],
            [3.0, 9.0],
            [0.0, 9.0],
        ],
        vec![[0.0, 0.0], [12.0, 0.0], [0.0, 12.0]],
        vec![[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]],
        ngon(48, 14.0),
        ngon(48, 8.0),
        rect(18.0, 6.0),
        ngon(6, 10.0),
        vec![[0.0, 0.0], [14.0, 0.0], [11.0, 7.0], [3.0, 7.0]],
        rect(36.0, 2.0),
    ];
    let mixed_qty: Vec<usize> = [4usize, 5, 4, 4, 5, 5, 6, 4, 6, 4, 5, 4, 5]
        .iter()
        .map(|q| q * 2)
        .collect();
    let r5 = run_probe(
        "mixed-13 (consumer shapes)",
        &mixed_items,
        &mixed_qty,
        &rect(120.0, 60.0),
        7200.0,
        0.375,
    );

    println!(
        "\nkill criterion: pentagon >= 24 -> {}",
        if r1.placed >= 24 { "PASS" } else { "FAIL" }
    );
    println!(
        "interlock by construction: 2/2 -> {}",
        if r3.placed == 2 { "PASS" } else { "FAIL" }
    );
    println!(
        "contact columns: 100/100 -> {}",
        if r4.placed == 100 { "PASS" } else { "FAIL" }
    );
    println!(
        "total verify-rejections: {}",
        r1.rejected_candidates
            + r2.rejected_candidates
            + r3.rejected_candidates
            + r4.rejected_candidates
            + r5.rejected_candidates
    );
    let _ = r2.ms;
}

/// Regular n-gon of bbox ≈ size × size anchored near the origin — the consumer harness's
/// `ngon_part`, reproduced. Test-INPUT generation only, so std trig carries the same waiver as
/// the generators in tests/nest.rs (the outline is data, not a placement decision).
#[allow(clippy::disallowed_methods)]
fn ngon(n: usize, size: Scalar) -> Vec<[Scalar; 2]> {
    let r = size / 2.0;
    (0..n)
        .map(|i| {
            let th = 2.0 * std::f64::consts::PI * (i as Scalar) / (n as Scalar);
            [r + r * th.cos(), r + r * th.sin()]
        })
        .collect()
}
