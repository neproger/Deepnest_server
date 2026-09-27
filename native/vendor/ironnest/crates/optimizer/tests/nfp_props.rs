// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Property tests for the exact-NFP substrate (docs/03 §9.1) against the CDE — the feasibility
//! arbiter. P1 (soundness — every region candidate must pass CDE verification, zero tolerance) is
//! the load-bearing test; plus the concave-container IFP check (the inverted-failure-direction
//! path, docs/03 §2), byte-determinism double-runs, the cardinal swap/negate ≤ 1-grid bound, and
//! the input-domain hard error.
//!
//! Drives the doc-hidden `ironnest_optimizer::nfp` module (test hook, NOT a stable API), with all
//! geometry built through the same `Importer` the engine uses.

use ironnest_cde::collision_detection::CDEConfig;
use ironnest_cde::collision_detection::hazards::filter::NoFilter;
use ironnest_cde::entities::{Container, Item, Layout};
use ironnest_cde::geometry::fail_fast::SPSurrogateConfig;
use ironnest_cde::geometry::geo_traits::{Transformable, TransformableFrom};
use ironnest_cde::io::ext_repr::{ExtContainer, ExtItem, ExtSPolygon, ExtShape};
use ironnest_cde::io::import::Importer;
use ironnest_geo::{DTransformation, Scalar, Transformation};
use ironnest_optimizer::nfp::geom::{IPt, quantize, unquantize};
use ironnest_optimizer::nfp::minkowski::Shapes;
use ironnest_optimizer::nfp::{NfpCache, NfpError, PlacedObstacle, candidates};

const CARDINAL: [Scalar; 4] = [0.0, 90.0, 180.0, 270.0];

fn cde_config() -> CDEConfig {
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

/// `(entities, container, rotations_deg, rotations_rad)` — the imported-problem tuple.
type Imported = (
    Vec<Option<Item>>,
    Container,
    Vec<Vec<Scalar>>,
    Vec<Vec<Scalar>>,
);

/// Imports a problem exactly the way the engine does (min_sep inflation + decimation switch).
fn import(outlines: &[Vec<[Scalar; 2]>], container: &[[Scalar; 2]], min_sep: Scalar) -> Imported {
    let mut importer = Importer::new(cde_config(), None, (min_sep > 0.0).then_some(min_sep), None);
    importer.shape_modify_config.collision_decimation = (min_sep > 0.0).then_some(min_sep / 16.0);
    let cont = importer
        .import_container(&ExtContainer {
            id: 0,
            shape: ExtShape::SimplePolygon(ExtSPolygon(
                container.iter().map(|p| (p[0], p[1])).collect(),
            )),
            zones: vec![],
        })
        .unwrap();
    let entities: Vec<Option<Item>> = outlines
        .iter()
        .enumerate()
        .map(|(i, o)| {
            importer
                .import_item(&ExtItem {
                    id: i as u64,
                    allowed_orientations: Some(CARDINAL.to_vec()),
                    shape: ExtShape::SimplePolygon(ExtSPolygon(
                        o.iter().map(|p| (p[0], p[1])).collect(),
                    )),
                    min_quality: None,
                })
                .ok()
        })
        .collect();
    let deg: Vec<Vec<Scalar>> = outlines.iter().map(|_| CARDINAL.to_vec()).collect();
    let rad: Vec<Vec<Scalar>> = deg
        .iter()
        .map(|r| r.iter().map(|d| d.to_radians()).collect())
        .collect();
    (entities, cont, deg, rad)
}

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

/// True iff placing `item` at `(rot_rad, x, y)` collides with nothing in `layout`'s CDE — the
/// arbiter check, verbatim engine semantics.
fn cde_feasible(layout: &Layout, item: &Item, rot_rad: Scalar, x: Scalar, y: Scalar) -> bool {
    let mut buffer = (*item.shape_cd).clone();
    buffer.surrogate = None;
    let transf = DTransformation::new(rot_rad, (x, y)).compose();
    buffer.transform_from(&item.shape_cd, &transf);
    !layout.cde().detect_poly_collision(&buffer, &NoFilter)
}

/// P1 SOUNDNESS (zero tolerance): every candidate the region emits must pass CDE verification —
/// on an empty layout (pure IFP), and again after obstacles are placed (IFP ∖ NFPs). Exercised on
/// convex (pentagon/brick) and CONCAVE (L-bracket) parts, at min_sep 0 and 0.375, on a rectangular
/// and a concave L-shaped container.
#[test]
fn p1_soundness_all_candidates_cde_feasible() {
    let pentagon = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 12.0],
        [12.0, 20.0],
        [0.0, 20.0],
    ];
    let l_bracket = vec![
        [0.0, 0.0],
        [16.0, 0.0],
        [16.0, 5.0],
        [5.0, 5.0],
        [5.0, 14.0],
        [0.0, 14.0],
    ];
    let brick = rect(13.0, 7.0);
    let l_container = vec![
        [0.0, 0.0],
        [100.0, 0.0],
        [100.0, 50.0],
        [50.0, 50.0],
        [50.0, 100.0],
        [0.0, 100.0],
    ];

    for (container, min_sep) in [
        (rect(100.0, 100.0), 0.0),
        (rect(100.0, 100.0), 0.375),
        (l_container.clone(), 0.0),
        (l_container, 0.375),
    ] {
        let (entities, cont, deg, rad) = import(
            &[pentagon.clone(), l_bracket.clone(), brick.clone()],
            &container,
            min_sep,
        );
        let cache = NfpCache::build(&entities, &deg, &rad, &cont).unwrap();
        let mut layout = Layout::new(cont.clone());
        let mut placed: Vec<PlacedObstacle> = Vec::new();
        let mut checked = 0usize;

        // Greedy: place up to 6 instances round-robin over (type, rotation), always at the best
        // candidate; before placing, verify EVERY candidate we walk past (cap per step).
        for step in 0..6 {
            let t = step % entities.len();
            let r = step % 4;
            let Some(item) = entities[t].as_ref() else {
                continue;
            };
            let region = cache.feasible_region(t, r, &placed);
            let cands = candidates(&region, cache.loss_offsets(t, r), 10);
            for &(_, x, y) in cands.iter().take(60) {
                assert!(
                    cde_feasible(&layout, item, rad[t][r], unquantize(x), unquantize(y)),
                    "P1 violated: candidate ({x},{y}) type {t} rot {r} min_sep {min_sep} rejected by CDE",
                );
                checked += 1;
            }
            if let Some(&(_, x, y)) = cands.first() {
                layout.place_item(
                    item,
                    DTransformation::new(rad[t][r], (unquantize(x), unquantize(y))),
                );
                placed.push(PlacedObstacle {
                    item_id: t,
                    rot_idx: r,
                    tx: x,
                    ty: y,
                });
            }
        }
        assert!(
            checked > 50,
            "P1 must actually exercise candidates (got {checked})"
        );
    }
}

/// Exact point-in-shapes test (NonZero irrelevant — canonical shapes are disjoint outer/holes).
fn point_in_ring(ring: &[IPt], p: IPt) -> bool {
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

fn point_in_shapes(shapes: &Shapes, p: IPt) -> bool {
    shapes
        .iter()
        .any(|shape| point_in_ring(&shape[0], p) && !shape[1..].iter().any(|h| point_in_ring(h, p)))
}

/// P2-lite on the CONCAVE container (the inverted-failure-direction path, docs/03 §2): every
/// CDE-feasible pose on a coarse scan grid must lie inside the IFP dilated by a small L∞ margin
/// (E + quantization slack). A lost IFP loop or an under-permissive IFP fails here.
#[test]
fn p2_concave_container_ifp_not_under_permissive() {
    let l_container = vec![
        [0.0, 0.0],
        [100.0, 0.0],
        [100.0, 50.0],
        [50.0, 50.0],
        [50.0, 100.0],
        [0.0, 100.0],
    ];
    let brick = rect(13.0, 7.0);
    let (entities, cont, deg, rad) = import(&[brick], &l_container, 0.0);
    let cache = NfpCache::build(&entities, &deg, &rad, &cont).unwrap();
    let layout = Layout::new(cont.clone());
    let item = entities[0].as_ref().unwrap();

    // Slack: E (16) + representation error; 64 grid units ≈ 6e-5 units is generous and still
    // far below the scan resolution.
    let slack = 64i64;
    #[allow(clippy::needless_range_loop)] // r keys region, rad, and the assertion label together
    for r in 0..4usize {
        let region = cache.feasible_region(0, r, &[]);
        let mut feasible_seen = 0;
        for gx in 0..20 {
            for gy in 0..20 {
                let (x, y) = (5.0 * f64::from(gx) + 2.5, 5.0 * f64::from(gy) + 2.5);
                if cde_feasible(&layout, item, rad[0][r], x, y) {
                    feasible_seen += 1;
                    // Dilate by checking the four L∞-shifted probes too (any hit counts).
                    let p = IPt::new(quantize(x), quantize(y));
                    let hit = [(0, 0), (slack, 0), (-slack, 0), (0, slack), (0, -slack)]
                        .iter()
                        .any(|&(dx, dy)| point_in_shapes(&region, IPt::new(p.x + dx, p.y + dy)));
                    assert!(
                        hit,
                        "CDE-feasible pose ({x},{y}) rot {r} missing from the IFP region"
                    );
                }
            }
        }
        assert!(
            feasible_seen > 50,
            "scan must find feasible poses (rot {r})"
        );
    }
}

/// P8: byte-determinism — two independent builds produce identical regions and candidate walks.
#[test]
fn p8_double_build_is_byte_identical() {
    let parts = [rect(13.0, 7.0), rect(10.0, 10.0)];
    let container = rect(60.0, 60.0);
    let (e1, c1, d1, r1) = import(&parts, &container, 0.375);
    let (e2, c2, d2, r2) = import(&parts, &container, 0.375);
    let cache1 = NfpCache::build(&e1, &d1, &r1, &c1).unwrap();
    let cache2 = NfpCache::build(&e2, &d2, &r2, &c2).unwrap();
    assert_eq!(cache1.pair_count(), cache2.pair_count());

    let placed = [PlacedObstacle {
        item_id: 0,
        rot_idx: 1,
        tx: quantize(20.0),
        ty: quantize(20.0),
    }];
    for t in 0..2 {
        for r in 0..4 {
            let reg1 = cache1.feasible_region(t, r, &placed);
            let reg2 = cache2.feasible_region(t, r, &placed);
            assert_eq!(reg1, reg2, "regions must be byte-identical (t={t}, r={r})");
            assert_eq!(
                candidates(&reg1, cache1.loss_offsets(t, r), 10),
                candidates(&reg2, cache2.loss_offsets(t, r), 10),
            );
        }
    }
}

/// Cardinal swap/negate vs libm-rotate-then-quantize: ≤ 1 grid unit per coordinate (the
/// review-corrected knife-edge bound; docs/03 §3.1).
#[test]
fn cardinal_swap_negate_within_one_grid_of_libm() {
    let pentagon = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 12.0],
        [12.0, 20.0],
        [0.0, 20.0],
    ];
    let (entities, cont, deg, rad) = import(&[pentagon], &rect(100.0, 100.0), 0.0);
    let cache = NfpCache::build(&entities, &deg, &rad, &cont).unwrap();
    let item = entities[0].as_ref().unwrap();

    #[allow(clippy::needless_range_loop)] // r keys cache, rad, and the assertion label together
    for r in 1..4usize {
        let cached = cache.footprint(0, r).unwrap();
        let rot = Transformation::from_rotation(rad[0][r]);
        for (i, p) in item.shape_cd.vertices.iter().enumerate() {
            let mut q = *p;
            q.transform(&rot);
            let (lx, ly) = (quantize(q.0), quantize(q.1));
            let c = cached[i];
            assert!(
                (c.x - lx).abs() <= 1 && (c.y - ly).abs() <= 1,
                "rot {r} vertex {i}: swap/negate ({},{}) vs libm ({lx},{ly})",
                c.x,
                c.y,
            );
        }
    }
}

/// The input-domain gate hard-errors instead of silently degrading (docs/03 §3.1).
#[test]
fn input_domain_gate_errors_on_huge_coordinates() {
    // 3e12 units × 2^20 ≈ 2^61.4 grid on the container span — past the 2^61 gate.
    let big = 3.0e12;
    let (entities, cont, deg, rad) = import(&[rect(big, big)], &rect(big * 2.0, big * 2.0), 0.0);
    match NfpCache::build(&entities, &deg, &rad, &cont) {
        Err(NfpError::InputDomain { .. }) => {}
        Ok(_) => panic!("expected InputDomain error"),
    }
}
