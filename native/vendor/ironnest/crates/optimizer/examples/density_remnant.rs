// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Throwaway remnant probe (docs/04 Tier D / §4e): the goal is to leave the largest usable REMNANT,
//! i.e. compact the parts into the smallest region. A FIXED part set that all fits, nested Full vs
//! Max at K=1 and K=8 (production path), reporting the used bounding box (smaller = bigger remnant)
//! and writing SVGs to /tmp. Max at K>1 uses densify-once (explore Full, densify NestScore winner
//! only) so K=8 wall ≈ K=1 densify + cheap multi-start. Instant/std-trig are output/generation only
//! (bench waiver).

use std::fmt::Write as _;

use ironnest_optimizer::{
    NestConfig, PlacementStrategy, Scalar, SeparationEffort, nest_with_config,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

#[allow(clippy::disallowed_methods)]
fn sector(ri: Scalar, ro: Scalar, deg: Scalar, steps: usize) -> Vec<[Scalar; 2]> {
    let half = deg.to_radians() / 2.0;
    let (a0, a1) = (
        std::f64::consts::FRAC_PI_2 - half,
        std::f64::consts::FRAC_PI_2 + half,
    );
    let mut pts = Vec::new();
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * (i as Scalar) / (steps as Scalar);
        pts.push([ro * a.cos(), ro * a.sin()]);
    }
    for i in 0..=steps {
        let a = a1 - (a1 - a0) * (i as Scalar) / (steps as Scalar);
        pts.push([ri * a.cos(), ri * a.sin()]);
    }
    let (mut mnx, mut mny) = (f64::INFINITY, f64::INFINITY);
    for p in &pts {
        mnx = mnx.min(p[0]);
        mny = mny.min(p[1]);
    }
    pts.iter().map(|p| [p[0] - mnx, p[1] - mny]).collect()
}

/// Transform an item's original outline to its placed pose (placed = Rot(deg)·orig + (x,y)).
#[allow(clippy::disallowed_methods)]
fn placed_outline(outline: &[[Scalar; 2]], x: Scalar, y: Scalar, deg: Scalar) -> Vec<[Scalar; 2]> {
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    outline
        .iter()
        .map(|p| [x + c * p[0] - s * p[1], y + s * p[0] + c * p[1]])
        .collect()
}

#[allow(clippy::disallowed_methods)]
fn time_ms<F: FnMut()>(mut f: F) -> f64 {
    let t = std::time::Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    // A FIXED set that all fits comfortably (so the metric is compaction, not admission).
    let cone = sector(28.0, 60.0, 55.0, 10);
    let sq = rect(10.0, 10.0);
    let items = vec![cone.clone(), sq.clone()];
    let qty = [6usize, 24];
    let (sheet_w, sheet_h) = (240.0, 120.0);
    let container = rect(sheet_w, sheet_h);
    let rots: Vec<Vec<Scalar>> = vec![
        (0..8).map(|i| i as Scalar * 45.0).collect(),
        vec![0.0, 90.0, 180.0, 270.0],
    ];

    println!(
        "--- density_remnant: {} cones + {} squares in {sheet_w}x{sheet_h} (all fit) ---",
        qty[0], qty[1]
    );

    for &(label, effort, restarts, col_w) in &[
        ("Full w10 K=1", SeparationEffort::Full, 1usize, 10u32),
        ("Full w3  K=1", SeparationEffort::Full, 1, 3),
        ("Full w3  K=8", SeparationEffort::Full, 8, 3),
        ("Max  w3  K=1", SeparationEffort::Max, 1, 3),
        ("Max  w3  K=8", SeparationEffort::Max, 8, 3),
    ] {
        let cfg = NestConfig {
            min_sep: 0.0,
            seed: 1,
            budget: 1500,
            restarts,
            strategy: PlacementStrategy::Nfp,
            separation_effort: effort,
            column_weight: col_w,
        };
        let mut placed = 0usize;
        let mut used_x = (f64::INFINITY, f64::NEG_INFINITY);
        let mut used_y = (f64::INFINITY, f64::NEG_INFINITY);
        let mut svg = String::new();
        writeln!(
            svg,
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='-5 -5 {} {}'>",
            sheet_w + 10.0,
            sheet_h + 10.0
        )
        .unwrap();
        writeln!(
            svg,
            "<rect x='0' y='0' width='{sheet_w}' height='{sheet_h}' fill='#eef' stroke='#333'/>"
        )
        .unwrap();
        let ms = time_ms(|| {
            let sol =
                nest_with_config(&items, &qty, &container, &[], &rots, &cfg).expect("in-domain");
            placed = sol.placements.len();
            used_x = (f64::INFINITY, f64::NEG_INFINITY);
            used_y = (f64::INFINITY, f64::NEG_INFINITY);
            svg.truncate(svg.find("</rect>").map_or(svg.len(), |_| svg.len()));
            for p in &sol.placements {
                let poly = placed_outline(&items[p.item], p.x, p.y, p.rotation_deg);
                let pts: String = poly
                    .iter()
                    .map(|q| format!("{:.2},{:.2} ", q[0], q[1]))
                    .collect();
                let fill = if p.item == 0 { "#f8c" } else { "#8cf" };
                let _ = writeln!(
                    svg,
                    "<polygon points='{pts}' fill='{fill}' stroke='#333' stroke-width='0.3'/>"
                );
                for q in &poly {
                    used_x = (used_x.0.min(q[0]), used_x.1.max(q[0]));
                    used_y = (used_y.0.min(q[1]), used_y.1.max(q[1]));
                }
            }
        });
        writeln!(svg, "</svg>").unwrap();
        let used_w = used_x.1 - used_x.0;
        let used_h = used_y.1 - used_y.0;
        let used_area = used_w * used_h;
        let sheet = sheet_w * sheet_h;
        let slug = label
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>();
        let path = format!("/tmp/nest_{slug}.svg");
        std::fs::write(&path, &svg).unwrap();
        println!(
            "{label:<14} placed {placed:>3}  used {used_w:>6.1}x{used_h:<6.1} = {used_area:>8.0} ({:>4.1}% of sheet)  \
             remnant {:>5.1}%  max_x {:>6.1}  {ms:>7.0} ms  -> {path}",
            used_area / sheet * 100.0,
            (sheet - used_area) / sheet * 100.0,
            used_x.1,
        );
    }
    println!(
        "(open /tmp/nest_*.svg to compare remnant / max_x — smaller max_x = more end remnant)"
    );
}
