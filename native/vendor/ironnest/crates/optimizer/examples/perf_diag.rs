// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Throwaway perf-diagnostic: reproduce the consumer "mixed-13" NFP pack at the consumer's
//! min_sep=0.375, restarts=1, and time the FULL pipeline so the construction-vs-separation-tail
//! split is visible. NOT a determinism path (Instant is benchmark-only).
//! `cargo run -p ironnest-optimizer --release --example perf_diag`

use ironnest_optimizer::{
    NestConfig, PlacementStrategy, Scalar, SeparationEffort, nest_with_config,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

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

#[allow(clippy::disallowed_methods)]
fn time_ms<F: FnMut()>(mut f: F) -> f64 {
    let t = std::time::Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    // The consumer acceptance corpus (verbatim from nfp_spike.rs): 13 heterogeneous plate shapes,
    // qty = base x 2 = 122 instances, 120x60 sheet, min_sep = 0.375-in part gap.
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
    let container = rect(120.0, 60.0);
    let rots: Vec<Vec<Scalar>> = mixed_items
        .iter()
        .map(|_| vec![0.0, 90.0, 180.0, 270.0])
        .collect();
    let total: usize = mixed_qty.iter().sum();

    println!("--- perf_diag: consumer mixed-13, min_sep=0.375, restarts=1 ---");
    for &(mode, strat) in &[
        ("NFP", PlacementStrategy::Nfp),
        ("sampling", PlacementStrategy::Sampling),
    ] {
        for &(estr, effort) in &[
            ("Full", SeparationEffort::Full),
            ("Fast", SeparationEffort::Fast),
        ] {
            let cfg = NestConfig {
                min_sep: 0.375,
                seed: 1,
                budget: 400,
                restarts: 1,
                strategy: strat,
                separation_effort: effort,
                column_weight: 10,
            };
            let mut placed = 0usize;
            let ms = time_ms(|| {
                let sol = nest_with_config(&mixed_items, &mixed_qty, &container, &[], &rots, &cfg)
                    .expect("in-domain");
                placed = sol.placements.len();
            });
            println!("{mode:<9} sep={estr:<4} placed {placed:>4}/{total}  {ms:>10.1} ms");
        }
    }
    // Construction-only reference is nfp_spike.rs's 908 ms (lazy cache) for the same shapes.
    println!("(nfp_spike.rs construction-only for the same shapes: ~908 ms placing 51/122)");
}
