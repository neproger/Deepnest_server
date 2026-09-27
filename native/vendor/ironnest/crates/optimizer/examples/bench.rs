// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-probe wall-clock + density harness for the nester — ground truth for the perf-tuning work
//! (the A-stack: collector reuse, hazkey cache, incremental loss). Reports best-of-N milliseconds
//! per probe so a speedup is visible against the noise floor. `cargo run -p ironnest-optimizer
//! --release --example bench`.
//!
//! DETERMINISM(ironnest): this is a BENCHMARK, never a placement path — the `Instant` wall-clock here
//! only times the engine; it never feeds a placement decision (which is why the determinism gate's
//! `Instant::now` ban does not apply, exactly as the std-trig ban is waived for the test-input
//! generators in `tests/nest.rs`). The placements themselves remain a pure function of `(inputs, seed,
//! budget)`. Timing is reported only to stderr-style stdout text; it is NOT part of any golden.

use ironnest_optimizer::{
    NestConfig, PlacementStrategy, Scalar, SeparationEffort, nest, nest_multistart,
    nest_with_config,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

/// Best-of-`reps` wall-clock (ms) of `f`. Best (min) is the most stable estimator for a CPU-bound,
/// allocation-sensitive routine: it filters scheduler/allocator noise that only ever adds time.
#[allow(clippy::disallowed_methods)] // wall-clock for benchmarking only — never a placement input
fn best_ms<F: FnMut()>(reps: u32, mut f: F) -> Scalar {
    let mut best = Scalar::INFINITY;
    for _ in 0..reps {
        let t = std::time::Instant::now();
        f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        best = best.min(ms);
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn probe(
    label: &str,
    item: Vec<[Scalar; 2]>,
    item_area: Scalar,
    qty: usize,
    side: Scalar,
    budget: u64,
    reps: u32,
    separation_heavy: bool,
) {
    let container = rect(side, side);
    let rotations = [0.0, 90.0, 180.0, 270.0];

    // Capture density once (it is deterministic), then time the call best-of-reps.
    let mut placed = 0usize;
    let ms = best_ms(reps, || {
        let sol = nest(
            std::slice::from_ref(&item),
            &[qty],
            &container,
            &[],
            0.0,
            &rotations,
            1,
            budget,
        );
        placed = sol.placements.len();
    });
    let util = (placed as Scalar * item_area) / (side * side) * 100.0;
    let tag = if separation_heavy { " [sep]" } else { "" };
    println!(
        "{label:<24} placed {placed:>4}/{qty:<4}  util {util:>5.1}%  best {ms:>8.2} ms  (budget {budget}, best of {reps}){tag}"
    );
}

fn main() {
    println!("--- bench (per-probe best-of-N wall-clock; [sep] = separation-search-heavy) ---");
    probe(
        "10x10 in 100 (exact)",
        rect(10.0, 10.0),
        100.0,
        100,
        100.0,
        2000,
        3,
        false,
    );
    probe(
        "10x10 in 100.5 (slack)",
        rect(10.0, 10.0),
        100.0,
        100,
        100.5,
        2000,
        3,
        false,
    );
    probe(
        "7x7 squares",
        rect(7.0, 7.0),
        49.0,
        220,
        100.0,
        2000,
        3,
        false,
    );
    probe(
        "13x7 bricks",
        rect(13.0, 7.0),
        91.0,
        120,
        100.0,
        2000,
        3,
        true,
    );
    let pentagon = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 12.0],
        [12.0, 20.0],
        [0.0, 20.0],
    ];
    probe("pentagon ~344", pentagon, 344.0, 40, 100.0, 4000, 3, true);
    let right_tri = vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]];
    probe(
        "2 right-tris pair->sq",
        right_tri,
        50.0,
        2,
        11.0,
        4000,
        5,
        true,
    );

    println!("\n--- multi-start best-of-K (B1): density vs time on the seed-variant case ---");
    multistart_probe(
        "13x7 bricks",
        rect(13.0, 7.0),
        91.0,
        120,
        100.0,
        2000,
        &[1, 4, 8],
    );

    println!("\n--- multi-start on a MIXED corpus (order diversification lever) ---");
    // Four part types at ~105% area demand: the unplaced tail is guaranteed, so the placed-area
    // utilization directly measures how well the K starts explore different insertion orders.
    // (Single-type probes cannot see the order-diversification lever at all.)
    let mixed_items = vec![
        rect(13.0, 7.0),                            // brick, area 91
        rect(10.0, 10.0),                           // square, area 100
        rect(7.0, 7.0),                             // small square, area 49
        vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]], // right triangle, area 50
    ];
    let mixed_areas = [91.0, 100.0, 49.0, 50.0];
    let mixed_qty = [45usize, 30, 45, 24]; // 4095+3000+2205+1200 = 10500 in 10000 (105%)
    multistart_mixed_probe(
        "mixed 4-type (105%)",
        &mixed_items,
        &mixed_areas,
        &mixed_qty,
        100.0,
        2000,
        &[1, 4, 8],
    );

    println!("\n--- NFP mode (feature #4): full pipeline (NFP construction + sep tail) ---");
    let pentagon = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 12.0],
        [12.0, 20.0],
        [0.0, 20.0],
    ];
    nfp_probe(
        "pentagon ~344 (NFP)",
        &[pentagon],
        &[344.0],
        &[40],
        100.0,
        &[1, 8],
    );
    nfp_probe(
        "13x7 bricks (NFP)",
        &[rect(13.0, 7.0)],
        &[91.0],
        &[120],
        100.0,
        &[1, 8],
    );
    nfp_probe(
        "mixed 4-type (NFP)",
        &mixed_items,
        &mixed_areas,
        &mixed_qty,
        100.0,
        &[1, 8, 16],
    );
    nfp_cache_probe();
}

/// The NFP cache-build cost on a 13-type × 4-rotation corpus (docs/03 §9.4 gate: cache build ≤ 5 %
/// of a budget-500-equivalent nest). Measures the ONCE-PER-CALL cost every consumer nest pays.
fn nfp_cache_probe() {
    use ironnest_optimizer::nfp;
    // 13 heterogeneous plate shapes (sizes matching the consumer harness's spread).
    let mut items: Vec<Vec<[Scalar; 2]>> = vec![
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
        rect(18.0, 6.0),
        vec![[0.0, 0.0], [14.0, 0.0], [11.0, 7.0], [3.0, 7.0]],
        rect(36.0, 2.0),
        rect(9.0, 9.0),
        rect(20.0, 4.0),
        vec![
            [0.0, 0.0],
            [11.0, 0.0],
            [11.0, 11.0],
            [5.5, 15.0],
            [0.0, 11.0],
        ],
    ];
    items.truncate(13);
    let qty: Vec<usize> = items.iter().map(|_| 1).collect();
    let container = rect(120.0, 60.0);
    let rots: Vec<Vec<Scalar>> = items
        .iter()
        .map(|_| vec![0.0, 90.0, 180.0, 270.0])
        .collect();
    // Build cost is measured through a minimal-qty nest (import + cache build dominate).
    let cfg = ironnest_optimizer::NestConfig {
        min_sep: 0.375,
        seed: 1,
        budget: 1,
        restarts: 1,
        strategy: ironnest_optimizer::PlacementStrategy::Nfp,
        separation_effort: ironnest_optimizer::SeparationEffort::Full,
        column_weight: 10,
    };
    let mut pair_count = 0usize;
    let ms = best_ms(3, || {
        let sol = ironnest_optimizer::nest_with_config(&items, &qty, &container, &[], &rots, &cfg)
            .expect("in-domain");
        pair_count = sol.placements.len(); // keep the call un-elided
    });
    let _ = pair_count;
    let _ = &nfp::NFP_BACKOFF_E; // anchor: the doc-hidden module is intentionally exercised
    println!(
        "\n--- NFP cache probe: 13 types × 4 rots (import + full cache build + 13 placements) ---"
    );
    println!("nfp-cache mixed-13     best {ms:>8.2} ms  (build once per nest() call)");
}

/// Full-pipeline NFP-mode probe at several K, comparable against the sampling rows above.
fn nfp_probe(
    label: &str,
    items: &[Vec<[Scalar; 2]>],
    areas: &[Scalar],
    qty: &[usize],
    side: Scalar,
    ks: &[usize],
) {
    let container = rect(side, side);
    let rots: Vec<Vec<Scalar>> = items
        .iter()
        .map(|_| vec![0.0, 90.0, 180.0, 270.0])
        .collect();
    for &k in ks {
        let cfg = NestConfig {
            min_sep: 0.0,
            seed: 1,
            budget: 2000, // drives only the sampling fallback + the sep tail's insertion attempts
            restarts: k,
            strategy: PlacementStrategy::Nfp,
            separation_effort: SeparationEffort::Full,
            column_weight: 10,
        };
        let mut placed = 0usize;
        let mut placed_area = 0.0;
        let ms = best_ms(1, || {
            let sol = nest_with_config(items, qty, &container, &[], &rots, &cfg)
                .expect("bench corpus is in-domain");
            placed = sol.placements.len();
            placed_area = sol.placements.iter().map(|p| areas[p.item]).sum();
        });
        let total: usize = qty.iter().sum();
        let util = placed_area / (side * side) * 100.0;
        println!(
            "{label:<18} K={k:<3} placed {placed:>4}/{total:<4}  util {util:>5.1}%  {ms:>9.1} ms"
        );
    }
}

/// [`multistart_probe`] over a heterogeneous corpus: reports placed-AREA utilization (the
/// multi-start objective) rather than a single type's count.
fn multistart_mixed_probe(
    label: &str,
    items: &[Vec<[Scalar; 2]>],
    areas: &[Scalar],
    qty: &[usize],
    side: Scalar,
    budget: u64,
    ks: &[usize],
) {
    let container = rect(side, side);
    let rotations = [0.0, 90.0, 180.0, 270.0];
    for &k in ks {
        let mut placed = 0usize;
        let mut placed_area = 0.0;
        let ms = best_ms(1, || {
            let sol = nest_multistart(items, qty, &container, &[], 0.0, &rotations, 1, budget, k);
            placed = sol.placements.len();
            placed_area = sol.placements.iter().map(|p| areas[p.item]).sum();
        });
        let total: usize = qty.iter().sum();
        let util = placed_area / (side * side) * 100.0;
        println!(
            "{label:<18} K={k:<3} placed {placed:>4}/{total:<4}  util {util:>5.1}%  {ms:>9.1} ms"
        );
    }
}

/// Runs `nest_multistart` at several K and reports the best-of-K density + total wall-clock, so the
/// density/time trade of the multi-start lever is visible. K=1 is the single-start reference.
fn multistart_probe(
    label: &str,
    item: Vec<[Scalar; 2]>,
    item_area: Scalar,
    qty: usize,
    side: Scalar,
    budget: u64,
    ks: &[usize],
) {
    let container = rect(side, side);
    let rotations = [0.0, 90.0, 180.0, 270.0];
    for &k in ks {
        let mut placed = 0usize;
        let ms = best_ms(1, || {
            let sol = nest_multistart(
                std::slice::from_ref(&item),
                &[qty],
                &container,
                &[],
                0.0,
                &rotations,
                1,
                budget,
                k,
            );
            placed = sol.placements.len();
        });
        let util = (placed as Scalar * item_area) / (side * side) * 100.0;
        println!(
            "{label:<18} K={k:<3} placed {placed:>4}/{qty:<4}  util {util:>5.1}%  {ms:>9.1} ms"
        );
    }
}
