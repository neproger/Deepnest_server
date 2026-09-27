// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Empirical probe for the multi-start **order-diversification** lever (the `start_order_policy`
//! contiguous-block split): two mixed corpora at K=8 over five base seeds, reporting per-seed and
//! mean placed-area utilization. Recorded result (2026-07-09, J=0.3, block policy): on the
//! similar-diameter corpus the split costs ~0.4 pp vs all-canonical (93.83 % vs 94.22 %); on the
//! diameter-misleading strips corpus it gains ~1.3 pp mean (95.45 % vs 94.15 %, best seeds 95.9 %
//! vs 94.8 %) — order jitter pays exactly where largest-diameter-first is a trap, at little cost
//! elsewhere. (A k-parity split scored similarly on these corpora but breaks the doubling-K floor
//! guarantee — see `start_order_policy`.) Re-run when touching the policy or `ORDER_JITTER`.
//! `cargo run -p ironnest-optimizer --release --features parallel --example orderdiv`

use ironnest_optimizer::{Scalar, nest_multistart};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

fn corpus(label: &str, items: &[Vec<[Scalar; 2]>], areas: &[Scalar], qty: &[usize]) {
    let container = rect(100.0, 100.0);
    let rotations = [0.0, 90.0, 180.0, 270.0];
    let total: usize = qty.iter().sum();

    let mut sum = 0.0;
    for base_seed in 1..=5u64 {
        let sol = nest_multistart(
            items,
            qty,
            &container,
            &[],
            0.0,
            &rotations,
            base_seed,
            2000,
            8,
        );
        let placed_area: Scalar = sol.placements.iter().map(|p| areas[p.item]).sum();
        let util = placed_area / 10_000.0 * 100.0;
        sum += util;
        println!(
            "{label:<16} seed {base_seed}: util {util:.1}%  placed {}/{total}",
            sol.placements.len()
        );
    }
    println!("{label:<16} mean util over 5 seeds: {:.2}%\n", sum / 5.0);
}

fn main() {
    // Similar-diameter mix: canonical largest-first is already near-optimal.
    corpus(
        "mixed 4-type",
        &[
            rect(13.0, 7.0),
            rect(10.0, 10.0),
            rect(7.0, 7.0),
            vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
        ],
        &[91.0, 100.0, 49.0, 50.0],
        &[45, 30, 45, 24],
    );
    // Diameter-misleading mix: long skinny strips out-diameter the big squares, so the canonical
    // order places low-area strips first — the case order diversification should rescue.
    corpus(
        "strips+squares",
        &[rect(20.0, 2.0), rect(12.0, 12.0), rect(6.0, 6.0)],
        &[40.0, 144.0, 36.0],
        &[30, 40, 80], // 1200 + 5760 + 2880 = 9840 (98.4%)
    );
}
