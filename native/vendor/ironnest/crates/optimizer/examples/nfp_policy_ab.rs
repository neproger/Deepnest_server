// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! N3 policy A/B probe (docs/03 §5.3): NFP-mode multistart on the over-subscribed mixed corpus,
//! K=8 over five base seeds, mean placed-area utilization. Recorded result (2026-07-10): the
//! shipped **block split wins in NFP mode too** — 94.01 % mean vs 93.49 % for all-jittered-k>0.
//! The corrected §5.3 premise holds: canonical starts diversify through the sampling-fallback and
//! separation-tail PRNG streams, so discarding them costs density exactly as it did in sampling
//! mode (docs/02 §11.2). `start_order_policy` therefore stays strategy-INDEPENDENT. Re-run when
//! touching the policy. `cargo run -p ironnest-optimizer --release --features parallel --example nfp_policy_ab`

use ironnest_optimizer::{
    NestConfig, PlacementStrategy, Scalar, SeparationEffort, nest_with_config,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

fn main() {
    let items = vec![
        rect(13.0, 7.0),
        rect(10.0, 10.0),
        rect(7.0, 7.0),
        vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
    ];
    let areas = [91.0, 100.0, 49.0, 50.0];
    let qty = [45usize, 30, 45, 24]; // 105 % over-subscribed in 100×100
    let container = rect(100.0, 100.0);
    let rots: Vec<Vec<Scalar>> = items
        .iter()
        .map(|_| vec![0.0, 90.0, 180.0, 270.0])
        .collect();

    let mut sum = 0.0;
    for seed in 1..=5u64 {
        let cfg = NestConfig {
            min_sep: 0.0,
            seed,
            budget: 2000,
            restarts: 8,
            strategy: PlacementStrategy::Nfp,
            separation_effort: SeparationEffort::Full,
            column_weight: 10,
        };
        let sol = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
        let placed_area: Scalar = sol.placements.iter().map(|p| areas[p.item]).sum();
        let util = placed_area / 10_000.0 * 100.0;
        sum += util;
        println!(
            "seed {seed}: util {util:.1}%  placed {}/144",
            sol.placements.len()
        );
    }
    println!("mean util over 5 seeds: {:.2}%", sum / 5.0);
}
