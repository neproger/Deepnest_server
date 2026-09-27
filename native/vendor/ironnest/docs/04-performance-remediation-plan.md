# 04 — Performance remediation plan (NFP-mode pack timing)

**Status: 2026-07-10.** Review + empirical profiling of the "265 s pack" (consumer mixed-13 in NFP
mode). This document is the source of truth for the perf work. It (a) verifies the three proposed
inefficiencies against source, (b) reprioritizes them against measured data, and (c) lays out the
full remediation workflows. It defers to the two prime directives in `CLAUDE.md` throughout.

> **Headline:** the three proposed fixes are all *real*, but they target **~1.4 % of the runtime**.
> Measured, **~98 % of the 265 s pack is the shared `sep::run_separation` tail** thrashing on the
> over-subscribed unplaced parts — a cost sampling mode pays too. The remediation must lead with the
> separation tail, and the biggest sep levers are **placement-changing** (PRNG-coupled → golden
> re-bless), not free.

---

## 1. Measured baseline (the reframing)

Reproduced with `crates/optimizer/examples/perf_diag.rs` (throwaway, uncommitted) +
`examples/bench.rs` + `examples/nfp_spike.rs`, release build, consumer mixed-13 shapes, 122
instances in a 120×60 sheet, `min_sep = 0.375`, `restarts = 1`:

| Phase | Time | Placed | Share |
|---|---:|---:|---:|
| NFP cache build (eager O((P·R)²) pairwise table + import) | ~459 ms | — | 0.5 % |
| NFP construction only (`nfp_spike`, lazy cache) | ~908 ms | 51/122 | 0.9 % |
| **NFP full pipeline, restarts=1** | **96,534 ms** | 53/122 | 100 % |
| → implied `sep::run_separation` tail | **~95,000 ms** | +2 parts | **~98.6 %** |
| Sampling full pipeline (no NFP work at all) | 111,392 ms | 55/122 | — |

Two facts fall out and drive everything below:

1. **The separation tail dominates by ~100×.** Construction + build ≈ 1.4 s; the rest is
   `sep::run_separation`. It runs on **every** strategy (`lib.rs:896`, outside the `match strategy`
   block), which is why **sampling mode is even slower (111 s) with zero NFP-specific work**. The
   265 s pack is a *separation* problem, not an NFP problem.
2. **The sep tail is almost pure waste on this pack.** It adds **2 parts over 95 s** (~47 s/part)
   because the pack is ~1.7× over-subscribed: ~69 parts never fit, and `run_separation` runs the
   **full** separator budget on each hopeless one before giving up.

### Why the sep tail is so expensive
`run_separation` (`sep/mod.rs:54`) iterates every still-unplaced instance and calls `try_insert`,
which per attempt does:
- `search::lowest_overlap_pose` — **400** seed samples (`SEED_SAMPLES`), each a CDE overlap eval;
- `layout.save()` snapshot (clones container + slotmap + CDE snapshot);
- `separator::separate` — up to `strike_limit(4) × iter_no_imprv_limit(150)` = **600** `move_items`
  sweeps (`separator.rs:55`), each sweep re-searching **every** currently-overlapping part with
  `80 + 40` samples × `3` coord descents (`SEP_CONFIG`), and `save`/`restore` on each improvement;
- innermost: `CDEngine::collect_poly_collisions` + `proxy::quantify_collision_poly_poly`, reaching
  ~10⁶–10⁷ calls per pack.

---

## 2. Verification of the three proposed fixes

All three are **real** and correctly located. Verified against source (29-agent adversarial review +
first-hand read). Corrected impact and caveats:

### C1 — Rotation branch-and-bound pruning · `nfp/place.rs:96` — REAL, byte-identical *with care*, small
- **Confirmed:** `try_place_one` computes `feasible_region` (a full `difference(ifp,&clips)`) for
  **all** `n_rots` rotations up front (`place.rs:98-103`), then sorts and first-feasible-walks.
- **The LB is valid:** `region ⊆ IFP` as a point set; the cost `10·(x+bx)+(y+by)` (`mod.rs:438`) is
  linear with strictly-positive coefficients, so its min over the IFP polygon is at an IFP vertex and
  equals `candidates(&ifp,loss).first().cost`. Every region vertex — *including* the new vertices the
  boolean subtraction creates — is a point inside the IFP, hence has cost ≥ LB(r). So LB(r) lower-
  bounds every candidate rotation r can emit. ✔
- **Byte-identical only with the pair tie-break.** The winner is the lex-min `(cost, rot_idx, x, y)`
  that passes CDE. The sound skip predicate is the **pair** comparison
  `(LB_cost(r), r) > (best.cost, best.rot_idx)` — **never** a cost-only `LB(r) ≥ best_cost`, which
  silently returns the wrong rotation on a cost tie (a byte-identity break, i.e. a bug). Implement as
  a **lazy lower-bound k-way merge** (open a rotation's region only before verifying a candidate whose
  key could beat it), not a per-rotation greedy commit. Only `NfpPlaceStats.verify_rejections`
  changes, and the golden snapshots only `item x y rot` (`golden_dump.rs:418`, `tests/golden.rs`), so
  placements stay byte-identical.
- **Corrected impact:** the blowup is bounded by `n_rots = 4` (cardinal set), **not 18×**. It cuts
  only region construction in the two construction/refill `place_pass` calls; realistic **~1.3× on
  construction**, ≈ **1 % of total** wall-clock on the sep-dominated pack.

### C2 — `PreparedParts` cross-call cache reuse · `lib.rs:513` / `nfp/mod.rs:231` — REAL, byte-identical, large *for the right workload*
- **Confirmed & byte-identical.** `PartsGeometry` is a pure function of `(items, rotations, min_sep)`
  and container-independent (already `Arc`-shared across sheets in `nest_multi_with_config:605-621`
  by the exact same argument, goldens untouched). `import_item` takes no container; the only per-call
  work is `import_container` + `NfpCache::assemble`'s IFPs. Reuse just moves the boundary from
  "within one call" to "across calls." No interior mutability, no PRNG. ✔
- **Impact is workload-dependent.** For the **part-in-part M-call** loop (many calls, identical
  parts, different containers) it removes the O((P·R)²) pairwise table + O(P) surrogate import on all
  `M−1` subsequent calls — approaching an **M× saving on the precompute fraction**. That is the
  *strongest* byte-identical win in this document **for that workload**. For the single 265 s pack it
  saves ~0.46 s of 96.5 s (a rounding error) — the pack is sep-bound, not build-bound.

### C3 — Parallelize the pairwise build · `nfp/mod.rs:231` — REAL, byte-identical *with care*, negligible
- **Confirmed feasible & byte-identical** if done right: reduce into the `BTreeMap` keyed
  `(ta,ra,tb,rb)` (iteration by key, thread-order-independent), pure-integer geometry (no float
  reduction), `std::thread::scope` (never `rayon`), and preserve the mirror-derivation equivalence
  (`reflect`-derived NFP(B←A) must equal `canonicalize_shapes(nfp_pair(b,a))` — verify, else keep the
  compute/derive split). Requires re-running the x-platform golden under `--features parallel`.
- **Corrected impact: negligible.** The whole pairwise build is ~0.46–1 s (docs/03: "1.04 s ≈ 0.35 %
  of a K=16 nest"). Parallelizing 1 s of a 96.5 s wall is **statistically invisible**. **Drop it**
  (or fold the build-once into C2, which subsumes it).

---

## 3. The determinism dividing line (read before §4)

The separation tail is on the **placement-deciding path** and it **consumes the PRNG** on every
attempt (`search_placement` draws per sample; `move_items` shuffles). Therefore:

- **Any sep change that alters PRNG draw-count or control flow** — skipping a hopeless part, capping
  the separator budget, early-terminating the tail — **shifts the stream for every subsequently-
  attempted part and changes their placements.** These are **placement-changing** (density may move
  ±, requires a deliberate golden re-bless; the consumer re-validates every layout so this is safe
  for them, but it is *not* free).
- **Byte-identical sep speedups** are exactly those that keep draw-count and control flow identical
  while making each iteration **cheaper**: caching per-type invariants, CDE accel tuning, collision-
  query micro-opts. These speed the dominant phase with **zero output change**.

This split is the backbone of the tiering below.

---

## 4. Remediation workflows (priority order)

### TIER 0 — Tame the separation tail. **SHIPPED 2026-07-10 as `SeparationEffort::Fast`.**

Implemented behind a `NestConfig.separation_effort` knob (`Full` default = byte-identical shipped
behavior; `Fast` = opt-in) and the Python `separation_effort="full"|"fast"` kwarg. `Fast` combines the
sound **area-conservation skip** (0a) with a **reduced per-insert separator budget** (0b) — because on
realistically-fragmented packs the area-skip alone rarely fires (free area sits ~21%, above any one
part), so the budget cut is the effective lever. Measured, consumer mixed-13, restarts=1:

| mode | Full | Fast | speedup | density |
|---|---:|---:|---:|---|
| NFP | 95,826 ms → 53/122 | **6,367 ms → 52/122** | **15×** | −1 part (0.8%) |
| sampling | 109,116 ms → 55/122 | **6,038 ms → 55/122** | **18×** | **0 parts** |

Determinism: `Full` proven byte-identical (every existing golden case unchanged; verified by direct
snapshot diff). `Fast` is a distinct, still cross-platform byte-identical result (pure-integer
construction + seeded fixed-budget separator), pinned by two new golden cases (`nfp-fast-reduced-budget`
exercises the reduced separator, `nfp-fast-area-skip` exercises the skip). The knob threads through the
config path only; the sampling default path is untouched. Details below were the design; the shipped
code is `crates/optimizer/src/sep/mod.rs` (`SEP_CONFIG_FAST`, `SEED_SAMPLES_FAST`, area-skip) +
`SeparationEffort` in `lib.rs`.

*(Original design notes — placement-changing → opt-in + re-bless:)*

The consumer cares about wall-clock and re-validates every layout, so a faster, slightly-different-
density result is acceptable **as an opt-in**. Gate all of this behind a new
`separation_effort` / config knob whose **default reproduces today's behavior** (so the committed
golden is untouched); the consumer opts into the fast tier.

**0a. Area-conservation hopeless-skip (biggest single win).**
- *Mechanism:* before `try_insert`, if `Σ placed CD-footprint area + this part's CD-footprint area >
  container CD area`, no collision-free arrangement can exist (area is invariant under the
  separator's moves), so the separator **must** fail. Skip it.
- *Expected:* on a 1.7× over-subscribed pack this short-circuits most of the ~69 hopeless attempts →
  collapses ~95 s toward a few seconds.
- *Determinism:* **placement-changing** — skipping an attempt that *would have failed anyway* still
  removes its PRNG draws, shifting later parts. Ship behind the opt-in knob; re-bless the golden for
  the fast tier (add a fast-tier golden case). Track placed-area delta (expected ≈ 0, since skipped
  attempts fail).
- *Workflow:* (1) add `container_cd_area` + running `placed_cd_area` accounting to `run_separation`;
  (2) gate `try_insert` on the area test; (3) thread a `SeparationEffort` enum through
  `NestConfig`→`run_separation`; (4) bench mixed-13 wall-clock + placed-area at each effort; (5)
  determinism-auditor pass; (6) add a fast-tier golden case and bless it.

**0b. Adaptive separator budget when over-subscribed.**
- *Mechanism:* scale `SEP_CONFIG` (strikes / iter_no_imprv / seed samples) down once the sheet is
  near-full or after *N* consecutive failed inserts; give up the tail after a global failure streak.
- *Determinism:* placement-changing (same knob/re-bless as 0a).
- *Workflow:* add the streak counter + budget schedule to `run_separation`/`separator`; sweep the
  density/time trade on mixed-13; document the Pareto point; bless.

**0c. Snapshot-cost reduction in `try_insert` / `separate`.**
- *Mechanism:* `layout.save()`/`restore()` clone container + slotmap + CDE snapshot per attempt and
  per separator improvement. An incremental undo-log (record placed/moved keys, roll back deltas)
  avoids the full clone. This one **can** be byte-identical (it changes *how* state is saved, not
  what is decided or drawn) — verify no float/order change.
- *Determinism:* byte-identical if the delta-restore reproduces the exact `LayoutSnapshot` state;
  guard with the existing `assertions::snapshot_matches_layout` debug asserts + the golden.

### TIER 1 — Byte-identical speedups of the dominant phase (no output change, no re-bless).

> **MEASURED VERDICT (2026-07-10): Tier 1 does not move the needle on a sep-bound pack.** The sep
> tail's cost is *inherent* work (O(poles²) proxy + CDE query, per sample, ×600 sweeps, ×hundreds of
> samples, ×~69 hopeless parts). The two byte-identical ideas below cap out at low-single-digit %:
> 1b was empirically ruled out (a wash / within noise), and 1a's target is ~1% of a proxy term. Keep
> them only as opportunistic cleanups. **The real speedup requires reducing the work → Tier 0.**

**1a. Cache the per-type sep proxy invariants · `sep/proxy.rs:66,80`.** — *marginal, skip unless free.*
- `calc_shape_penalty` (3× `sqrt`) + `epsilon` are rigid-invariant per type yet recomputed per pair-
  quantification. BUT the dominant per-term cost is `overlap_area_proxy` — an O(poles²) loop
  (~100–400 pole-pairs, each a `distance_to`/sqrt); `penalty`+`epsilon` are ~4 ops (~1% of a term).
  Worse, the recomputed `sqrt` is *cheaper* than any per-type memo-map lookup that would replace it.
  Net expected win **≲1%** of the sep tail. Byte-identical if done (same values/order/draw-count), but
  not worth the API surface. **Recommendation: skip.**

**1b. CDE acceleration tuning · `lib.rs:108` (`default_cde_config`) — MEASURED: no reliable win, keep (5,16).**
- Empirical sweep (throwaway env-override harness in `default_cde_config`, since reverted), sep-heavy
  NFP packs, depth ∈ {4..8} × cd_threshold ∈ {8,16,32}: **`placed` was constant across every config**
  (the byte-identity proof — accel-only), and wall-clock varied only ~±5% (best-of-2 noise). `(5,16)`
  is already near-optimal; `depth=4` was ~1–2% faster on square packs (noise), and **`depth≥7`
  clearly *hurts*** (small parts gain nothing from a deeper tree). **Do not change the defaults.**
- **1b′ (wide sheets).** The container quadtree is built over `bbox.inflate_to_square()`
  (`container.rs:69`), so a high-aspect sheet under-resolves the short axis. Confirmed no material
  gain on the 2:1 mixed-13 pack; only worth revisiting for extreme (≥3:1) sheets, and it touches the
  forked CDE (higher risk). Deprioritized.

**1c. NFP construction micro-opts (small, but free).**
- **C1 rotation B&B** (§2) — ~1.3× on construction, byte-identical with the pair tie-break.
- **`buffer` hoist · `place.rs:106`** — clone `item.shape_cd` **once per type** in `place_pass`, not
  once per instance (a q-demand type clones q times today). Byte-identical.
- **Query-path canonicalization skip · `mod.rs:392`** — `feasible_region`'s `difference` runs full
  `canonicalize_shapes` (Booth rotation + two sorts) on a throwaway region that `candidates()`
  immediately re-sorts+dedups. A query-only `difference` variant that applies only the winding-fix +
  zero-area drop is byte-identical (verify the candidate multiset/order is unchanged).
- **Allocation churn · `mod.rs:372,384` / `place.rs:97`** — `feasible_region` builds a throwaway
  `Vec<Vec<Ring>>` of `translate()`-copied clip rings per rotation per placement; `cands`/
  `candidates()` grow without capacity hints. Add a translate-on-the-fly `difference` variant + a
  reused scratch buffer + `reserve`. Byte-identical (no output/order change).
- **Optional `canonical_rotation` → real Booth (O(n))** at `geom.rs:252` (currently a nested
  min-scan). Byte-identical (identical least rotation); only worth it if canonicalization stays on a
  hot path after the query-path skip.

### TIER 2 — Cross-call reuse for the part-in-part workload. **SHIPPED 2026-07-10.**

Implemented `PreparedParts` + `prepare()` + `nest_with_prepared()` (Rust) and the Python
`ironnest.prepare(items, rotations, min_sep, strategy="nfp") -> PreparedParts` with a
`prepared.nest(qty, container, holes, seed, budget, restarts, separation_effort)` method. Byte-identical
to `nest_with_config` (proven by `prepared_equals_nest_with_config_byte_for_byte` across containers /
strategies / seeds / restarts, and by the untouched goldens). Measured part-in-part (13-type library
into M=15 containers, NFP): **per-call 6,898 ms → prepared 584 ms = 11.8×**, placements identical; the
saving approaches M× as the container count grows. Refactored `import_problem` into
`make_importer`/`normalize_rotations`/`import_entities` (the container-independent item import) — all
`&self` on the importer, so the split is provably byte-identical. Subsumes C3. **Consumer follow-up
still open:** `nest_across_sheets` in `drawing_and_gcode` calls `nest()` per sheet and leaves this reuse
on the table.

**Also shipped (byte-identical NFP-construction micro-opts):** `buffer` hoisted to one clone per type
(was per instance) in `place_pass`; `candidates()`/`cands` pre-sized. Goldens unchanged.
**Deliberately deferred:** the `feasible_region` obstacle spatial-prune (the IFP bbox ≈ the whole
container, so few obstacles' NFP bboxes fall outside it → weak) and the rotation branch-and-bound
(~1.3 % of construction at real tie-break risk). After Tier 0 + Tier 2 the region query is no longer
a dominant cost, so neither earns its complexity.

**2a. `PreparedParts` handle (C2)** — the one large byte-identical construction win, for the M-call
part-in-part pattern. *(Original design notes:)*
- *Rust API:* `PreparedParts::prepare(items, rotations, min_sep) -> PreparedParts` (holds
  `entities`, rotation sets, `min_sep`, `Arc<PartsGeometry>` for NFP, `areas`), then
  `nest_with_prepared(&prepared, container, holes, seed, budget, strategy) -> …`. Keyed on
  `(items, rotations, min_sep)`; error if the caller reuses it with a different `min_sep`.
- *PyO3:* expose `ironnest.prepare(...) -> PreparedParts` and `prepared.nest(container, …)`; the
  handle is immutable from Python (no mutation surface).
- *Also fix the consumer:* `nest_across_sheets` in `drawing_and_gcode` currently calls `nest()` per
  sheet and leaves the same reuse on the table.
- *Determinism:* byte-identical (same argument the multi-sheet path already relies on). Prove with a
  test asserting `prepare + N× nest_with_prepared` == `N× nest_with_config` byte-for-byte, and by the
  untouched existing goldens. Subsumes C3.

### TIER 3 — Parallelism reality check.
At `restarts=1` the pipeline is single-threaded **by determinism directive** (no threads on the
placement-deciding path). The only sanctioned parallelism is the K-start meta-loop (`--features
parallel`), which *multiplies* total work — it helps density-per-wall-clock only when the consumer
actually wants best-of-K. **C3 (parallel build)** is defensible but <1 %. There is no
determinism-legal way to parallelize a single start's sep tail or region queries. Recommendation:
document that the 265 s pack is single-threaded on purpose, and that cores are usable only via
`restarts>1 --features parallel`.

---

## 4d. Tier D — Densification / remnant compaction (`SeparationEffort::Max`, v1 shipped 2026-07-10)

The goal a consumer actually wants is **maximize the usable remnant** — compact the parts into the
smallest region so the rest of the sheet stays one clean piece. The leftover-only separator never
moves an *already-feasible* part (no overlap ⇒ no pressure), so a gappy-but-feasible layout is never
tightened. `Max` adds a compaction loop: shrink the sheet along the **slack axis** (LBF grows
full-height columns rightward, so the gaps live along the growth axis) toward the corner, re-run the
**full-layout GLS separator** (`sep::separate_layout` — moves every part, samples rotations) to pull
the arrangement inward, swap back, refill freed space, keep the arrangement with the smallest used
region (`used_extent`/`placed_used_bbox_area`). Shrink is relative to the *used* span (not the sheet)
in 1 % steps so the local separator can follow a slowly-tightening boundary.

**Measured (cone+square, all-fit, restarts=1):** remnant 53.0 % → **55.8 %** (used width 112.8 →
106.2); packing within the used region ~78 % → ~82 %. **It works — refutes docs/02's "architectural
ceiling" (that was measured on rotation-invariant / no-slack rectangle cases + the pentagon, never a
low-util irregular case).** Fully opt-in; `Full`/`Fast` and all goldens untouched.

**Limitation / next (the human-level gap):** the *local* GLS plateaus at ~82 % vs ~92 % achievable —
more separator budget only adds PRNG noise. Closing the rest needs a stronger **global** search:
(1) a **remnant-aware best-of-K keep-best** (today the multi-start keeps highest *area*, ignoring
which start densified tightest — an easy high-value fix); (2) **re-seed-and-separate** (sparrow's
method: place all parts overlapping in a tight box and separate from scratch, escaping the constructed
basin); (3) **simulated-annealing / ruin-and-recreate** acceptance. Verified visually via
`examples/density_remnant.rs` (writes `/tmp/nest_{full,max}.svg`).

## 4c. Density (packing tightness) — 2026-07-10 experiments

Prompted by a consumer nest showing large gaps between big "developed-cone" (annular-sector) parts.
Measured with `examples/density_probe.rs` (throwaway; over-subscribed so
placed-area = tightness). Root cause: the placement loss is pure column-**LBF** (`10·x_max + y_max`
in both `loss.rs` and `nfp/mod.rs candidates()`) — it prefers the bottom-left-most feasible pose, not
the *interlocked* pose tucked against a neighbor, and once placed a part is never rearranged (the GLS
squeeze runs only on **unplaced** parts). Findings:

1. **Finer rotations do NOT help** these (roughly convex) sectors: NFP K=8 = 84.1 % at cardinal,
   12-way, and 45-step alike — at 1.5× the runtime. **Do not add rotations for cone parts.**
2. **NFP + restarts=8 beats sampling K=1 by ~2.5 pp** (84.1 % vs 81.6 %) on the sector+square case.
   If the consumer runs sampling / low-K here, switch to `strategy="nfp"`, `restarts=16`.
3. **The LBF column weight is the real lever.** `X_MULTIPLIER = 3` (vs the shipped 10, inherited from
   jagua) measured **+0.9 pp bricks (94.6→95.5), +0.7 pp mixed (93.1→93.8), +2.5 pp on the sectors
   (84.1→86.6), pentagon flat, no regressions** across 4 corpora. The heavy `10×` over-forces strict
   left-columns and leaves vertical gaps; a gentler weight packs tighter. `max(x,y)` (mode 3) is
   inconsistent (helps pentagon, wrecks bricks) — not shippable. **This is placement-changing (golden
   re-bless), and docs/02's standing rule is to spec density changes against the consumer's
   `nest_benchmark.py --mixed` before committing** — so ship it either as an opt-in `column_weight`
   knob (default 10 = byte-identical) OR change the default after the consumer A/Bs it on real jobs.
4. **Compaction is a measured dead-end** (docs/02 §11.2: "do not re-try wall/shrink compression") —
   the part-targeted GLS `try_insert` squeeze already subsumes it.
5. The wide-squares 79 % (vs ~100 %) is purely the **□E contact-backoff exact-fit artifact**: exact
   200×60 = 95 squares, but 200.5×60.5 = 120 (99 %). Real sheets have `edge_margin` slack, so it never
   bites; not the image's cause.

### 4c′ — Product goal: maximize remnant at the sheet end

**Any nest's end goal is the largest usable remnant at the end of the sheet** (a clean free strip after
the packing, not merely high utilization of an over-subscribed demand). Operationalized as a
lexicographic keep-best score ([`NestScore`] in `lib.rs`):

1. **Maximize placed area** — never drop a placeable part to "make remnant".
2. **Minimize `used_x_max`** — LBF packs toward the low-x corner; free sheet lives at high-x.
3. **Minimize `used_y_max`** — secondary packing-down.
4. Earliest multi-start *k* on a full tie.

This score drives multi-start keep-best (all entry points) and densify's keep-best. `K=1` is
byte-identical to the previous area-only reduction (no competing starts). `K>1` is
placement-changing when two starts place equal area but different far-edges — deliberately, because
the product prefers the tighter end-remnant. Use `restarts≥8` + `column_weight=3` +
`separation_effort="max"` for remnant-critical jobs.

### 4c″ — Shipped 2026-07-10 follow-up (independent re-measure, branch `perf/nfp-density-and-speed`)

Reproduced the sector corpus and **did not trust** the earlier "nothing left" conclusion. Results:

| lever | result | ship? |
|---|---|---|
| **`NestConfig.column_weight` (opt-in, default 10)** | NFP cone+square K=1: **83.3 % → 86.6 %** at `w=3` (+3.3 pp, +8 squares). Fast path still holds the gain (81.6 → 85.8). Goldens untouched at default 10. | **SHIPPED** — Rust `column_weight` + `DENSITY_COLUMN_WEIGHT=3`; Python `column_weight=` kwarg on `nest` / `nest_multi` / `PreparedParts.nest` |
| **Post-separation NFP refill** (docs/03 §13 Q5) | Rebuilds integer obstacles from the live layout after `run_separation` (+ after Max densify) and runs one more NFP `place_pass`. No-op when the sheet is already full (goldens byte-identical). Seats parts into pockets sep opens. | **SHIPPED** (always on for NFP) |
| **Local NFP re-pack** (remove each part, re-place at LBF-best NFP pose) | **REJECTED**: at `w=10` regressed util **83.3 → 82.5 %** by rearranging a feasible construction into a worse basin for the sep tail. Do not re-try without a keep-best guard. | no |
| **Finer rotations / more restarts alone** | Confirmed dead for sector tightness once `w=3` is on (K=4 = K=1 = 86.6 %). | no |
| **Remnant-first multi-start keep-best** (`NestScore`) | Max placed area, then min packing far-edge (`used_x_max`, `used_y_max`). `K=1` byte-identical; `K>1` prefers tighter end remnant when areas tie (re-blessed `multistart-mixed` golden). Measured: Max+w3 **K=1 max_x 103.6 / remnant 56.8 %** → **K=8 max_x 103.4 / remnant 57.0 %**. | **SHIPPED** |
| **Post-NFP BL compact** | **REJECTED** — regressed over-subscribed util 86.6→85.8 %. Remnant of feasible layouts is Max densify, not free compact. | no |
| **Recommended consumer settings** | **Remnant-critical (all parts fit):** `strategy="nfp"`, `column_weight=3`, `restarts≥8`, `separation_effort="max"`. **Over-subscribed / wall-clock:** same + `separation_effort="fast"`. | docs |

Performance posture is unchanged from §5: sep tail dominates; use `SeparationEffort::Fast` (15–18×) and `PreparedParts` (11.8× part-in-part). There is still **no free byte-identical 10×** for a single over-subscribed start — the work is inherent.

## 4e. Max densify-once multi-start (shipped 2026-07-11) — production path speed

**Target production config** (drawing_and_gcode station): `strategy=nfp`, `restarts=8`,
`separation_effort=max`, `column_weight=3`. Density knobs are already dialed; the wall-clock tax
was **Max densify × K starts** (losers densified then discarded by NestScore keep-best).

**Policy (now default for Max when `restarts > 1`):**

1. Explore all K starts at **Full** (construction + leftover sep + post-sep NFP refill — no densify).
2. NestScore keep-best picks the winner (same remnant-first order).
3. Re-run **only** that start with Max densify (+ post-densify NFP refill).

`restarts == 1` densifies that single start unchanged (`nfp-max-densify` golden byte-identical).
Full/Fast multi-start paths unchanged.

**Also shipped with this change (byte-identical densify micros):**

- Skip per-step leftover `run_separation` inside densify when demand is already met (all-fit jobs).
- `Layout::swap_container` returns the previous container so densify restores the real sheet without
  re-cloning it every shrink step.

**Measured (`examples/density_remnant`, cone+square all-fit, release, sequential):**

| Config | wall | remnant | max_x |
|---|---:|---:|---:|
| Max w3 K=1 | 2570 ms | 56.8 % | 103.6 |
| Max w3 K=8 (densify-once) | **2607 ms** | **56.8 %** | **103.6** |
| Full w3 K=8 (explore cost only) | 46 ms | 52.9 % | 113.4 |

Previously Max K=8 densified every start (~8× densify wall). Densify-once makes K=8 ≈ K=1 + cheap
Full multi-start (~same densify once). Wheel already builds with `--features parallel` so explore
starts can wall-clock-overlap further on multi-core.

**Follow-up densify / NFP speed (same day, still on Max path):**

| lever | result |
|---|---|
| NestScore plateau early-stop (`DENSIFY_PLATEAU=5`) | Safety net; no win when score improves until the last feasible step |
| Align `DENSIFY_SEP` → `SEP_CONFIG` (was 200×6) | ~2× densify (2570 → 1306 ms), remnant held |
| Align `DENSIFY_SEP` → **`SEP_CONFIG_FAST`** | **~16× densify** (2570 → **~160 ms** Max K=1), remnant **held** 56.8 % / max_x 103.6; goldens held |
| 2 % shrink steps (`0.98`) | ~20 % faster but +0.2 max_x — **not shipped** |
| Pose-only snapshots (0c partial) | Byte-identical; wall within noise on densify-bound packs |
| Query-path `difference_query` (no Booth canonicalize) | Byte-identical candidate multiset; small construction win under K restarts |

Max w3 K=8 after densify-once + Fast densify sep: **~190 ms**, remnant 56.8 % (vs ~20 s densify-every-start baseline at session start).

**Max leftover = Fast (same day):** Max’s pre-densify leftover insertion now uses Fast budgets +
area-skip (not Full). Over-sub packs no longer pay the ~97 s Full leftover tax under Max; densify
still runs after. All-fit remnant corpus held; goldens held. Multi-start Max explores at Fast too.

**Still open:** full 0c undo-log of CDE hazards; rotation B&B on NFP place; re-seed densify for
remnant quality.

## 5. Recommended sequence (revised after Tier 1 measurement)

1. ~~**Tier 1 (proxy caching + CDE tuning)**~~ — **measured a wash** (§4). Ruled out; the sep tail is
   inherent work. Shipped the byte-identical **`opt-level=2` dev/test build fix** instead (§6b).
2. ✅ **Tier 0 `SeparationEffort::Fast`** — **SHIPPED**. 15× (NFP) / 18× (sampling) on the mixed-13
   pack, ≤1 part density cost; opt-in, `Full` default byte-identical, two new golden cases blessed.
3. ✅ **Tier 2 `PreparedParts`** — **SHIPPED**. 11.8× on M=15 part-in-part, byte-identical; Python
   `prepare()`/`prepared.nest()`. Subsumes C3.
4. ✅ **Micro-opts** — buffer hoist + capacity hints SHIPPED (byte-identical). C1 B&B + spatial-prune
   deferred (marginal after Tier 0+2).
5. **0c** (incremental snapshot restore) + the consumer `nest_across_sheets` reuse — still open.
6. **Drop C3** (parallel build, <1 % — subsumed by PreparedParts).

**Bottom line:** there is no free (byte-identical) way to make the 265 s pack dramatically faster —
its cost is the separator doing real work on ~69 unplaceable parts. The decisive lever (0a) trades a
deliberate, re-blessed placement change for ~70×, which is the right deal for a consumer that
re-validates every layout.

## 6b. Test-suite speed (shipped 2026-07-10)

Observed separately: `cargo test` was painfully slow. Root cause — `cargo test` builds the engine (and
the `golden_dump` bin the golden test runs ×3) at **`opt-level = 0`**, and the engine is compute-bound,
so real nests run ~10–12× slower unoptimized. The golden test alone: **~633 s at opt 0 → ~51 s at opt 2**
(release `golden_dump` is ~2.8 s/run). Fix shipped: `[profile.dev] opt-level = 2` in the root
`Cargo.toml`. Byte-identical (opt-level is codegen-only, no fast-math, no IEEE change → all goldens pass
unchanged; determinism's lever is `codegen-units`/`target-cpu`, left on the release profile). `opt-level
3` measured no faster; `codegen-units = 1` shaved only ~5 s at a compile cost — not worth it. The
*residual* ~51 s is the separation tail doing real work (this document) — the same root bottleneck, so
Tier 0 also speeds the heaviest tests. Verifies the user's hypothesis: test slowness was the engine
bottleneck, amplified ~12× by the unoptimized profile.

## 6. Determinism checklist (every change)
- Byte-identical changes: prove with `cargo test -p ironnest-optimizer` (goldens) unchanged, plus a
  determinism-auditor pass; no new `HashMap`/threads/std-trig/`mul_add`/RNG.
- Placement-changing changes (Tier 0): gate behind an opt-in whose default is today's behavior;
  add a *separate* fast-tier golden case and bless it deliberately; re-run the x-platform CI golden.
- Never thread inside a single placement search. Keep `i_overlay` pinned. Keep the oracle boundary
  (no machine concepts).
