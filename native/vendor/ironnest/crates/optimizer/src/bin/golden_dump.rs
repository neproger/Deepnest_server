// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Canonical solver-output dump for the cross-platform determinism golden (Phase 3, the headline
//! contract: byte-identical placements on macOS-arm64 == Windows-x64 == linux-x64).
//!
//! Runs a fixed corpus of nests and prints the placements as plain `item x y rot` lines. This is the
//! single source of truth for both golden tests (`tests/golden.rs`): the insta snapshot (level 3 +
//! the cross-platform gate — every CI platform must reproduce the committed `.snap`) and the
//! cross-subprocess byte-diff (level 2). Run it by hand to inspect / re-bless:
//! `cargo run -p ironnest-optimizer --bin golden_dump`.
//!
//! DETERMINISM(ironnest):
//! - The corpus includes a **nonzero-`min_sep`** case (`separated-squares`). Separation routes
//!   through the *vendored, libm-deterministic* offsetter (`ironnest_geo::buffer`, ex-`geo-buffer`),
//!   so these layouts are now byte-identical cross-platform too — this case is the standing proof
//!   that docs/00 risk #2 is resolved.
//! - Coordinates are printed with the default `f64` `Display` (Rust's pure-Rust shortest-round-trip
//!   `flt2dec`), which is a deterministic, **injective** function of the bits: two different f64
//!   values never print the same string, so the text diff is a faithful byte-identity check.
//! - The placement emit order is `nest`'s own order (slotmap slot order — a pure function of the
//!   deterministic op history); it is part of the contract, so it is dumped unsorted.

use ironnest_optimizer::{
    NestConfig, Placement, PlacementStrategy, Scalar, SeparationEffort, nest, nest_multistart,
    nest_per_item, nest_with_config,
};
use std::fmt::Write as _;

/// `w × h` axis-aligned rectangle, lower-left at the origin (CCW).
fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

/// A 64-vertex CCW "circle" of radius `r` centered at the origin — a high-vertex curved part that
/// stands in for the consumer's hundreds-of-vertex developed-cone shells and triggers collision
/// decimation (64 > `DECIMATION_MIN_VERTICES`).
///
/// DETERMINISM(ironnest): the golden_dump output must be byte-identical on every platform, so the
/// *input geometry* must be too. We therefore build the vertices with a fixed rotation recurrence
/// using only `+ − ×` on the **f64 literals** `cos(π/32)` / `sin(π/32)` — never std `sin`/`cos` at
/// dump time (whose platform libm would diverge). Literals + IEEE arithmetic ⇒ identical bits
/// everywhere; the recurrence's sub-ULP radius drift over 64 steps is immaterial (it is still a fixed,
/// valid, curved polygon — the point is a many-vertex convex outline, not a perfect circle).
fn circle_64(r: Scalar) -> Vec<[Scalar; 2]> {
    const C: Scalar = 0.9951847266721969; // cos(π/32)
    const S: Scalar = 0.0980171403295606; // sin(π/32)
    let mut pts = Vec::with_capacity(64);
    let (mut x, mut y) = (r, 0.0);
    for _ in 0..64 {
        pts.push([x, y]);
        (x, y) = (C * x - S * y, S * x + C * y);
    }
    pts
}

const CARDINAL: [Scalar; 4] = [0.0, 90.0, 180.0, 270.0];

/// A case's allowed rotations: either one set for every item (the [`nest`] path) or a distinct set
/// per item type (the [`nest_per_item`] path). Both are exercised by the cross-platform golden.
enum Rotations {
    Uniform(Vec<Scalar>),
    PerItem(Vec<Vec<Scalar>>),
}

/// One golden case. Everything here is fixed — seeds, budgets, geometry — so the output is a pure
/// function of the engine.
struct Case {
    name: &'static str,
    items: Vec<Vec<[Scalar; 2]>>,
    qty: Vec<usize>,
    container: Vec<[Scalar; 2]>,
    holes: Vec<Vec<[Scalar; 2]>>,
    min_sep: Scalar,
    rotations: Rotations,
    seed: u64,
    budget: u64,
}

fn corpus() -> Vec<Case> {
    let pentagon = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 12.0],
        [12.0, 20.0],
        [0.0, 20.0],
    ];
    let right_tri = vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]];
    // A 60×60 sheet with a central 20×20 keep-out hole at (20,20)–(40,40): parts must nest around it.
    let center_hole = vec![[20.0, 20.0], [40.0, 20.0], [40.0, 40.0], [20.0, 40.0]];
    vec![
        // Construction + compaction over two part types and full cardinal rotation.
        Case {
            name: "mixed-rects",
            items: vec![rect(10.0, 10.0), rect(20.0, 5.0)],
            qty: vec![6, 4],
            container: rect(60.0, 60.0),
            holes: vec![],
            min_sep: 0.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 7,
            budget: 1500,
        },
        // The no-rotation path (rotations empty ⇒ 0° only).
        Case {
            name: "no-rotation-squares",
            items: vec![rect(10.0, 10.0)],
            qty: vec![4],
            container: rect(50.0, 50.0),
            holes: vec![],
            min_sep: 0.0,
            rotations: Rotations::Uniform(vec![]),
            seed: 1,
            budget: 800,
        },
        // Separation search must discover the interlocked pairing (one triangle rotated 180°).
        Case {
            name: "interlock-triangles",
            items: vec![right_tri],
            qty: vec![2],
            container: rect(11.0, 11.0),
            holes: vec![],
            min_sep: 0.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 42,
            budget: 2000,
        },
        // Separation search on an irregular part at meaningful demand.
        Case {
            name: "pentagon",
            items: vec![pentagon],
            qty: vec![10],
            container: rect(80.0, 80.0),
            holes: vec![],
            min_sep: 0.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 3,
            budget: 2000,
        },
        // Interior-void path: parts must avoid the central keep-out hole (Phase 6). Quantity kept
        // low so construction places them all (no slow debug separation) — the point is to exercise
        // the holes path deterministically in the cross-platform golden.
        Case {
            name: "sheet-with-hole",
            items: vec![rect(10.0, 10.0)],
            qty: vec![12],
            container: rect(60.0, 60.0),
            holes: vec![center_hole],
            min_sep: 0.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 5,
            budget: 1000,
        },
        // Nonzero min-separation path: each part is inflated by min_sep/2 via the vendored, libm-
        // deterministic offsetter (ex-geo-buffer). This is the proof that resolving docs/00 risk #2
        // makes separated layouts byte-identical across platforms.
        Case {
            name: "separated-squares",
            items: vec![rect(10.0, 10.0)],
            qty: vec![4],
            container: rect(60.0, 60.0),
            holes: vec![],
            min_sep: 4.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 8,
            budget: 1000,
        },
        // Collision-footprint decimation path (Inflate / item side): a high-vertex (64-gon) curved part
        // at nonzero min_sep. Its collision footprint is Douglas–Peucker-simplified then offset by
        // min_sep/2 + tol — both the DP (pure +−×÷) and the offset (vendored libm offsetter) are
        // cross-platform-deterministic, so this layout is byte-identical on every target too. The
        // reported placements are in the *original* 64-gon frame. (Proof decimation kept determinism.)
        Case {
            name: "decimated-circles",
            items: vec![circle_64(12.0)],
            qty: vec![4],
            container: rect(60.0, 60.0),
            holes: vec![],
            min_sep: 1.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 8,
            budget: 1000,
        },
        // Decimation path on the Deflate / container side: a high-vertex (64-gon) curved CONTAINER is
        // DP-simplified then deflated by min_sep/2 + tol. Exercises the symmetric container/boundary
        // over-reservation across platforms; simple square parts (4 vtx) stay on the exact path.
        Case {
            name: "decimated-curved-container",
            items: vec![rect(8.0, 8.0)],
            qty: vec![6],
            container: circle_64(30.0),
            holes: vec![],
            min_sep: 1.0,
            rotations: Rotations::Uniform(CARDINAL.to_vec()),
            seed: 4,
            budget: 1000,
        },
        // Per-item rotation sets (the `nest_per_item` path): two part types with DIFFERENT allowed
        // orientations in ONE nest — the square pinned axis-aligned (`{0, 90}`), the right triangle
        // free to interlock on a fine 45° step (`{0, 45, …, 315}`). The non-cardinal angles route
        // through the SAME `libm::sincos` as the cardinal path (Transformation::from_rotation), so this
        // layout is byte-identical cross-platform too — the standing proof that per-item rotation sets
        // (and arbitrary non-cardinal angles) preserve the determinism contract.
        Case {
            name: "per-item-rotations",
            items: vec![rect(10.0, 10.0), vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]],
            qty: vec![4, 6],
            container: rect(40.0, 40.0),
            holes: vec![],
            min_sep: 0.0,
            rotations: Rotations::PerItem(vec![
                vec![0.0, 90.0],
                vec![0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0],
            ]),
            seed: 11,
            budget: 1500,
        },
    ]
}

/// Renders the full corpus to canonical text. Each case is a `# name` header, one `item x y rot`
/// line per placement (in `nest`'s emit order), then an `unplaced <ids…>` line.
fn dump() -> String {
    let mut out = String::new();
    for case in corpus() {
        writeln!(out, "# {}", case.name).unwrap();
        let sol = match &case.rotations {
            Rotations::Uniform(r) => nest(
                &case.items,
                &case.qty,
                &case.container,
                &case.holes,
                case.min_sep,
                r,
                case.seed,
                case.budget,
            ),
            Rotations::PerItem(r) => nest_per_item(
                &case.items,
                &case.qty,
                &case.container,
                &case.holes,
                case.min_sep,
                r,
                case.seed,
                case.budget,
            ),
        };
        write_solution(&mut out, &sol.placements, &sol.unplaced);
    }

    // Multi-start (best-of-K) determinism case — APPENDED after the corpus so every case above stays
    // byte-for-byte (the K=1 path already equals `nest`; this exercises the K>1 best-of-area reduction
    // over two heterogeneous item types and must be byte-identical on every platform too).
    writeln!(out, "# multistart-mixed").unwrap();
    let ms = nest_multistart(
        &[rect(10.0, 10.0), rect(20.0, 5.0)],
        &[4, 3],
        &rect(50.0, 50.0),
        &[],
        0.0,
        &CARDINAL,
        13,
        800,
        3,
    );
    write_solution(&mut out, &ms.placements, &ms.unplaced);

    // Multi-start where the JITTERED-order start wins (the order-diversification path): overflow
    // demand (946 area in 900) so the starts genuinely differ, and at this seed the best-of-3 argmax
    // keeps start k=2 — the trailing jittered block of the contiguous-block policy (verified:
    // per-start placed areas [646, 646, 673]). This case is the cross-platform byte-stability PROOF
    // for the jittered path (`start_order_policy`), which the all-placing `multistart-mixed` case
    // above cannot: its starts tie on placed area, so the remnant-first `NestScore` tie-break
    // (min used_x_max) — not the insertion-order diversification — decides that case's winner.
    writeln!(out, "# multistart-overflow").unwrap();
    let mo = nest_multistart(
        &[rect(13.0, 7.0), rect(10.0, 10.0)],
        &[6, 4],
        &rect(30.0, 30.0),
        &[],
        0.0,
        &CARDINAL,
        4,
        500,
        3,
    );
    write_solution(&mut out, &mo.placements, &mo.unplaced);

    // --- NFP-mode cases (feature #4, docs/03 §9.2) — APPENDED, existing cases byte-frozen. -----
    // The integer NFP pipeline is byte-deterministic by construction; these cases are the
    // cross-platform PROOF for: contact-quality placement (the exact-fit artifact killer), the
    // concave-pocket NFP-hole path, the concave-container + keep-out IFP classification, the
    // min_sep-inflated footprint path, and the DP-decimation → offset → quantize chain. All fully
    // place in construction (no debug-CI separation cost); `nfp-multistart` follows in N3 with the
    // policy it exercises.
    let nfp_cfg = |min_sep: Scalar, seed: u64| NestConfig {
        min_sep,
        seed,
        budget: 400,
        restarts: 1,
        strategy: PlacementStrategy::Nfp,
        separation_effort: SeparationEffort::Full,
        column_weight: 10,
    };
    let per_item = |n: usize| -> Vec<Vec<Scalar>> { (0..n).map(|_| CARDINAL.to_vec()).collect() };

    // Contact-quality: 9 squares of 10 in a 30.01 sheet — 3 columns of 3, ~100 % util (the
    // sampling engine's touching-artifact makes this impossible for it).
    writeln!(out, "# nfp-contact-columns").unwrap();
    let sol = nest_with_config(
        &[rect(10.0, 10.0)],
        &[9],
        &rect(30.01, 30.01),
        &[],
        &per_item(1),
        &nfp_cfg(0.0, 1),
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // Concave pocket: a C-part whose 12×12 pocket must host one of the two 8×8 squares.
    writeln!(out, "# nfp-concave-pocket").unwrap();
    let c_part = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 4.0],
        [8.0, 4.0],
        [8.0, 16.0],
        [20.0, 16.0],
        [20.0, 20.0],
        [0.0, 20.0],
    ];
    let sol = nest_with_config(
        &[c_part, rect(8.0, 8.0)],
        &[1, 2],
        &rect(42.0, 30.0),
        &[],
        &per_item(2),
        &nfp_cfg(0.0, 2),
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // Concave L-container + keep-out hole: the face-classified IFP + zone folding, end to end.
    writeln!(out, "# nfp-l-sheet-hole").unwrap();
    let l_container = vec![
        [0.0, 0.0],
        [60.0, 0.0],
        [60.0, 30.0],
        [30.0, 30.0],
        [30.0, 60.0],
        [0.0, 60.0],
    ];
    let keepout = vec![[12.0, 12.0], [20.0, 12.0], [20.0, 20.0], [12.0, 20.0]];
    let sol = nest_with_config(
        &[rect(13.0, 7.0)],
        &[6],
        &l_container,
        &[keepout],
        &per_item(1),
        &nfp_cfg(0.0, 3),
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // min_sep through the whole integer pipeline (inflated footprints → NFP → □E → verify).
    writeln!(out, "# nfp-min-sep").unwrap();
    let sol = nest_with_config(
        &[rect(13.0, 7.0), rect(10.0, 10.0)],
        &[4, 2],
        &rect(45.0, 45.0),
        &[],
        &per_item(2),
        &nfp_cfg(0.375, 5),
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // NFP-mode multi-start where the JITTERED start wins: overflow demand, and at this seed the
    // best-of-3 argmax keeps start k=2 (the trailing jittered block) — verified: the K=3 result
    // differs from BOTH canonical starts (K=1 at seed and seed+1), placed areas 673 vs 646. This
    // pins the NFP-mode multistart path (shared read-only cache incl. under `--features parallel`,
    // block policy, jitter) into the cross-platform byte-identity contract.
    writeln!(out, "# nfp-multistart").unwrap();
    let sol = nest_with_config(
        &[rect(13.0, 7.0), rect(10.0, 10.0)],
        &[6, 4],
        &rect(30.0, 30.0),
        &[],
        &per_item(2),
        &NestConfig {
            min_sep: 0.0,
            seed: 3,
            budget: 500,
            restarts: 3,
            strategy: PlacementStrategy::Nfp,
            separation_effort: SeparationEffort::Full,
            column_weight: 10,
        },
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // Decimated curved part at min_sep > 0: DP-decimation → offset → quantize → NFP chain.
    writeln!(out, "# nfp-decimated-circle").unwrap();
    let sol = nest_with_config(
        &[circle_64(12.0)],
        &[3],
        &rect(60.0, 60.0),
        &[],
        &per_item(1),
        &nfp_cfg(1.0, 8),
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // Fast separation tier (`SeparationEffort::Fast`, perf Tier 0, docs/04) — a DELIBERATELY
    // placement-changing result vs `Full`, pinned into the cross-platform byte-identity contract on
    // its own terms (the pure-integer NFP construction + the seeded, fixed-budget separator are as
    // deterministic under the reduced Fast budget as under Full). Two cases pin the two Fast branches:
    //
    // (a) reduced-budget separator: 12 squares of 100 into a 25×25 = 625 sheet — only 4 fit
    //     geometrically (2×2 in 20×20), so the 8 leftovers each run the REDUCED per-insert separator
    //     budget (SEP_CONFIG_FAST / SEED_SAMPLES_FAST) and fail. Placed area (400) stays below the
    //     container (625), so the area-skip does NOT fire here — this case locks the Fast PRNG path.
    writeln!(out, "# nfp-fast-reduced-budget").unwrap();
    let fast_cfg = |container_side: Scalar, qty: usize| {
        (
            vec![rect(10.0, 10.0)],
            vec![qty],
            rect(container_side, container_side),
        )
    };
    let (items, q, cont) = fast_cfg(25.0, 12);
    let nfp_fast = |seed: u64| NestConfig {
        min_sep: 0.0,
        seed,
        budget: 400,
        restarts: 1,
        strategy: PlacementStrategy::Nfp,
        separation_effort: SeparationEffort::Fast,
        column_weight: 10,
    };
    let sol =
        nest_with_config(&items, &q, &cont, &[], &per_item(1), &nfp_fast(7)).expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // (b) area-conservation hopeless-skip: 12 squares of 100 into a 30.01×30.01 ≈ 900.6 sheet — the
    //     0.01 slack lets the □E-backoff contact placement seat all 9 (perfect 3×3 grid, area 900), so
    //     the 3 leftovers each trip the area-skip (900 + 100 > 900.6) and are dropped WITHOUT running
    //     the separator. Locks the sound Tier 0a skip path. (An exact 30.0 sheet seats only 4 — the
    //     backoff denies 3-in-a-row — which is why contact-columns above also uses 30.01.)
    writeln!(out, "# nfp-fast-area-skip").unwrap();
    let (items, q, cont) = fast_cfg(30.01, 12);
    let sol =
        nest_with_config(&items, &q, &cont, &[], &per_item(1), &nfp_fast(7)).expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    // Max densification tier (`SeparationEffort::Max`, docs/04 Tier D): a small all-fit pack with
    // horizontal slack — densify shrinks the sheet toward its corner and re-separates the whole
    // layout (moving already-placed parts + sampling rotations) to compact the pack and enlarge the
    // end remnant, then refills. A DELIBERATELY placement-changing tier; this case pins its
    // cross-platform byte-identity contract (the shrink/separate/refill loop is a pure function of
    // the seeded PRNG + integer geometry).
    writeln!(out, "# nfp-max-densify").unwrap();
    let sol = nest_with_config(
        &[rect(18.0, 8.0), rect(8.0, 8.0)],
        &[4, 8],
        &rect(90.0, 20.0),
        &[],
        &per_item(2),
        &NestConfig {
            min_sep: 0.0,
            seed: 5,
            budget: 400,
            restarts: 1,
            strategy: PlacementStrategy::Nfp,
            separation_effort: SeparationEffort::Max,
            column_weight: 10,
        },
    )
    .expect("in-domain");
    write_solution(&mut out, &sol.placements, &sol.unplaced);

    out
}

/// Renders one solution to the canonical `item x y rot` lines plus the `unplaced …` line.
fn write_solution(out: &mut String, placements: &[Placement], unplaced: &[usize]) {
    for Placement {
        item,
        x,
        y,
        rotation_deg,
    } in placements
    {
        writeln!(out, "{item} {x} {y} {rotation_deg}").unwrap();
    }
    write!(out, "unplaced").unwrap();
    for id in unplaced {
        write!(out, " {id}").unwrap();
    }
    writeln!(out).unwrap();
}

fn main() {
    print!("{}", dump());
}
