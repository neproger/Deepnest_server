//! Density A/B: column_weight × effort on the cone+square sector pack (consumer image family).
use ironnest_optimizer::{
    DENSITY_COLUMN_WEIGHT, NestConfig, PlacementStrategy, Scalar, SeparationEffort,
    nest_with_config,
};

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

#[allow(clippy::disallowed_methods)]
fn sector(ri: Scalar, ro: Scalar, deg: Scalar, steps: usize) -> Vec<[Scalar; 2]> {
    let half = deg.to_radians() / 2.0;
    let a0 = std::f64::consts::FRAC_PI_2 - half;
    let a1 = std::f64::consts::FRAC_PI_2 + half;
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
    let cone = sector(28.0, 60.0, 55.0, 10);
    let sq = rect(10.0, 10.0);
    let items = vec![cone.clone(), sq.clone()];
    let areas = [area(&cone), area(&sq)];
    let qty = [20usize, 150];
    let container = rect(200.0, 120.0);
    let cont_area = 200.0 * 120.0;
    let cardinal = vec![0.0, 90.0, 180.0, 270.0];
    let rots = vec![cardinal.clone(), cardinal.clone()];

    println!("--- density_ab: cone+square oversubscribed; NFP ---");
    for (label, cw, effort, restarts) in [
        ("w=10 Full K=1", 10u32, SeparationEffort::Full, 1usize),
        (
            "w=3  Full K=1",
            DENSITY_COLUMN_WEIGHT,
            SeparationEffort::Full,
            1,
        ),
        ("w=10 Fast K=1", 10, SeparationEffort::Fast, 1),
        (
            "w=3  Fast K=1",
            DENSITY_COLUMN_WEIGHT,
            SeparationEffort::Fast,
            1,
        ),
        (
            "w=3  Full K=4",
            DENSITY_COLUMN_WEIGHT,
            SeparationEffort::Full,
            4,
        ),
        (
            "w=3  Max  K=1",
            DENSITY_COLUMN_WEIGHT,
            SeparationEffort::Max,
            1,
        ),
    ] {
        let cfg = NestConfig {
            min_sep: 0.0,
            seed: 1,
            budget: 1000,
            restarts,
            strategy: PlacementStrategy::Nfp,
            separation_effort: effort,
            column_weight: cw,
        };
        let mut placed = [0usize; 2];
        let ms = time_ms(|| {
            let sol = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
            placed = [0, 0];
            for p in &sol.placements {
                placed[p.item] += 1;
            }
        });
        let util =
            (areas[0] * placed[0] as Scalar + areas[1] * placed[1] as Scalar) / cont_area * 100.0;
        println!(
            "{label:<16} cones {:>2}  sq {:>3}  util {:>5.1}%  {ms:>8.0} ms",
            placed[0], placed[1], util
        );
    }

    // Remnant (all-fit) case
    println!("--- remnant (6 cones + 24 sq in 240x120) ---");
    let qty2 = [6usize, 24];
    let container2 = rect(240.0, 120.0);
    let sheet = 240.0 * 120.0;
    for (label, cw, effort) in [
        ("w=10 Full", 10u32, SeparationEffort::Full),
        ("w=3  Full", DENSITY_COLUMN_WEIGHT, SeparationEffort::Full),
        ("w=3  Max", DENSITY_COLUMN_WEIGHT, SeparationEffort::Max),
    ] {
        let cfg = NestConfig {
            min_sep: 0.0,
            seed: 1,
            budget: 1500,
            restarts: 1,
            strategy: PlacementStrategy::Nfp,
            separation_effort: effort,
            column_weight: cw,
        };
        let sol = nest_with_config(&items, &qty2, &container2, &[], &rots, &cfg).unwrap();
        let mut x0 = f64::INFINITY;
        let mut x1 = f64::NEG_INFINITY;
        let mut y0 = f64::INFINITY;
        let mut y1 = f64::NEG_INFINITY;
        for p in &sol.placements {
            // crude used extent from placement origin (good enough for A/B ranking)
            x0 = x0.min(p.x);
            x1 = x1.max(p.x + 60.0);
            y0 = y0.min(p.y);
            y1 = y1.max(p.y + 60.0);
        }
        let used = (x1 - x0) * (y1 - y0);
        println!(
            "{label:<12} placed {:>2}  rough_used_bbox {:>8.0} ({:>4.1}% of sheet)  max_x~{:>6.1}",
            sol.placements.len(),
            used,
            used / sheet * 100.0,
            x1
        );
    }
}
