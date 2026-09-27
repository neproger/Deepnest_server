// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! End-to-end tests for the NFP placement mode (docs/03 N2): byte-determinism, the
//! Sampling-config ≡ multistart equivalence (the frozen-wrapper contract), the capabilities the
//! sampling engine lacks (contact-quality exact-fit columns; interlock discovered by construction
//! alone), and the input-domain error surface.

use ironnest_optimizer::{
    NestConfig, NestError, PlacementStrategy, Scalar, SeparationEffort, Sheet,
    nest_multi_multistart_per_item, nest_multi_with_config, nest_multistart_per_item,
    nest_with_config, nest_with_prepared, prepare,
};

const CARDINAL: [Scalar; 4] = [0.0, 90.0, 180.0, 270.0];

fn rect(w: Scalar, h: Scalar) -> Vec<[Scalar; 2]> {
    vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

fn cardinal_per_item(n: usize) -> Vec<Vec<Scalar>> {
    (0..n).map(|_| CARDINAL.to_vec()).collect()
}

fn config(strategy: PlacementStrategy, min_sep: Scalar, seed: u64, restarts: usize) -> NestConfig {
    NestConfig {
        min_sep,
        seed,
        budget: 400,
        restarts,
        strategy,
        separation_effort: SeparationEffort::Full,
        column_weight: 10,
    }
}

#[test]
fn nfp_mode_is_byte_deterministic() {
    let items = vec![rect(13.0, 7.0), rect(10.0, 10.0)];
    let qty = [4usize, 3];
    let container = rect(50.0, 50.0);
    let rots = cardinal_per_item(2);
    let cfg = config(PlacementStrategy::Nfp, 0.375, 7, 2);
    let a = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    let b = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    assert_eq!(a, b, "NFP mode must be byte-identical for the same args");
    assert!(!a.placements.is_empty());
}

#[test]
fn prepared_equals_nest_with_config_byte_for_byte() {
    // Tier 2 (docs/04): prepare + nest_with_prepared must produce byte-identical placements to
    // nest_with_config with the matching NestConfig — for BOTH strategies, and across multiple
    // containers reusing one prepared library (the part-in-part reuse contract).
    let items = vec![rect(13.0, 7.0), rect(10.0, 10.0)];
    let rots = cardinal_per_item(2);
    let containers = [rect(50.0, 50.0), rect(41.0, 39.0), rect(60.0, 30.0)];
    let qtys: [&[usize]; 3] = [&[4, 3], &[3, 2], &[5, 1]];

    for strategy in [PlacementStrategy::Nfp, PlacementStrategy::Sampling] {
        let min_sep = 0.375;
        let prepared = prepare(&items, &rots, min_sep, strategy).unwrap();
        for (container, qty) in containers.iter().zip(qtys) {
            for (seed, restarts) in [(7u64, 1usize), (3, 2)] {
                let cfg = NestConfig {
                    min_sep,
                    seed,
                    budget: 400,
                    restarts,
                    strategy,
                    separation_effort: SeparationEffort::Full,
                    column_weight: 10,
                };
                let via_config =
                    nest_with_config(&items, qty, container, &[], &rots, &cfg).unwrap();
                let via_prepared = nest_with_prepared(
                    &prepared,
                    qty,
                    container,
                    &[],
                    seed,
                    400,
                    restarts,
                    SeparationEffort::Full,
                )
                .unwrap();
                assert_eq!(
                    via_config, via_prepared,
                    "prepared reuse must be byte-identical to nest_with_config \
                     (strategy={strategy:?}, seed={seed}, restarts={restarts})"
                );
            }
        }
    }
}

#[test]
fn prepared_fast_effort_matches_config_fast() {
    // The Fast separation tier flows through the prepared path identically.
    let items = vec![rect(10.0, 10.0)];
    let rots = cardinal_per_item(1);
    let container = rect(25.0, 25.0);
    let qty = [12usize];
    let prepared = prepare(&items, &rots, 0.0, PlacementStrategy::Nfp).unwrap();
    let cfg = NestConfig {
        min_sep: 0.0,
        seed: 7,
        budget: 400,
        restarts: 1,
        strategy: PlacementStrategy::Nfp,
        separation_effort: SeparationEffort::Fast,
        column_weight: 10,
    };
    let via_config = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    let via_prepared = nest_with_prepared(
        &prepared,
        &qty,
        &container,
        &[],
        7,
        400,
        1,
        SeparationEffort::Fast,
    )
    .unwrap();
    assert_eq!(
        via_config, via_prepared,
        "Fast effort must match through prepared"
    );
}

#[test]
fn sampling_config_equals_multistart_byte_for_byte() {
    // The frozen-wrapper contract (docs/03 §7): nest_with_config(Sampling) IS
    // nest_multistart_per_item, bit for bit.
    let items = vec![rect(13.0, 7.0), rect(10.0, 10.0)];
    let qty = [6usize, 4];
    let container = rect(60.0, 60.0);
    let rots = cardinal_per_item(2);
    let cfg = config(PlacementStrategy::Sampling, 0.5, 11, 3);
    let via_config = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    let direct = nest_multistart_per_item(&items, &qty, &container, &[], 0.5, &rots, 11, 400, 3);
    assert_eq!(
        via_config, direct,
        "Sampling config must be the frozen wrapper"
    );
}

#[test]
fn nfp_contact_columns_beat_the_exact_fit_artifact() {
    // 9 squares of 10×10 in a 30.01 sheet: needs contact-quality placement (3 columns of 3).
    // The sampling engine's artifact caps this at 2 per row/column class of results.
    let items = vec![rect(10.0, 10.0)];
    let qty = [9usize];
    let container = rect(30.01, 30.01);
    let rots = cardinal_per_item(1);
    let cfg = config(PlacementStrategy::Nfp, 0.0, 1, 1);
    let sol = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    assert_eq!(
        sol.placements.len(),
        9,
        "NFP construction must place the full 3×3 contact grid (got {}/9)",
        sol.placements.len()
    );
}

#[test]
fn nfp_finds_interlock_by_construction() {
    // Two right triangles that only fit interlocked (one at 180°) in an 11×11 sheet — the case
    // the sampling engine needs the whole separation search to solve; NFP construction finds it
    // directly (and the pipeline result must place both regardless of which phase does).
    let right_tri = vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]];
    let items = vec![right_tri];
    let qty = [2usize];
    let container = rect(11.0, 11.0);
    let rots = cardinal_per_item(1);
    let cfg = config(PlacementStrategy::Nfp, 0.0, 42, 1);
    let sol = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    assert_eq!(sol.placements.len(), 2, "interlock must place 2/2");
}

#[test]
fn nfp_l_container_with_keepout_hole() {
    // Concave L-sheet + a keep-out hole: the IFP classification + zone-folding path end to end.
    let l_container = vec![
        [0.0, 0.0],
        [60.0, 0.0],
        [60.0, 30.0],
        [30.0, 30.0],
        [30.0, 60.0],
        [0.0, 60.0],
    ];
    let hole = vec![[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]];
    let items = vec![rect(13.0, 7.0)];
    let qty = [8usize];
    let rots = cardinal_per_item(1);
    let cfg = config(PlacementStrategy::Nfp, 0.0, 3, 1);
    let sol = nest_with_config(&items, &qty, &l_container, &[hole], &rots, &cfg).unwrap();
    assert!(
        sol.placements.len() >= 6,
        "L-sheet with keep-out must still pack most bricks (got {}/8)",
        sol.placements.len()
    );
    // And byte-determinism on this concave path too.
    let again = nest_with_config(
        &items,
        &qty,
        &l_container,
        &[vec![[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]]],
        &rots,
        &cfg,
    )
    .unwrap();
    assert_eq!(sol, again);
}

#[test]
fn nfp_domain_error_surfaces() {
    let big = 3.0e12;
    let items = vec![rect(big, big)];
    let qty = [1usize];
    let container = rect(big * 2.0, big * 2.0);
    let rots = cardinal_per_item(1);
    let cfg = config(PlacementStrategy::Nfp, 0.0, 1, 1);
    match nest_with_config(&items, &qty, &container, &[], &rots, &cfg) {
        Err(NestError::NfpInputDomain { .. }) => {}
        other => panic!("expected NfpInputDomain, got {other:?}"),
    }
}

/// Max densify-once multi-start (docs/04 §4e): K>1 explores at Full, densifies only the NestScore
/// winner. Must be deterministic and place at least as many parts as a single Max start (demand
/// first). Remnant may differ from densify-every-start — that is the deliberate speed policy.
#[test]
fn nfp_max_densify_once_multistart_is_deterministic_and_admits() {
    let items = vec![rect(18.0, 8.0), rect(8.0, 8.0)];
    let qty = [4usize, 8];
    let container = rect(90.0, 20.0);
    let rots = cardinal_per_item(2);
    let cfg = NestConfig {
        min_sep: 0.0,
        seed: 5,
        budget: 400,
        restarts: 4,
        strategy: PlacementStrategy::Nfp,
        separation_effort: SeparationEffort::Max,
        column_weight: 3,
    };
    let a = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    let b = nest_with_config(&items, &qty, &container, &[], &rots, &cfg).unwrap();
    assert_eq!(a, b, "Max densify-once multi-start must be byte-identical");
    let k1 = nest_with_config(
        &items,
        &qty,
        &container,
        &[],
        &rots,
        &NestConfig {
            restarts: 1,
            ..cfg.clone()
        },
    )
    .unwrap();
    assert!(
        a.placements.len() >= k1.placements.len(),
        "Max densify-once K=4 must not place fewer than Max K=1 ({} vs {})",
        a.placements.len(),
        k1.placements.len()
    );
    assert_eq!(
        a.unplaced.len() + a.placements.len(),
        qty.iter().sum::<usize>()
    );
}

#[test]
fn sampling_multi_config_equals_multi_multistart_byte_for_byte() {
    // The multi-sheet frozen-wrapper contract: nest_multi_with_config(Sampling) IS
    // nest_multi_multistart_per_item, bit for bit.
    let items = vec![rect(13.0, 7.0), rect(10.0, 10.0)];
    let qty = [10usize, 6];
    let sheets = vec![
        Sheet {
            outline: rect(35.0, 35.0),
            holes: vec![],
        },
        Sheet {
            outline: rect(30.0, 30.0),
            holes: vec![],
        },
    ];
    let rots = cardinal_per_item(2);
    let cfg = config(PlacementStrategy::Sampling, 0.25, 9, 2);
    let via_config = nest_multi_with_config(&items, &qty, &sheets, &rots, &cfg).unwrap();
    let direct = nest_multi_multistart_per_item(&items, &qty, &sheets, 0.25, &rots, 9, 400, 2);
    assert_eq!(
        via_config, direct,
        "Sampling multi config must be the frozen wrapper"
    );
}

#[test]
fn nfp_multi_sheet_spills_and_is_deterministic() {
    // Overflow demand on sheet 1 spills to sheet 2; the container-independent PartsGeometry is
    // built once and shared across sheets (behavioral proof: byte-determinism + full placement).
    let items = vec![rect(13.0, 7.0), rect(10.0, 10.0)];
    let qty = [10usize, 6]; // 910 + 600 = 1510 > 35² = 1225 → guaranteed spill
    let sheets = vec![
        Sheet {
            outline: rect(35.0, 35.0),
            holes: vec![],
        },
        Sheet {
            outline: rect(40.0, 40.0),
            holes: vec![],
        },
    ];
    let rots = cardinal_per_item(2);
    let cfg = config(PlacementStrategy::Nfp, 0.0, 5, 2);
    let a = nest_multi_with_config(&items, &qty, &sheets, &rots, &cfg).unwrap();
    let b = nest_multi_with_config(&items, &qty, &sheets, &rots, &cfg).unwrap();
    assert_eq!(
        a, b,
        "NFP multi-sheet must be byte-identical for the same args"
    );
    assert!(
        !a.per_sheet[0].is_empty() && !a.per_sheet[1].is_empty(),
        "demand must spill to the second sheet"
    );
    assert!(
        a.unplaced.is_empty(),
        "everything fits across the two sheets"
    );
}
