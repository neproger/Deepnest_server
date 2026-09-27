// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Throwaway density probe: mimic the consumer image — a few big "developed-cone" annular sectors
//! plus many small square plates, OVER-SUBSCRIBED, so "how many fit" measures packing tightness
//! (utilization at fixed demand is arrangement-independent once everything fits). Compares strategy,
//! rotation granularity, and restarts. Instant/std-trig are input-generation only (bench waiver).

use ironnest_optimizer::{
    NestConfig, PlacementStrategy, Scalar, SeparationEffort, nest_with_config,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

/// A developed-cone-shell annular sector: outer radius `ro`, inner `ri`, spanning `deg` degrees,
/// opening upward, anchored near the origin. ~convex-ish curved trapezoid (the image's big parts).
#[allow(clippy::disallowed_methods)]
fn sector(ri: Scalar, ro: Scalar, deg: Scalar, steps: usize) -> Vec<[Scalar; 2]> {
    let half = deg.to_radians() / 2.0;
    let a0 = std::f64::consts::FRAC_PI_2 - half;
    let a1 = std::f64::consts::FRAC_PI_2 + half;
    let mut pts = Vec::with_capacity(2 * steps + 2);
    // outer arc a0 -> a1
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * (i as Scalar) / (steps as Scalar);
        pts.push([ro * a.cos(), ro * a.sin()]);
    }
    // inner arc a1 -> a0
    for i in 0..=steps {
        let a = a1 - (a1 - a0) * (i as Scalar) / (steps as Scalar);
        pts.push([ri * a.cos(), ri * a.sin()]);
    }
    // shift so min corner ~origin
    let (mut mnx, mut mny) = (f64::INFINITY, f64::INFINITY);
    for p in &pts {
        mnx = mnx.min(p[0]);
        mny = mny.min(p[1]);
    }
    pts.iter().map(|p| [p[0] - mnx, p[1] - mny]).collect()
}

fn area(o: &[[Scalar; 2]]) -> Scalar {
    let n = o.len();
    let mut a = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        a += o[i][0] * o[j][1] - o[j][0] * o[i][1];
    }
    (0.5 * a).abs()
}

#[allow(clippy::disallowed_methods)]
fn time_ms<F: FnMut()>(mut f: F) -> f64 {
    let t = std::time::Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let cone = sector(28.0, 60.0, 55.0, 10); // ~developed cone shell
    let sq = rect(10.0, 10.0);
    let items = vec![cone.clone(), sq.clone()];
    let areas = [area(&cone), area(&sq)];
    // Over-subscribed: request far more than fits so "placed area" measures tightness.
    let qty = [20usize, 150];
    let container = rect(200.0, 120.0);
    let cont_area = 200.0 * 120.0;
    let total_req: Scalar = areas[0] * qty[0] as Scalar + areas[1] * qty[1] as Scalar;
    println!(
        "--- density_probe: {} cones (area {:.0}) + {} squares in 200x120 (req area {:.0} = {:.0}% of sheet) ---",
        qty[0],
        areas[0],
        qty[1],
        total_req,
        total_req / cont_area * 100.0
    );

    let cardinal = vec![0.0, 90.0, 180.0, 270.0];
    // Rotation sets to try for the CONE (squares stay cardinal). Sector-matched: multiples of the
    // 55° span let sectors tessellate point-to-point.
    let fine12: Vec<Scalar> = (0..12).map(|i| i as Scalar * 30.0).collect();
    let sector_match: Vec<Scalar> = (0..8).map(|i| i as Scalar * 45.0).collect();

    for (label, strat, cone_rots, restarts) in [
        (
            "sampling cardinal K=1",
            PlacementStrategy::Sampling,
            &cardinal,
            1usize,
        ),
        (
            "sampling cardinal K=8",
            PlacementStrategy::Sampling,
            &cardinal,
            8,
        ),
        ("nfp cardinal K=1", PlacementStrategy::Nfp, &cardinal, 1),
        ("nfp cardinal K=8", PlacementStrategy::Nfp, &cardinal, 8),
        ("nfp 12-way K=8", PlacementStrategy::Nfp, &fine12, 8),
        ("nfp 45-step K=8", PlacementStrategy::Nfp, &sector_match, 8),
    ] {
        let rots = vec![cone_rots.clone(), cardinal.clone()];
        let cfg = NestConfig {
            min_sep: 0.0,
            seed: 1,
            budget: 1000,
            restarts,
            strategy: strat,
            separation_effort: SeparationEffort::Full,
            // Density-first column weight (docs/04 §4c): w=3 beats the historical 10 by ~2–3 pp
            // on irregular/sector packs; sampling mode ignores this field.
            column_weight: if matches!(strat, PlacementStrategy::Nfp) {
                ironnest_optimizer::DENSITY_COLUMN_WEIGHT
            } else {
                10
            },
        };
        let mut placed = [0usize; 2];
        let ms = time_ms(|| {
            let sol =
                nest_with_config(&items, &qty, &container, &[], &rots, &cfg).expect("in-domain");
            placed = [0, 0];
            for p in &sol.placements {
                placed[p.item] += 1;
            }
        });
        let placed_area = areas[0] * placed[0] as Scalar + areas[1] * placed[1] as Scalar;
        println!(
            "{label:<24} cones {}/{}  squares {:>2}/{}  util {:>5.1}%  {ms:>8.0} ms",
            placed[0],
            qty[0],
            placed[1],
            qty[1],
            placed_area / cont_area * 100.0
        );
    }
}
