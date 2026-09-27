// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Throwaway Tier 2 bench (docs/04): the "part-in-part" workload — nest one 13-type library into M
//! containers. `per-call` rebuilds the O(P^2) pairwise NFP table every time; `prepared` builds it
//! once via prepare() + nest_with_prepared(). Reports the M× precompute saving. Instant is bench-only.

use ironnest_optimizer::{
    NestConfig, PlacementStrategy, Scalar, SeparationEffort, nest_with_config, nest_with_prepared,
    prepare,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

#[allow(clippy::disallowed_methods)]
fn ms<F: FnMut()>(mut f: F) -> f64 {
    let t = std::time::Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    // A 13-type library (the mixed-13 consumer spread, matching bench.rs's nfp_cache_probe).
    let items: Vec<Vec<[Scalar; 2]>> = vec![
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
    let qty: Vec<usize> = vec![1; items.len()];
    let rots: Vec<Vec<Scalar>> = items
        .iter()
        .map(|_| vec![0.0, 90.0, 180.0, 270.0])
        .collect();
    let min_sep = 0.375;
    // M distinct "parent" containers (the part-in-part loop). Kept modest so everything places by
    // construction and the O(P^2) precompute — not the nest itself — is what each call pays.
    let m = 15usize;
    let containers: Vec<Vec<[Scalar; 2]>> = (0..m)
        .map(|i| rect(60.0 + i as Scalar, 40.0 + i as Scalar))
        .collect();

    // --- Per-call: rebuild everything each nest (today's part-in-part cost). ---
    let mut placed_a = 0usize;
    let per_call = ms(|| {
        for c in &containers {
            let cfg = NestConfig {
                min_sep,
                seed: 1,
                budget: 400,
                restarts: 1,
                strategy: PlacementStrategy::Nfp,
                separation_effort: SeparationEffort::Full,
                column_weight: 10,
            };
            let sol = nest_with_config(&items, &qty, c, &[], &rots, &cfg).expect("in-domain");
            placed_a += sol.placements.len();
        }
    });

    // --- Prepared: build the library once, reuse across containers. ---
    let mut placed_b = 0usize;
    let prepared_total = ms(|| {
        let prepared = prepare(&items, &rots, min_sep, PlacementStrategy::Nfp).expect("in-domain");
        for c in &containers {
            let sol =
                nest_with_prepared(&prepared, &qty, c, &[], 1, 400, 1, SeparationEffort::Full)
                    .expect("in-domain");
            placed_b += sol.placements.len();
        }
    });

    println!("--- prepared_bench: 13-type library into M={m} containers (NFP, min_sep=0.375) ---");
    println!("per-call (rebuild each): {per_call:>9.1} ms   (placed total {placed_a})");
    println!("prepared (build once):   {prepared_total:>9.1} ms   (placed total {placed_b})");
    println!(
        "speedup: {:.1}×   (placements identical: {})",
        per_call / prepared_total,
        placed_a == placed_b
    );
}
