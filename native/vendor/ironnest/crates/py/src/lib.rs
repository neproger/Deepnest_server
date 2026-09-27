// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! PyO3 binding — the `ironnest` abi3-py313 wheel (`import ironnest`).
//!
//! Two `#[pyfunction]`s — [`nest`] (one container) and [`nest_multi`] (spill across sheets) —
//! wrapping [`ironnest_core::nest`] / [`ironnest_core::nest_multi`]. Polygons marshal as plain
//! `list[list[tuple[float, float]]]` (PyO3 `Vec<Vec<[f64; 2]>>`) — **no numpy, no JSON wire** (the
//! JSON wire is exactly the float-drift class that killed the old CLI stub). Python `float` *is* IEEE
//! f64, so the marshalling is exact and introduces no nondeterminism: the wheel inherits the engine's
//! byte-identical, cross-platform-reproducible output (proven by the Phase-3 golden).

use ironnest_core::{
    NestConfig, PlacementStrategy, PreparedParts, SeparationEffort, Sheet,
    nest_multi_multistart_per_item as nest_multi_multistart_impl,
    nest_multi_per_item as nest_multi_impl, nest_multi_with_config,
    nest_multistart_per_item as nest_multistart_impl, nest_per_item as nest_impl, nest_with_config,
    nest_with_prepared_weighted, prepare as core_prepare,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyString;

/// One placed instance returned to Python: `(item, x, y, rotation_deg)`.
type PyPlacement = (usize, f64, f64, f64);

/// One sheet for [`nest_multi`]: `(outline, holes)`.
type PySheet = (Vec<[f64; 2]>, Vec<Vec<[f64; 2]>>);

/// Parses the Python `rotations` argument into a per-item list of length `n_items` (the form the
/// engine's per-item entry consumes). Accepts EITHER:
/// * a flat `list[float]` — one orientation set applied to **every** item (broadcast); or
/// * a `list[list[float]]` — one set per item type (`len == n_items`, each inner list non-empty).
///
/// The two forms are told apart by the **type** of the first element (a number ⇒ uniform; a nested
/// sequence ⇒ per-item), never by length — so a single-item nest is unambiguous (`[0.0]` is uniform,
/// `[[0.0]]` is per-item). An empty OUTER list is the historical uniform "no rotation" default.
///
/// Raises `ValueError` (never silently coerces) on: a per-item length mismatch with `items`, an
/// empty inner set (a part must allow at least one orientation — pass `[0.0]` for "no rotation"), or
/// any non-numeric / non-finite (NaN, ±inf) angle.
fn parse_rotations(rotations: &Bound<'_, PyAny>, n_items: usize) -> PyResult<Vec<Vec<f64>>> {
    // A bare `str` is iterable (it yields characters) but is never a valid rotations value — reject
    // it up front with a clear message instead of letting it fail deep in per-character extraction.
    if rotations.is_instance_of::<PyString>() {
        return Err(PyValueError::new_err(
            "rotations must be a list[float] or a list[list[float]], not a string",
        ));
    }

    let outer: Vec<Bound<'_, PyAny>> = rotations
        .try_iter()
        .map_err(|_| {
            PyValueError::new_err("rotations must be a list[float] or a list[list[float]]")
        })?
        .collect::<PyResult<_>>()?;

    // Empty outer ⇒ uniform "no rotation" for every item (the historical meaning of `rotations=[]`).
    if outer.is_empty() {
        return Ok(vec![Vec::new(); n_items]);
    }

    if outer[0].extract::<f64>().is_ok() {
        // Uniform: a flat list[float] applied to every item.
        let angles = extract_angles(&outer, "rotations")?;
        Ok(vec![angles; n_items])
    } else {
        // Per-item: one list[float] per item type.
        if outer.len() != n_items {
            return Err(PyValueError::new_err(format!(
                "per-item rotations has {} entr{} but there {} {} item type(s); pass one rotation \
                 list per item, or a single list[float] applied to all",
                outer.len(),
                if outer.len() == 1 { "y" } else { "ies" },
                if n_items == 1 { "is" } else { "are" },
                n_items,
            )));
        }
        let mut per_item = Vec::with_capacity(n_items);
        for (k, inner) in outer.iter().enumerate() {
            let inner_elems: Vec<Bound<'_, PyAny>> = inner
                .try_iter()
                .map_err(|_| {
                    PyValueError::new_err(format!(
                        "rotations[{k}] must be a list[float] (a per-item orientation set)"
                    ))
                })?
                .collect::<PyResult<_>>()?;
            if inner_elems.is_empty() {
                return Err(PyValueError::new_err(format!(
                    "rotations[{k}] is empty; a part must allow at least one orientation \
                     (use [0.0] for no rotation)"
                )));
            }
            per_item.push(extract_angles(&inner_elems, &format!("rotations[{k}]"))?);
        }
        Ok(per_item)
    }
}

/// Extracts a finite-`f64` angle list from `elems`, raising `ValueError` on any non-numeric or
/// non-finite (NaN/±inf) entry. `ctx` names the list in error messages (e.g. `"rotations[2]"`).
fn extract_angles(elems: &[Bound<'_, PyAny>], ctx: &str) -> PyResult<Vec<f64>> {
    let mut out = Vec::with_capacity(elems.len());
    for (i, e) in elems.iter().enumerate() {
        let angle: f64 = e
            .extract()
            .map_err(|_| PyValueError::new_err(format!("{ctx}[{i}] is not a real number")))?;
        if !angle.is_finite() {
            return Err(PyValueError::new_err(format!(
                "{ctx}[{i}] = {angle} is not a finite angle (NaN and ±inf are not allowed)"
            )));
        }
        out.push(angle);
    }
    Ok(out)
}

/// Parses the `strategy` kwarg: `"sampling"` (the default engine) or `"nfp"` (the exact-NFP
/// constructive mode, feature #4 — docs/03). Raises `ValueError` on anything else — never a
/// silent fallback (a placement-changing knob must fail loudly).
fn parse_strategy(strategy: &str) -> PyResult<PlacementStrategy> {
    match strategy {
        "sampling" => Ok(PlacementStrategy::Sampling),
        "nfp" => Ok(PlacementStrategy::Nfp),
        other => Err(PyValueError::new_err(format!(
            "strategy must be \"sampling\" or \"nfp\", got \"{other}\""
        ))),
    }
}

/// Parses the `separation_effort` kwarg: `"full"` (the shipped, byte-identical separation tail — the
/// default) or `"fast"` (prune provably-hopeless separation attempts + run a reduced per-insert budget;
/// far faster on over-subscribed packs at the cost of a **deliberately different, still-feasible**
/// layout — the engine re-validates nothing, but the consumer does). Raises `ValueError` otherwise —
/// never a silent fallback (a placement-changing knob must fail loudly).
fn parse_separation_effort(effort: &str) -> PyResult<SeparationEffort> {
    match effort {
        "full" => Ok(SeparationEffort::Full),
        "fast" => Ok(SeparationEffort::Fast),
        "max" => Ok(SeparationEffort::Max),
        other => Err(PyValueError::new_err(format!(
            "separation_effort must be \"full\", \"fast\", or \"max\", got \"{other}\""
        ))),
    }
}

/// Nest `items` (one outline per part type, each `[[x, y], …]` in item-local coordinates) into a
/// single irregular `container` outline.
///
/// Parameters
/// ----------
/// items : list[list[tuple[float, float]]]
///     One polygon outline per item *type*, in item-local coordinates.
/// qty : list[int]
///     Demand per item type; ``len(qty) == len(items)``.
/// container : list[tuple[float, float]]
///     The container boundary outline.
/// holes : list[list[tuple[float, float]]]
///     Keep-out polygons inside the container that no part may overlap — interior voids, sheet
///     defects, or (to "nest inside a part") the solid region of an already-placed part. ``[]`` for
///     none.
/// min_sep : float
///     Minimum separation enforced part↔part, part↔boundary, and part↔hole; ``0.0`` disables it.
///     Any value is fully cross-platform byte-identical: the separation offset is computed by the
///     vendored straight-skeleton offsetter with all trig routed through ``libm`` (a nonzero-
///     ``min_sep`` case is in the cross-platform determinism golden). A production part gap is the
///     intended use — there is no reproducibility reason to keep it ``0.0``.
/// rotations : list[float] | list[list[float]]
///     Allowed discrete orientations **in degrees**, in either of two forms:
///
///     * ``list[float]`` — one set applied to *every* item (e.g. ``[0, 90, 180, 270]``); ``[]`` ⇒
///       no rotation. This is the original, uniform form.
///     * ``list[list[float]]`` — one set *per item type*, so ``len(rotations) == len(items)`` and
///       ``rotations[k]`` is the allowed-orientation set for ``items[k]``. Lets different shapes use
///       different orientations in one nest (e.g. rectangles pinned to ``[0, 90]``, right triangles
///       free to interlock on ``[0, 45, 90, 135, 180, 225, 270, 315]``). Each inner list must be
///       non-empty — use ``[0.0]`` for "do not rotate this part".
///
///     The form is decided by the *type* of the first element (a number ⇒ uniform; a list ⇒
///     per-item), never by length, so a single-item nest is unambiguous. A returned placement's
///     rotation for item ``k`` is always a member of that item's set.
///
///     Every listed angle is applied cross-platform-deterministically — the rotation trig routes
///     through ``libm`` (byte-identical on every target), so a non-cardinal angle like ``45`` is
///     just as reproducible. The cardinal set ``{0, 90, 180, 270}`` is nonetheless recommended where
///     it suffices: those orientations also yield coordinate-exact placements (integer rotation
///     matrix, no sub-ULP trig fuzz), which is the cleanest input for a cut-realizable audit trail.
///
///     Raises ``ValueError`` on a per-item length mismatch, an empty inner set, or any non-numeric
///     or non-finite (NaN/±inf) angle.
/// seed : int
///     Explicit PRNG seed. There is **no** entropy fallback — determinism is the contract.
/// budget : int
///     Samples per item placement (a fixed budget; never a wall clock).
/// restarts : int, optional
///     Number of independent constructions to run from decorrelated seeds, keeping the **densest**
///     result — a deterministic best-of-K "multi-start". Defaults to ``1``, which is byte-identical to
///     the single-start nest. Higher values trade run time (≈ linear in ``restarts``) for packing
///     density on heterogeneous parts, where a single greedy construction lands in a seed-dependent
///     basin. Still fully cross-platform byte-identical for the same arguments.
/// separation_effort : str, optional
///     How hard the post-construction separation search tries to place the leftovers. On an
///     **over-subscribed** job (more parts requested than fit) this tail dominates the run time.
///
///     * ``"full"`` (default) — attempt every leftover at the full separator budget. **Byte-identical
///       to every prior release.**
///     * ``"fast"`` — skip provably-hopeless attempts (area conservation) and run a reduced per-insert
///       budget. Much faster on over-subscribed jobs, at the cost of a **deliberately different**
///       (but still fully feasible) layout — it is a distinct, still cross-platform byte-identical
///       result, not the ``"full"`` one. Recommended when the caller re-validates every layout and
///       cares about wall-clock. Raises ``ValueError`` on any other value.
///     * ``"max"`` — ``"full"`` plus a densification pass that shrinks the used region (closes gaps
///       between already-placed parts). Highest density, highest cost.
/// column_weight : int, optional
///     LBF horizontal weight for NFP candidate ranking (``cost = column_weight * x_max + y_max``).
///     Default **10** is byte-identical to every prior release. **3** packs irregular / sector parts
///     tighter (measured +2–3 pp util on cone+square packs) by not over-forcing strict left-columns.
///     Sampling mode currently ignores this.
///
/// Returns
/// -------
/// tuple[list[tuple[int, float, float, float]], list[int]]
///     ``(placements, unplaced)`` where each placement is ``(item, x, y, rotation_deg)`` mapping the
///     item's original outline as ``placed = Rot(rotation_deg)·original + (x, y)``, and ``unplaced``
///     lists the item-type index of every instance that did not fit.
///
/// The same arguments always produce a byte-identical result.
#[pyfunction]
#[pyo3(signature = (items, qty, container, holes, min_sep, rotations, seed, budget, restarts=1, strategy="sampling", separation_effort="full", column_weight=10))]
#[allow(clippy::needless_pass_by_value)] // PyO3 marshals owned Vecs out of the Python objects
#[allow(clippy::too_many_arguments)] // injected `py` GIL token + the oracle's input surface (mirrors the Rust API)
fn nest(
    py: Python<'_>,
    items: Vec<Vec<[f64; 2]>>,
    qty: Vec<usize>,
    container: Vec<[f64; 2]>,
    holes: Vec<Vec<[f64; 2]>>,
    min_sep: f64,
    rotations: Bound<'_, PyAny>,
    seed: u64,
    budget: u64,
    restarts: usize,
    strategy: &str,
    separation_effort: &str,
    column_weight: u32,
) -> PyResult<(Vec<PyPlacement>, Vec<usize>)> {
    if items.len() != qty.len() {
        return Err(PyValueError::new_err(format!(
            "items ({}) and qty ({}) must have the same length",
            items.len(),
            qty.len()
        )));
    }

    // Resolve the uniform-or-per-item `rotations` into one set per item BEFORE releasing the GIL —
    // it touches Python objects, so it must run while the GIL is held. The result is a plain, owned
    // `Vec<Vec<f64>>` the `Ungil` solve closure can take by reference.
    let rotations = parse_rotations(&rotations, items.len())?;
    let strategy = parse_strategy(strategy)?;
    let effort = parse_separation_effort(separation_effort)?;

    // Release the GIL for the (synchronous, potentially long) solve so the caller's other Python
    // threads keep running — without this a worker-thread nest freezes the whole interpreter: the
    // asyncio event loop stalls ("connection lost") and SIGINT/Ctrl+C handling dies. Every input is
    // an owned, `Send` Rust `Vec` and the returned `Solution` is plain data, so the closure is
    // `Ungil` and this is the textbook PyO3 release pattern. It does NOT touch the determinism
    // contract: no threads run *inside* the solve (single canonical worker, CLAUDE.md), so output
    // stays byte-identical.
    let solution = py
        .detach(|| {
            // With the DEFAULT separation effort (`Full`), Sampling keeps the historical branches
            // byte-for-byte and NFP routes through the config entry point (docs/03) — every existing
            // golden holds. A non-default `separation_effort` is placement-changing, so it routes
            // through the config path for BOTH strategies (nest_with_config handles Sampling too).
            // Default path (sampling + full effort + column_weight=10) keeps the historical
            // frozen branches so existing goldens / callers stay byte-identical. Any non-default
            // column_weight or effort routes through nest_with_config.
            match strategy {
                PlacementStrategy::Sampling
                    if effort == SeparationEffort::Full && restarts <= 1 && column_weight == 10 =>
                {
                    Ok(nest_impl(
                        &items, &qty, &container, &holes, min_sep, &rotations, seed, budget,
                    ))
                }
                PlacementStrategy::Sampling
                    if effort == SeparationEffort::Full && column_weight == 10 =>
                {
                    Ok(nest_multistart_impl(
                        &items, &qty, &container, &holes, min_sep, &rotations, seed, budget,
                        restarts,
                    ))
                }
                _ => nest_with_config(
                    &items,
                    &qty,
                    &container,
                    &holes,
                    &rotations,
                    &NestConfig {
                        min_sep,
                        seed,
                        budget,
                        restarts,
                        strategy,
                        separation_effort: effort,
                        column_weight,
                    },
                ),
            }
        })
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

    let placements = solution
        .placements
        .iter()
        .map(|p| (p.item, p.x, p.y, p.rotation_deg))
        .collect();

    Ok((placements, solution.unplaced))
}

/// Nest `items` (demand `qty`) across several `sheets` in order, spilling leftover demand to the next.
///
/// Parameters
/// ----------
/// items, qty, min_sep, rotations, seed, budget
///     As in [`nest`]. In particular ``rotations`` accepts both the uniform ``list[float]`` and the
///     per-item ``list[list[float]]`` form (with ``len(rotations) == len(items)``), validated
///     identically.
/// sheets : list[tuple[list[tuple[float, float]], list[list[tuple[float, float]]]]]
///     Each sheet is ``(outline, holes)`` — a boundary outline plus its keep-out holes.
/// restarts : int, optional
///     Best-of-K multi-start applied **per sheet** (default ``1`` = byte-identical to the historical
///     behaviour). Each sheet is packed with the densest of ``restarts`` decorrelated-seed runs before
///     leftover demand spills to the next, so fewer sheets / less material are used on a mixed-parts
///     job. Same determinism contract; the K runs of each sheet execute in parallel (GIL released).
///
/// Returns
/// -------
/// tuple[list[list[tuple[int, float, float, float]]], list[int]]
///     ``(per_sheet, unplaced)`` where ``per_sheet[i]`` are the placements on ``sheets[i]`` and
///     ``unplaced`` lists the item-type index of every instance that fit on no sheet.
///
/// Each sheet is nested with a seed derived deterministically from `seed`, so the result is
/// byte-identical for the same arguments.
#[pyfunction]
#[pyo3(signature = (items, qty, sheets, min_sep, rotations, seed, budget, restarts=1, strategy="sampling", separation_effort="full", column_weight=10))]
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)] // `py` is the injected GIL token (not an oracle input); the rest are the oracle's input surface
fn nest_multi(
    py: Python<'_>,
    items: Vec<Vec<[f64; 2]>>,
    qty: Vec<usize>,
    sheets: Vec<PySheet>,
    min_sep: f64,
    rotations: Bound<'_, PyAny>,
    seed: u64,
    budget: u64,
    restarts: usize,
    strategy: &str,
    separation_effort: &str,
    column_weight: u32,
) -> PyResult<(Vec<Vec<PyPlacement>>, Vec<usize>)> {
    if items.len() != qty.len() {
        return Err(PyValueError::new_err(format!(
            "items ({}) and qty ({}) must have the same length",
            items.len(),
            qty.len()
        )));
    }

    // Resolve `rotations` (uniform or per-item) into one set per item while the GIL is held — see the
    // note in `nest`.
    let rotations = parse_rotations(&rotations, items.len())?;
    let strategy = parse_strategy(strategy)?;
    let effort = parse_separation_effort(separation_effort)?;

    let sheets: Vec<Sheet> = sheets
        .into_iter()
        .map(|(outline, holes)| Sheet { outline, holes })
        .collect();

    // Release the GIL for the solve — see the note in `nest` for the why and the safety argument.
    // `restarts <= 1` is the historical single-start spill; `> 1` packs each sheet best-of-K. A
    // non-default `separation_effort` / `column_weight` is placement-changing and routes through the
    // config path (the defaults keep the historical branches byte-for-byte).
    let solution = py
        .detach(|| match strategy {
            PlacementStrategy::Sampling
                if effort == SeparationEffort::Full && restarts <= 1 && column_weight == 10 =>
            {
                Ok(nest_multi_impl(
                    &items, &qty, &sheets, min_sep, &rotations, seed, budget,
                ))
            }
            PlacementStrategy::Sampling
                if effort == SeparationEffort::Full && column_weight == 10 =>
            {
                Ok(nest_multi_multistart_impl(
                    &items, &qty, &sheets, min_sep, &rotations, seed, budget, restarts,
                ))
            }
            _ => nest_multi_with_config(
                &items,
                &qty,
                &sheets,
                &rotations,
                &NestConfig {
                    min_sep,
                    seed,
                    budget,
                    restarts,
                    strategy,
                    separation_effort: effort,
                    column_weight,
                },
            ),
        })
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

    let per_sheet = solution
        .per_sheet
        .iter()
        .map(|sheet| {
            sheet
                .iter()
                .map(|p| (p.item, p.x, p.y, p.rotation_deg))
                .collect()
        })
        .collect();

    Ok((per_sheet, solution.unplaced))
}

/// A pre-imported, container-independent part library reusable across many [`PreparedParts::nest`]
/// calls (the "part-in-part" workload — one part set nested into many parent shapes). Built once by
/// [`prepare`], it holds the imported items and, for ``strategy="nfp"``, the O((P·R)²) pairwise NFP
/// table — so nesting the same parts into ``M`` containers pays that precompute **once** instead of
/// ``M`` times (docs/04 Tier 2). ``prepared.nest(...)`` is byte-identical to the equivalent
/// ``ironnest.nest(...)`` call.
///
/// Immutable (``frozen``): safe to hold and reuse across calls and threads.
#[pyclass(frozen, name = "PreparedParts")]
struct PyPreparedParts {
    inner: PreparedParts,
    /// The prepared item-type count, for validating each ``nest`` call's ``qty``.
    n_items: usize,
    min_sep: f64,
}

#[pymethods]
impl PyPreparedParts {
    /// Nest this prepared part library into one ``container`` (with ``holes``). ``qty``, the
    /// container, ``seed``, ``budget``, ``restarts`` and ``separation_effort`` are per call — the
    /// heavy container-independent precompute is reused. ``min_sep`` and ``strategy`` were fixed at
    /// [`prepare`] time. Returns ``(placements, unplaced)`` exactly as [`nest`].
    #[pyo3(signature = (qty, container, holes, seed, budget, restarts=1, separation_effort="full", column_weight=10))]
    #[allow(clippy::needless_pass_by_value)]
    #[allow(clippy::too_many_arguments)]
    fn nest(
        &self,
        py: Python<'_>,
        qty: Vec<usize>,
        container: Vec<[f64; 2]>,
        holes: Vec<Vec<[f64; 2]>>,
        seed: u64,
        budget: u64,
        restarts: usize,
        separation_effort: &str,
        column_weight: u32,
    ) -> PyResult<(Vec<PyPlacement>, Vec<usize>)> {
        if qty.len() != self.n_items {
            return Err(PyValueError::new_err(format!(
                "qty ({}) must match the prepared item count ({})",
                qty.len(),
                self.n_items
            )));
        }
        let effort = parse_separation_effort(separation_effort)?;
        // Release the GIL for the solve — same rationale/safety as `nest` (owned Send inputs, plain
        // data out, no threads inside the solve).
        let solution = py
            .detach(|| {
                nest_with_prepared_weighted(
                    &self.inner,
                    &qty,
                    &container,
                    &holes,
                    seed,
                    budget,
                    restarts,
                    effort,
                    column_weight,
                )
            })
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let placements = solution
            .placements
            .iter()
            .map(|p| (p.item, p.x, p.y, p.rotation_deg))
            .collect();
        Ok((placements, solution.unplaced))
    }

    /// The separation offset the parts were imported with (containers passed to ``nest`` reuse it).
    #[getter]
    fn min_sep(&self) -> f64 {
        self.min_sep
    }
}

/// Pre-import a part library for reuse across many containers (docs/04 Tier 2).
///
/// Parameters
/// ----------
/// items : list[list[tuple[float, float]]]
///     One polygon outline per item *type*, as in :func:`nest`.
/// rotations : list[float] | list[list[float]]
///     Allowed orientations, uniform or per-item — parsed exactly as :func:`nest`.
/// min_sep : float
///     Minimum separation, fixed for every ``nest`` call off this handle (a container nested with a
///     different gap would not match the parts' footprints).
/// strategy : str, optional
///     ``"nfp"`` (default here — reuse pays off most for NFP, which builds the O(P²) pairwise table)
///     or ``"sampling"``. Must match the intent of subsequent ``nest`` calls.
///
/// Returns
/// -------
/// PreparedParts
///     A handle whose ``nest(qty, container, holes, seed, budget, restarts=1,
///     separation_effort="full")`` method nests into one container, reusing the precompute.
#[pyfunction]
#[pyo3(signature = (items, rotations, min_sep, strategy="nfp"))]
#[allow(clippy::needless_pass_by_value)]
fn prepare(
    py: Python<'_>,
    items: Vec<Vec<[f64; 2]>>,
    rotations: Bound<'_, PyAny>,
    min_sep: f64,
    strategy: &str,
) -> PyResult<PyPreparedParts> {
    let n_items = items.len();
    // Resolve `rotations` (uniform or per-item) and `strategy` while the GIL is held — both touch
    // Python objects, so they must run before the release (see the note in `nest`).
    let rotations = parse_rotations(&rotations, n_items)?;
    let strategy = parse_strategy(strategy)?;
    // Release the GIL for the (synchronous, single-threaded, O((P·R)²)-for-NFP) part-library build —
    // same rationale and safety as `nest`. Without this a worker-thread `prepare` freezes the whole
    // interpreter for the entire build (measured ~4.3 s on a dense NFP job), stalling the caller's
    // asyncio event loop the same way a GIL-holding solve did. `core_prepare` is pure Rust over owned
    // (`items`) / already-parsed (`rotations`) / `Copy` inputs and touches no Python object, so the
    // closure is `Ungil`; releasing the GIL alters only scheduling, never the computation, so the
    // produced `PreparedParts` and every downstream layout stay byte-identical. Error mapping runs
    // after the GIL is re-acquired (the closure must not touch Python).
    let inner = py
        .detach(|| core_prepare(&items, &rotations, min_sep, strategy))
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(PyPreparedParts {
        inner,
        n_items,
        min_sep,
    })
}

/// The `ironnest` Python module. (The function name must match `[lib] name` for the abi3
/// `PyInit_ironnest` symbol.)
#[pymodule]
fn ironnest(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // Build provenance for the consumer's per-cut audit sidecar (issue #258 `engine{}` block): the
    // upstream jagua-rs commit this CDE was forked from, and this wheel's own build commit. Both are
    // metadata strings that never touch the placement path — they do not affect the determinism
    // contract. `__commit__` is injected by CI via the `IRONNEST_GIT_SHA` env (`option_env!` reads it
    // at compile time — no `build.rs`, per CLAUDE.md); local/sdist builds without it read "unknown".
    m.add("__jagua_fork_rev__", "43e8137")?; // upstream jagua-rs 0.7.2 base — see docs/01
    m.add(
        "__commit__",
        option_env!("IRONNEST_GIT_SHA").unwrap_or("unknown"),
    )?;
    m.add_function(wrap_pyfunction!(nest, m)?)?;
    m.add_function(wrap_pyfunction!(nest_multi, m)?)?;
    m.add_function(wrap_pyfunction!(prepare, m)?)?;
    m.add_class::<PyPreparedParts>()?;
    Ok(())
}
