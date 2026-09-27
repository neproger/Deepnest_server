# ironnest exact-NFP construction mode — design & build plan (feature #4)

**Status (2026-07-10): N1a ✅ · N1 ✅ (§11.2) · N2 ✅ (§11.3) · N3 ✅ (§11.4) · N4 ✅ core
(§11.5) — ACCEPTANCE VERDICT: SHIP OPT-IN** (+0.61 pp mean over sampling at 1.02× wall-clock on the
consumer corpus, floor held on every seed, 84.03 % all-time best; wheel ships
`strategy="sampling"|"nfp"`). Remaining follow-ups: consumer adapter wiring (`drawing_and_gcode`),
ESICUP tracker corpus, verify-rejection counter surfacing, post-sep NFP refill (the GO lever). This
document is the source of truth for the opt-in exact no-fit-polygon (NFP) constructive placement
mode. Produced by a five-axis research pass (Rust ecosystem survey with source-level dependency
audits, NFP algorithm literature + reference-implementation reads, a file:line integration map of
this repo, a determinism/numerics spec, and a validation/acceptance protocol), then revised against
a four-critic adversarial panel whose confirmed findings are folded in below (the two largest: the
union-fill needs quad orientation-normalization or winding cancellation under-fills concave NFPs —
§3.2; and the acceptance framing must budget *construction* time, because the separation tail
dominates wall-clock — §10). Read [`00-…`](00-ironnest-architecture-and-plan.md) and
[`02-…`](02-optimizer-and-separation-search.md) (§11.2 is the motivating evidence) first.

> **One-line orientation for a fresh session:** both existing search axes (sampling noise ×
> insertion order) saturate at ~83–84 % on the consumer acceptance harness; the remaining gap lives
> in the *constructive* phase. This feature replaces it — opt-in, default-off — with Deepnest-style
> exact contact placement: feasible region = IFP ∖ ⋃NFP in **scaled integer arithmetic**
> (byte-deterministic by construction), candidates = region vertices (exact for our linear loss),
> the CDE stays the feasibility arbiter. The density thesis is falsifiable early and cheaply
> (§11 N0); the honest headline risk is not correctness but **value** — §10.

---

## 1. Why NFP, and why now

- **The saturation evidence (docs/02 §11.2).** v0.4.1's all-canonical best-of-8 scores exactly
  82.76 % on every base seed of the consumer harness; order diversification lifted the ceiling to
  83.66 % (restarts=16 mean 83.44 %). Every other §11 lever measured dead.
- **The Deepnest gap analysis (2026-07-09, source-verified).** Deepnest's density edge is exact NFP
  contact placement: every candidate is a touching pose on the true feasible-region boundary,
  computed once per (pair, rotation) and cached.
- **NFP is a density + consistency + per-placement-cost lever**: the constructive pass becomes
  (near-)seed-independent, and each placement costs one boolean difference + vertex scan instead of
  `budget` CDE queries.
- **Why the docs/02 §7.2 "avoid NFP first" verdict flips.** That verdict targeted *float orbiting*
  NFP — genuinely a minefield (§3.3). Integer convolution (Boost.Polygon → Deepnest's
  `minkowski.cc`, in production since 2016) is a different animal: pure integer arithmetic after
  one deterministic snap is **more** cross-platform-deterministic than our float paths.
- **The honest counter-evidence (from the adversarial review, kept on purpose).** docs/02 §11.1's
  B3 negative result — structured contact-anchor candidates changed *nothing*, because
  `place_dropped`'s slide already snaps loose samples to axis-reachable contacts. NFP's genuinely
  new placements are the **non-axis-reachable pocket contacts** (poses needing simultaneous-x/y
  entry), whose share of real corpora is unquantified. And sparrow reaches ESICUP SOTA with
  sampling+GLS, *no* NFP construction; Deepnest's edge was measured against engines without a GLS
  separator. **Therefore the density thesis must be falsified or confirmed in N0 for ~2–4 days of
  work, before the main spend** (§10, §11).

**Non-goals:** part-in-hole nesting (a follow-on that reuses this substrate — the NFP output's
holes already encode it), continuous rotation, any change to sampling-mode behavior (byte-frozen),
any machine/kerf concept (prime directive #2).

---

## 2. Decision summary

| Decision | Choice | Rejected alternatives (why) |
|---|---|---|
| Boolean engine | **`i_overlay` = 7.0.2, integer API, i64 engine** (+ MIT family: i_float, i_shape, i_tree, i_key_sort), exact-pinned; **`Solver` strategy AND `Precision` pinned explicitly as part of the determinism contract** | clipper2 crates (C++ FFI/build.rs); geo BooleanOps (float wrapper + dropped dep tree); iron-shapes-booleanop (AGPL); writing our own (~17k-LOC class) |
| NFP algorithm | **Convolution union-fill with orientation-normalized pieces** (§3.2): every edge-pair quad and seed translate normalized to CCW by exact i128 shoelace *before* the union — an honest set union of true-NFP subsets, provably complete for hole-free simple pairs; **convex×convex O(n+m) edge-merge fast path** | Raw signed-winding quad soup (winding cancellation between CW/CCW quads under-fills concave NFPs — confirmed by worked counterexample in review); orbiting (§3.3); Ghosh/Bennell–Song tracing (libnest2d abandoned it); decomposition-first (same engine dep, worse output; fallback only) |
| IFP | **Frame trick** (bbox frame, container as hole → outer NFP → holes are the IFP) | Direct erosion via complement (unbounded complement, second winding convention) |
| Numerics | **Fixed global power-of-two grid 2^20/unit, i64 coords, i128 for ALL self-written predicates**; input domain validated with explicit headroom (§3.1) | Fixed decimal 1e7 (two lossy steps + i64 predicate overflow); adaptive per-call scale (cache incoherence, non-uniform ε) |
| Contact policy | **Fixed integer backoff `NFP_BACKOFF_E` (initial 16 grid units, sized empirically in N1 — §4)** as ONE L∞-square inflation of the moving footprint per (type, rot) | Per-candidate nudging (ambiguous direction); changing CDE touch semantics (`edge.rs` closed-interval predicate protects hazard-scope semantics engine-wide) |
| Candidates | **All vertices of the boolean-difference output**, integer loss `10·X + Y`, lexicographic `(cost, rot_idx, X, Y)` tie-break; first `feasible_at`-accepted wins; **sampling fallback (search + place_dropped, verbatim today's path) on empty region** | Edge sampling (vertex optimality is exact for a strictly linear loss — §5.2); raw input vertices (miss boolean-created crossings) |
| Cache | **Items imported ONCE per public call** (hoisted out of `nest_core` — §6), cache built before `run_starts` forks, `BTreeMap` keyed by rotation **indices**; point-reflection symmetry (mirrored entries **derived by negation, never recomputed**); region queries = **one flat multi-path union from pristine cached NFPs** per query (never re-union snapped output) | Per-start lazy cache (K× compute); incremental re-union of snapped accumulators (iterated snap-rounding has no additive error bound) |
| Mode switch | **`NestConfig` struct** (strategy + future knobs); the existing 8 public fns become thin frozen wrappers — a committed N2 deliverable, not an aside | 8 more fn variants (combinatorial explosion); parameter breaking all 8 signatures without consolidation |
| Arbiter | **Unchanged: the CDE**, via `feasible_at`; deterministic next-candidate on rejection; rejection counter surfaced (§10 gates it *on the acceptance harness* at 0) | Trusting the region (violates the advisory-signal + exact-arbiter pattern, `sep/mod.rs`) |

**The safety property, stated precisely** (the review corrected the original over-claim): for
shapes entering the region difference **negatively** (obstacle and hole NFPs), any construction
error is an under-filled NFP ⇒ an over-permissive region ⇒ bad candidates the CDE rejects —
density loss, never overlap. For the **IFP** (which enters positively), under-fill of NFP(frame,·)
propagates as an *under*-permissive or lost IFP loop — silent density loss with **zero rejection
signal**; the backstop there is the sampling fallback, and the validation covers it directly
(P2/P6 on concave containers, §9.1). With orientation normalization (§3.2) both directions reduce
to snap-rounding effects only.

---

## 3. The geometry substrate (`crates/optimizer/src/nfp/`)

### 3.1 Numerics spec — frames, scale, and bounds

**Coordinate frames (pinned; the repo has two and the review caught the gap):** items are
centroid-centered at import (`io/import.rs` applies `centering_transformation`); **containers are
not** (`pre_transform = DTransformation::empty()`). All NFP-module geometry lives in the
**container-native CDE frame**: cached NFPs are translated by placed items' internal
`d_transf.translation()` — **never** by the public `Placement (x, y)`, which folds the centroid
back out through a libm rotation (`search::original_to_placed`). Candidate poses produced by this
mode ARE `d_transf` translations, so they feed `Layout::place_item` directly.

```rust
pub type IntCoord = i64;                    // engine + our coordinates
const NFP_SCALE: f64     = 1_048_576.0;     // 2^20 grid units per input unit — FIXED, GLOBAL
const NFP_INV_SCALE: f64 = 1.0 / NFP_SCALE; // 2^-20, exactly representable
const NFP_BACKOFF_E: i64 = 16;              // contact backoff, grid units (§4; empirically re-sized in N1)
const NFP_FRAME_MARGIN: i64 = 64;           // frame-trick ring width, grid units
fn quantize(v: f64) -> i64 { (v * NFP_SCALE).round() as i64 }
fn unquantize(n: i64) -> f64 { (n as f64) * NFP_INV_SCALE }
```

**Quantization sites — exactly two** (rule §8-2): (1) shape vertices, once per (type, rotation) at
cache build, from the same f64 `shape_cd` collision shapes the CDE uses (post min_sep/2 inflation,
post decimation); (2) **obstacle poses**, once per off-grid pose — poses placed by this mode are
grid-exact by construction, but **sampling-fallback placements are arbitrary floats** and must be
quantized when they enter region queries (their ≤ 0.71g error is a budget line in §4).

**Input-domain validation with explicit headroom.** Worst-case coordinate entering a boolean op =
container extent + part extent + E + frame margin + pose translation. The validation therefore
checks the **derived worst case**, not a bare input bound:
`max_scaled_op_coord = S·(container_span + 2·max_part_span) + NFP_BACKOFF_E + NFP_FRAME_MARGIN`,
required `< 2^61` (headroom argument below), with a practical input gate of `max|coord| ≤ 500`
units on the container-native frame (hard error — mode fallback would create density cliffs). A
`debug_assert!` on `max|coord|` guards **every boolean-op entry**.

**Bounds, done honestly** (the original sketch failed at its own boundary — review finding, all
four critics): the oft-quoted `< 2^30` input bound in i_overlay's `split/cross_solver.rs` is
calibrated for the **i32 engine** (whose `Wide` is i64). The **i64 engine** computes intersections
in i128/u128-composite arithmetic; its effective coordinate bound is ~2^62. At S = 2^20 and a
500-unit domain: inputs ≤ 2^29; Minkowski sums ≤ 2^30 + E + margin; pose-translated NFPs ≤
~1.5·2^30 ≈ 2^30.6 — all with **> 30 bits of headroom** against 2^62. Our own predicates: coordinate
differences reach ~2^31.6, single cross products ~2^63.2, and orient2d is a **difference of two
such products, i.e. up to ~2^64 — overflowing i64**; hence the blanket rule (§8-4): **every
self-written geometric predicate accumulates in i128.** N0 verifies the i64-engine bound claim
empirically with boundary-magnitude inputs before anything else is built.

**Which polygons:** post-import `shape_cd` (min_sep/2-inflated, decimated when applicable) — never
raw input rings — so all existing min_sep/decimation guarantees inherit and the arbiter sees the
same geometry. Contact of two inflated CD shapes ⇒ original outlines exactly `min_sep` apart.

**Rotations:** cardinal orientations of the quantized 0° footprint by **exact integer swap/negate**;
non-cardinal via libm-rotate (existing rule) → quantize. The swap/negate vs rotate-then-quantize
discrepancy is **≤ 1 full grid unit per coordinate** (knife-edge rounding near half-grid values —
the review's counterexample corrected the earlier ½g claim), absorbed by E. Cache keys use
rotation-set **indices**, never float degrees.

### 3.2 NFP construction (orientation-normalized convolution union-fill)

Port of Boost.Polygon's `convolve_two_polygon_sets` semantics (Deepnest's `minkowski.cc:20-92`),
**with one mandatory correction found in review**:

1. For every edge pair (eA ∈ A-cycles, eB ∈ (−B)-cycles): emit the 4-point segment-sum quad
   `{a1+b2, a1+b1, a2+b1, a2+b2}`. Holes participate as extra cycles.
2. **Orientation-normalize every quad (and seed polygon) to CCW via exact i128 shoelace sign
   before insertion.** Without this, concave inputs emit both CW and CCW quads whose overlap nets
   winding 0 under NonZero fill — an under-filled NFP interior in exactly the pocket-bridging
   poses this feature exists to handle (worked counterexample in the review: a notched part × a
   bar; the two notch-wall quads cancel over the bridge zone). Degenerate/bowtie quads from
   parallel edges are normalized or dropped by exact zero-area test.
3. Seed interiors: translated copy of A at (−B)'s first vertex and of −B at A's first vertex
   (CCW-normalized like everything else).
4. One `Overlay::overlay(Union, NonZero)` on i64. Output = NFP as polygons **with holes**
   (an NFP hole = poses where B nests inside a pocket of A — the future part-in-hole substrate).

With normalization, the overlay is an honest **set union of pieces that are each subsets of the
true NFP** — so it never over-approximates, and completeness for **hole-free simple pairs is
provable** (case split: boundary-contact poses are covered by some edge-pair quad; B-inside-A poses
are covered by the A-translate seed; A-inside-B poses by the −B-translate seed; no other
non-overlap-free pose class exists for connected simple polygons). Holed inputs keep the empirical
differential coverage (P3/P10).

**Fast path:** convex×convex NFP via O(n+m) sorted-slope edge merge (no boolean). Convexity
predicate on **quantized** vertices: no negative i128 cross, zero-crosses (collinear triples)
merged; borderline-after-quantization shapes route deterministically to the boolean path. Fast-path
output passes through the same canonicalization as boolean output (§3.4).

**IFP (frame trick):** rectangle frame = container bbox expanded by `NFP_FRAME_MARGIN`, container
as its hole; outer NFP(frame, B⊕□E) via the same routine; the result's **holes are the IFP loops**.
Container keep-out holes (quality-0 zones) each contribute an ordinary obstacle NFP. Note the
inverted failure direction here (§2) — covered by P2/P6-concave.

### 3.3 Why not orbiting (unchanged; receipts in the research record)

SVGnest ships an explicit NFP-null failure path; libnfporb's README disowns its own robustness;
libnest2d abandoned Bennell–Song mid-implementation (its consumer packs convex hulls to this day).
Tolerance-driven f64 control flow is the worst determinism fit, and the region difference needs a
boolean engine anyway. **Closed.**

### 3.4 Canonicalization (every NFP from ANY path, before caching or consuming)

i_overlay's output ordering is deterministic today but **uncontractual**; the fast path has no
inherent convention either. Canonical form: (a) winding by exact i128 signed area (outer CCW, holes
CW); (b) each ring reduced to its **lexicographically-least rotation** (Booth's algorithm — plain
"start at the minimal vertex" is ill-defined for pinched rings that visit their minimal vertex
twice, a real output class of snap-rounded NonZero unions); (c) rings sorted by (min-vertex,
length, lexicographic compare); (d) zero-area rings dropped (exact test).

### 3.5 The `i_overlay` dependency (audited 2026-07-10)

Source-audited at 7.0.2: `#![no_std]`; zero std HashMap/HashSet in library code; zero floats in the
integer pipeline; float-API transcendentals via libm 0.2.16 (the exact version already in our
lock); rayon only behind the off-by-default `allow_multithreading` feature (CI asserts it stays out
of the resolved feature set); RNG in dev-deps only; `sort_unstable` on integer keys (equal-key
order can shift across toolchain bumps — covered by the pinned-toolchain re-bless policy). MIT OR
Apache-2.0 family; the engine behind geo's `BooleanOps`. Pure Rust, no build.rs → wheel-clean.

**Two contract items the review added:** (1) the `Solver`'s **`Precision` is pinned explicitly** —
its default escalating endpoint-snap (radius doubling per split round) can displace crossing
vertices by 2^(L/2) grid units on pathological inputs, which is why the §4 error budget is
*measured*, not assumed, and why N0 tests whether a non-escalating precision setting terminates on
convolution quad soups; (2) version bumps are re-bless events (like toolchain bumps); vendoring the
~34k-LOC family is the escalation path (precedent: `crates/geo/src/buffer/`). **MSRV floor rises
1.87 → 1.88** (docs + `rust-version`; the pinned 1.96 toolchain is unaffected).

---

## 4. The epsilon-contact policy

**Problem.** The CDE counts touching as collision (closed-interval edge predicate — verified;
protects hazard-scope semantics engine-wide and is not to be touched), and quantization/snap noise
straddles the true boundary. Unbacked-off exact-contact candidates would be rejected erratically.

**Mechanism — one inflation, both effects.** Inflate the **moving part's** integer footprint by the
L∞ square □E once per (type, rot) *before* all NFP/IFP computation. By Minkowski associativity and
□E's symmetry (and since −(B ⊕ □E) = (−B) ⊕ □E): every obstacle NFP inflates by E and the IFP
deflates by E, with unchanged cache keys and no per-region offset op.

**The error budget — stated as measured-not-proven** (the review dismantled the earlier "≤ 3g
proven" claim): identifiable additive terms are vertex quantization (≤ 0.71g per shape → ≤ 1.42g
after Minkowski), boolean crossing snap (≤ ~1g per boolean pass — one for the NFP union, one for
the region difference), cardinal swap/negate discrepancy (≤ 1g), off-grid fallback-pose
quantization (≤ 0.71g) — **≈ 3.6–4.6g worst case, PLUS i_overlay's escalating endpoint-snap, which
has no closed-form bound on adversarial inputs.** Therefore: (a) region queries are **one flat
multi-path union from pristine cached integer NFPs** — never an iterated re-union of snapped
output, killing the unbounded-drift path; (b) a **displacement-measurement property test** (region
boundary vs exact-rational reference over the seeded corpus) empirically sizes the real error, and
`NFP_BACKOFF_E` is set ≥ 4× the measured max (initial value 16; density cost of even E = 256 is
< 11 mil across a 120-unit row — far below harness resolution, so raising E is always available).

**Guarantees (conditional on the measured E):** candidates sit ≥ E − (measured error) inside the
true feasible set ⇒ `feasible_at` acceptance is structurally expected; the CDE remains the arbiter
and rejections advance deterministically to the next candidate, **counted** (§10 gates the counter
at 0 on the acceptance harness; elsewhere a nonzero count is a release-blocking investigation
trigger, not an automatic feature kill — the review's tail-event argument). Separation floor:
placed outlines ≥ min_sep apart exactly as today, plus the residual backoff — the consumer's
0.375-in gap / 0.5-in margin re-validation is strictly *more* satisfied. Exact-fit artifact:
narrowed (10×10 ×100 in a 100.001 container places 100), not eliminated (exactly-100.0 still loses
a column) — documented.

---

## 5. The placer

### 5.1 Region assembly (per part, per allowed rotation)

```
feasible(B@rot) = IFP(container, B⊕□E @rot)                    // cached per (type, rot); holes folded in
                ∖ ⋃ⱼ (NFP(placedⱼ, B⊕□E @rot) + d_transfⱼ.translation)   // ONE flat union per query
```

Per query: gather the pristine cached NFPs of all placed obstacles, translate each by its placed
pose (grid-exact for NFP-placed parts; quantized-once for fallback-placed parts), and run **one**
multi-path union + difference. (The earlier incremental snapped-accumulator design is rejected —
iterated snap-rounding has no additive error bound. If profiling ever wants incrementality back, it
must cache *pre-union piece lists*, not snapped results.) Cost control comes from the convex fast
path, the per-(type,rot) reuse of cached NFPs, and — if measured necessary — a bbox prefilter
(obstacles whose inflated-NFP bbox cannot intersect the current best-candidate half-plane).

### 5.2 Candidate selection (exact for our loss)

`LbfLoss = 10·x_max + y_max` is strictly linear in translation at fixed rotation ⇒ the minimum over
the region is at a **vertex of the boolean-difference output** (including boolean-created crossing
vertices; hole-loop vertices can never win but are harmless to include). Integer loss
`10·(X + bbox_x_max_int(rot)) + (Y + bbox_y_max_int(rot))`, argmin tie-break lexicographic
`(cost, rot_idx, X, Y)`. Walk ascending; first `feasible_at`-accepted pose is placed **directly via
`place_item` — NFP placements bypass `place_dropped` entirely** (the slide would burn 60–240 CDE
queries per placement re-deriving a contact the region already encodes, smear the deliberate E
backoff, and turn grid-exact poses into floats — three of this doc's claims died in review until
this was made explicit). Empty region, or all candidates rejected (counted) ⇒ **fall back to
today's sampling path verbatim** — `search::search` + `place_dropped` — for that part; fallback
parts keep the slide because they carry no NFP backoff.

### 5.3 Pipeline changes in NFP mode

- `constructive_fill` / refill: strategy-dispatched placer. **Exactly one refill pass** (compaction
  is skipped, so later rounds could differ only through sampling-fallback luck; today's
  `IMPROVE_ROUNDS = 3` loop degenerates to cost).
- **Slide compaction skipped**; `sep::run_separation` runs **unchanged** on the unplaced tail
  (post-separation NFP refill: recorded future option).
- **PRNG invariant, stated correctly** (the review killed the original "zero PRNG" claim): NFP
  construction consumes PRNG **only through the sampling fallback**, and the separation tail
  consumes it per start regardless — on over-subscribed inputs (the acceptance harness is 1.3×
  over-subscribed) canonical starts do **not** collapse; they diversify through the seeded tail
  exactly as today. Fallback triggering is itself a pure function of byte-deterministic integer
  regions + CDE verdicts, so cross-platform stream alignment is not at risk.
- **Multi-start policy: keep the proven block split as the default** (canonical block first,
  jittered block second — canonical starts still diversify via the seeded fallback + sep tail).
  The earlier "k=0 canonical, all k>0 jittered" proposal was reasoned from the false collapse
  premise and is structurally the configuration docs/02 §11.2 measured as a regression. N3 runs
  the A/B (block split vs all-jittered vs a candidate second axis — a seeded permutation of the
  candidate walk among the top-m tied-or-near candidates, integer + seeded + deterministic) on the
  over-subscribed harness and keeps the winner. **The NFP-mode per-seed floor is empirical** — the
  sampling-mode structural floor argument does not transfer; §10 requires the floor to hold on all
  acceptance seeds at the calibrated K.

---

## 6. Cache design & the import hoist

**The structural fix found in review:** `shape_cd` footprints exist only after import, and import
currently runs **inside `nest_core`, per start** — so "build the cache before `run_starts` forks"
was impossible as written. The plan is therefore: **hoist the import** — items + container imported
**once per public call**, `entities` (and the importer's decimation config) passed into `nest_core`
by reference. This refactor is placement-inert by construction (imports are deterministic and
identical per start today) and must be proven inert by the untouched goldens **in its own
preparatory PR** before any NFP code lands (N1a, §11). It also removes today's K× redundant import
work — a small unconditional win. The cache is then built once (ascending key order), passed as
`&NfpCache` (no interior mutability ⇒ `Sync`) through **both** `run_starts` builds into
`multistart_start` → `nest_core` — and through the multi-sheet chain
(`nest_multi_multistart_per_item` shares pair-NFPs across sheets; IFPs are per-sheet).

- **Key**: `(type_a: u16, rot_idx_a: u8, type_b: u16, rot_idx_b: u8)` in a `BTreeMap`. Mirrored
  entries **derived by point reflection at insert time, never recomputed** (recomputing the swapped
  direction is not bit-guaranteed to match — different seed translates + snap; deriving is exact
  and safe). Day-one storage for the mixed harness: 52 footprints ⇒ **1,378 stored pair entries**
  (= (2704 − 52)/2 + 52) + 52 IFPs — the review corrected the earlier 676 figure, which assumed the
  deferred relative-rotation reduction. ~1–4 MB typical.
- **CLAUDE.md amendment (goes with §8):** the sanctioned parallel exception's wording extends from
  "no shared state" to "no shared **mutable** state; the read-only `NfpCache` is built before the
  fork and crosses threads as `&NfpCache` (`Sync`)".
- **Budget gate:** cache build ≤ 5 % of a budget-500-equivalent nest on the mixed corpus (the
  consumer calls `nest()` several times per job).

---

## 7. API surface, end to end

**The committed decision (was an aside; the review made it load-bearing):** introduce a
**`NestConfig`** struct (strategy + seed/budget/restarts consolidation point for future knobs); the
existing 8 public fns are kept as **thin frozen wrappers** with byte-identical behavior. This is an
N2 deliverable with its own LOC line and an enumerated call-site inventory (golden_dump, tests,
4 examples, facade, py). **`crates/py` must compile at every phase boundary** — note explicitly:
`ci.yml` excludes `ironnest-py` from clippy/test, so a broken py crate is invisible until
`wheels.yml`; the phase checklists include a local `cargo check -p ironnest-py`.

| Layer | Change | Notes |
|---|---|---|
| `crates/optimizer` | `NestConfig` + `PlacementStrategy`; `nfp/` module (`mod.rs` placer+region, `minkowski.rs`, `cache.rs`); import hoist (§6); **error channel**: the strategy-bearing entry points return `Result` (input-domain hard error); sampling-mode signatures untouched | `multistart_start` (doc-hidden test hook) and `tests/nest.rs` + `bin/golden_dump.rs` call sites update in lockstep — mechanical, output-inert, listed in the N2 diff inventory |
| `crates/ironnest` | re-export `NestConfig` + new entry | |
| `crates/py` | `strategy="sampling"` kwarg on **both** pyfunctions (`nest`, `nest_multi`); unknown value or domain error → `ValueError` | default byte-identical |
| consumer | `IronNester(strategy=…)`; append-only audit key `"placement_mode"` (only when non-default); **version-floor precedent** (`restarts=` — a new kwarg TypeErrors on old wheels; the functional-probe pattern is for input-shape changes, not kwargs) | follow-on `drawing_and_gcode` issue |
| `budget` semantics | drives **only** the sampling fallback and refill; the separation tail runs on its own fixed internal budgets (unchanged — the earlier wording was wrong); recorded as-is in the sidecar | |

---

## 8. Determinism-rules addendum (paste into CLAUDE.md when N1 lands)

> **Integer-NFP rules (feature #4):**
> 1. **One global grid.** `NFP_SCALE = 2^20`, `IntCoord = i64`. Never derive scale from input;
>    never quantize twice.
> 2. **Exactly two quantization sites**: shape vertices at cache build (from the same f64 collision
>    shapes the CDE uses) and off-grid obstacle poses (once per pose). No other float↔int
>    conversion exists in the NFP module.
> 3. **Cardinal rotations are exact integer swap/negate** on the quantized 0° footprint;
>    non-cardinal go libm-rotate → quantize. Cache keys use rotation **indices**, never floats.
> 4. **No float between quantization and unscaling.** Every self-written predicate accumulates in
>    **i128**. Candidate loss is integer `10·X + Y`, tie-break lexicographic `(cost, rot_idx, X, Y)`.
> 5. **iOverlay i64 engine only, with `Solver` strategy and `Precision` pinned**;
>    `allow_multithreading` stays off forever (CI-asserted); iShape family bumps are re-bless events.
> 6. **Canonicalize every NFP from any path** (winding by exact area; rings to lexicographically-
>    least rotation via Booth; rings sorted; zero-area dropped). Mirrored cache entries are derived
>    by negation, never recomputed.
> 7. **The CDE stays the feasibility arbiter**: every chosen pose passes `feasible_at`; rejection ⇒
>    deterministic next candidate, counted. Region queries are one flat union from pristine cached
>    NFPs — never re-unions of snapped output.
> 8. NFP mode validates the **derived worst-case boolean-op coordinate** (container + 2·part + E +
>    frame margin) against the engine bound with explicit headroom, and hard-errors otherwise.
>    `NFP_BACKOFF_E` is a fixed integer sized ≥ 4× the *measured* boundary-displacement bound.
> 9. **Parallel-exception amendment**: no shared **mutable** state; the read-only `NfpCache` is
>    built before the fork and crosses threads as `&NfpCache` (`Sync`).

---

## 9. Validation

### 9.1 Property tests (`tests/nfp_props.rs`; seeded PCG64 only)

- **P1 Soundness**: every region vertex + seeded interior points ⇒ `feasible_at` accepts. Failure
  attribution: E vs the *measured* displacement bound (§4), not a paper constant.
- **P2 Completeness**: CDE-accepted seeded poses lie in the region dilated by □(E + measured
  bound + 4). **Run explicitly on concave containers and holed containers** — this is the test
  that catches the inverted IFP failure direction (§2).
- **P3 Pair ground truth**: point-in-NFP ⇔ shapes overlap at that offset (boundary band exempt),
  convex and concave seeded pairs.
- **P4 Convex reference oracle — run twice**: once against the production fast path, and once with
  the fast path force-disabled so the **convolution union-fill** is checked against the exact
  edge-merge oracle on convex pairs (otherwise the code with the completeness risk gets zero exact
  coverage — review finding).
- **P5 Symmetry/rotation identities — scoped**: exact on the fast path; equality-modulo-snap
  (symmetric-difference area ≤ tolerance) on the boolean path; the shipped cache *derives* mirrored
  entries, so P5 is a diagnostic, not a shipping invariant.
- **P6 IFP exactness**: rectangles (exact bbox-shrink oracle) **and concave L-containers**
  (seeded CDE-verified interior poses must lie in the extracted IFP).
- **P7 Monotonicity + convex area inequality.** **P8 Loss-vertex optimality + tie determinism.**
  **P9 Degenerate geometry** (exact-fit slivers, collinear/duplicate vertices, pinched rings,
  holes touching boundaries, part == container).
- **P11 Displacement measurement** (new, load-bearing): max region-boundary displacement vs an
  exact-rational reference over the corpus → sizes E (§4).
- **P10 (dev-side, per release)**: shapely/GEOS + pyclipper differentials.
- Round-trip ≤ ½g; swap/negate vs libm-rotate ≤ **1g** (knife-edge corrected).

### 9.2 Golden additions (append-only; 3 OS × debug/release/parallel)

N2: `nfp-concave-pocket` · `nfp-l-sheet-hole` · `nfp-min-sep` · `nfp-decimated-circle` ·
`nfp-contact-columns` (10 columns of 10×10 in a 100.01-wide sheet — the density signature AND the
ε-policy sentinel). N3 (with the policy it exercises): `nfp-multistart` (restarts=3,
jittered-start winner, run under `--features parallel` too — landing it in N2 would guarantee an
intra-feature re-bless when N3's policy lands; review caught the phase mismatch). Feasibility is
asserted via `debug_assert!(layout.is_feasible())` **inside `nest_core`'s NFP path** (the dump
binary only sees `NestSolution` — the earlier "assert in golden_dump" had no access path); CI's
debug leg executes it. Optionally golden 2–3 canonicalized integer NFP vertex lists (localizes any
x-platform divergence to construction vs search).

### 9.3 ESICUP corpus (context + regression tracking — never the gate)

Commit the 13 discrete-rotation academic instances **re-derived from the CC0 originals at
`github.com/ESICUP/datasets`** (using sparrow's MIT-licensed jagua-JSON conversions requires
carrying its notice — the provenance README states whichever source is actually used; review
caught the mismatch), plus `examples/esicup.rs` (deterministic fixed-iteration bisection on strip
length; `swim` pre-scaled ×1/16 for the input domain). **`package.exclude` the corpus** in the
owning crate's Cargo.toml so wheels/sdists don't ship it; note the data-licensing caveat against
CLAUDE.md's "MPL uniform" line in the README. sparrow's numbers (20 min × 100 runs × 16 cores) are
context, not a bar.

### 9.4 Perf gates

- `examples/bench.rs`: `nfp-cache mixed-13` (**1,378-entry** build), `nfp-place mixed-13`
  (single-start end-to-end **with a construction-vs-separation time split** — the number §10
  actually gates), `nfp-union stress` (100 × 48-gon discs), existing probes in NFP mode.
- **Deterministic op-count ceilings in `cargo test`** (boolean ops, quantized vertices, candidates
  evaluated, CDE verifies + rejections) — CI-safe, double as determinism probes.
- Consumer vertex-scaling table re-published honestly (NFP cost is vertex-dependent, bounded by
  decimation).

---

## 10. Acceptance protocol & rollout

**The framing correction (review, critical):** separation is **97–99 % of wall-clock** on
unplaced-tail nests (docs/02 §11.1) and the acceptance corpus always has a tail — so NFP
construction time is *additive* unless NFP construction *substitutes* separation-tail work by
placing more parts constructively. The protocol therefore gates two separate numbers:

1. **Per-start construction budget:** NFP construction (cache amortized) ≤ **1 s/start** on the
   mixed harness — measured at N2 exit from the bench split, falsifiable immediately, replacing
   the old un-decomposable "≤ 1.1× total" arm as the engineering gate.
2. **Sep-substitution hypothesis, stated with a number:** NFP construction must place enough more
   parts per start that total wall-clock at the calibrated K stays ≤ **1.1×** the incumbent
   (sampling, restarts=16, budget=500, cardinal). Measured at N4 on tuning seeds 101–104; K is the
   calibration knob; the earlier "≥ 30 % less wall-clock" SHIP branch is **deleted** (arithmetically
   unreachable while the sep tail dominates).

**Prerequisite evidence for the density bar (review: the +1.0 pp bar is currently
under-justified):** N0 measures (a) the share of non-axis-reachable pocket contacts in existing
harness layouts (replayable from saved placements), and (b) a throwaway end-to-end NFP probe on the
pentagon/bricks/mixed probes. If N0 cannot show a mechanism for ≥ +1.0 pp, the feature's primary
claim is **re-framed before N1** (consistency + pentagon-class + per-placement cost, with a lower
density bar agreed with the consumer) or the build stops — that is what the spike is for.

| Verdict (seeds 1–8, paired deltas) | Criteria |
|---|---|
| **GO** (default-flip candidate) | mean ≥ 84.5 % at ≤ 1.1× wall-clock; per-seed floor ≥ 82.76 % (empirical in NFP mode — §5.3); rejection counter == 0 **on the acceptance harness** |
| **SHIP OPT-IN** | mean +0.5–1.0 pp at ≤ 1.1× clock, floor holds |
| **NO-GO** | any seed < 82.76 %, or gain < 0.5 pp, or unexplained rejections |

Rejections elsewhere (ESICUP, property fuzzing) are release-blocking investigation triggers with a
≤ 1e-6 rate bound + root-cause analysis — not an automatic kill (tail-event realism; review).

**Architectural-ceiling proofs** (in-repo): pentagon ≥ 24/40 **at N0/N1 exit** (the cheapest
falsifier of the whole thesis — front-loaded by review); 2-right-triangle interlock by construction
alone; exact-fit columns at 100.01.

**Rollout**: opt-in default-off (existing goldens byte-identical in the landing PR); default flip
no earlier than 0.6.0, coordinated with a consumer pin bump; gated on GO + zero acceptance-harness
rejections + two consecutive releases of green x-platform NFP goldens + no ESICUP instance losing
> 0.5 pp + `ManualNest.violations == 0` over a ≥ 50-nest consumer replay + perf gates. **Sampling
mode is kept indefinitely** (escape hatch, differential oracle, per-part fallback).

---

## 11. Build plan (revised for honest scope; goldens green at every step)

| Phase | Scope | Exit criteria |
|---|---|---|
| **N0 — spike (2–4 days, throwaway)** | i_overlay pinned; quantize + orientation-normalized convolution + one hacked single-start placement loop (no cache/canon/mode-switch polish); boundary-magnitude engine-bound test; Precision termination test on quad soups; pocket-contact share measurement on saved harness layouts | **pentagon ≥ 24 by construction** (kill criterion); construction ≤ ~1 s/start extrapolated; engine bound + precision behavior confirmed; density mechanism evidence for §10 |
| **N1a — import hoist (prep PR)** | items/container imported once per public call; `entities` passed into `nest_core` | goldens byte-identical (the proof of inertness); no NFP code |
| **N1 — geometry substrate** | `nfp/minkowski.rs`, `nfp/cache.rs`; convex fast path; normalized union-fill; frame IFP; □E; Booth canonicalization; P1–P9 + P11 displacement measurement → E sized | property tests green on 3 platforms; goldens untouched |
| **N2 — placer + NestConfig** | region assembly (flat unions); vertex/loss/tie-break; verify + fallback (search+place_dropped verbatim); **place_dropped bypass on NFP placements**; `NestConfig` + 8 frozen wrappers + error channel; single refill pass; 5 golden cases; bench construction/sep split; diff inventory incl. `multistart_start` hook + golden_dump + tests lockstep; `cargo check -p ironnest-py` | old goldens byte-identical, new blessed on 3 OS; interlock by construction; construction ≤ 1 s/start on the harness |
| **N3 — multi-start & multi-sheet** | shared read-only cache through both `run_starts` builds + multi-sheet chain; policy A/B (block split default vs alternatives, over-subscribed harness); `nfp-multistart` golden (seq + parallel); cache ≤ 5 % gate | parallel == sequential byte-identity; policy chosen on data; floor measured |
| **N4 — wheel + consumer + acceptance** | py kwarg (both pyfunctions) + facade; consumer adapter + audit key + version floor; ESICUP corpus + runner (+ package.exclude); §10 protocol on tuning then acceptance seeds | verdict recorded here with numbers |

### 11.1 N0 results (2026-07-10, `examples/nfp_spike.rs`) — **VERDICT: GO to N1a/N1**

| probe | NFP construction (single start, no sep, no PRNG) | incumbent reference |
|---|---|---|
| pentagon ×40 | 19 placed (65.4 %) · **3.6 ms** | sampling *construction+improve*: ~65 % (same basin); +sep: 23 |
| 13×7 bricks ×120 | 99 placed (90.1 %) · **21 ms** | sampling construction: ~87 %; full pipeline +sep: 101 @ ~2,100 ms |
| 2 right-tris → square | **2/2 by construction** · 1.4 ms | sampling needs the full GLS separation search |
| 10×10 ×100 in 100.01 | **100/100 (100.0 %)** · 5.1 ms | sampling: 81 (the exact-fit artifact) — killed by construction |
| **mixed-13 consumer corpus** (concave L-brackets, 48-gon discs, min_sep 0.375) | **51/122, 79.7 % · 937 ms** | full K=1 pipeline (constr+improve+sep): 80.3–82.8 % @ **150–214 s** |

Reading: (a) the **kill criterion as originally phrased compared the wrong slices** — construction-
alone vs construction+separation; on the fair comparison NFP construction beats sampling
construction everywhere it differs (bricks +3 pp, contact-columns +19 placed, interlock without
sep) and lands ~1–3 pp under the *full* K=1 pipeline at **~1/200th the wall-clock** on the real
corpus. The pentagon's 19 = sampling's construction basin; its 23 needs the sep tail, which N2
keeps. (b) The **per-start construction budget gate (≤ 1 s) holds** (937 ms on mixed-13, spike
quality — no incremental union caching yet). (c) The **sep-substitution hypothesis has favorable
evidence**: sep will start from ~80 % instead of sampling-construction's much lower base.
(d) **ε-policy validated**: 0 verify-rejections on all convex probes; 6 (of thousands of candidate
verifies) on the concave+decimated mixed corpus — safe by design (next-candidate), and exactly the
N1 error-budget homework (P11 displacement measurement → E sizing). (e) Two hard-won implementation
lessons encoded for N1: hole cycles must never be CCW-normalized into seed pieces, and the
translated *outer* seed of a holed shape floods its holes — for holed A either omit the A-seed
(safe: ring thinner than any part ⇒ no poses lost) or do winding-correct shape-group seeding.

### 11.2 N1a + N1 results (2026-07-10, landed)

- **N1a import hoist**: `Problem` struct + `import_problem()` (lib.rs); items/container imported
  once per public call, shared read-only (`&Problem`, no interior mutability) across the K starts
  through `multistart_start_imported` — the sequential/parallel single-definition wiring survives.
  **Goldens byte-identical** (the inertness proof) on default and `parallel` builds. Side win: K×
  fewer imports per multistart call.
- **N1 substrate** (`nfp/geom.rs`, `nfp/minkowski.rs`, `nfp/mod.rs` — doc-hidden pub test hook):
  quantize/unquantize (2^20 grid, i64), i128 predicates, CCW normalization, collinear strip,
  Booth-restricted canonical rotation + full shape canonicalization, convex hull, exact O(n+m)
  convex edge-merge Minkowski, orientation-normalized convolution union-fill (solid-cycle seeds),
  □E inflation (convex fast path / union-fill), **quads-only frame IFP with exact face
  classification** (see below), zone folding, eager pair cache with point-reflection mirror
  derivation, domain gate (`NfpError::InputDomain`, ≥ 2^61 hard error), region query as one flat
  difference, integer candidate walk with the `(cost, x, y)` tie-break.
- **P1 caught a real design bug before it shipped** (the property suite earning its keep): the
  original frame-trick seed rules leaked concave-container pockets into the IFP (an L-container's
  notch appeared feasible; the "ring thinner than any part" A-seed-omission argument is false for
  concave — and even non-rectangular convex — containers). **Fix: quads-only frame sum + exact face
  classification** — edge quads alone cover every boundary-touching pose, so each hole of their
  union is a constant-status face, classified by one exact integer point-in-container test of a
  deterministic interior probe (vertex-diagonal midpoint ladder), with nested forbidden islands
  subtracted. Seed-free, all-integer, correct for any simple container; docs §3.2's IFP paragraph
  is superseded accordingly.
- Tests green: 12 substrate unit tests + 5 integration property tests (P1 soundness zero-tolerance
  across {rect, L-container} × {min_sep 0, 0.375} × {convex, concave parts}; P2-lite concave IFP
  completeness; P8 double-build byte-identity; cardinal swap/negate ≤ 1 grid vs libm; domain gate)
  + all pre-existing suites; goldens untouched; clippy `-D warnings` clean on both feature sets.

### 11.3 N2 results (2026-07-10, landed)

**Shipped:** `PlacementStrategy { Sampling, Nfp }` + `NestConfig` + `NestError` +
`nest_with_config` (the strategy-bearing `Result` entry point; `Sampling` config is the frozen
wrapper — locked byte-for-byte against `nest_multistart_per_item` by test). `nfp/place.rs`: the
candidate-walk placer (exact `(cost, rot_idx, x, y)` tie-break, `feasible_at` verification, direct
`place_item` — **no slide** on NFP placements; sampling fallback = `search` + `place_dropped`
verbatim with the settled pose quantized once as an obstacle), construction + exactly one refill
pass, `debug_assert!(is_feasible)` on the NFP path. Strategy + the once-per-call `NfpCache` thread
through `run_starts_with` into both sequential and `parallel` builds (immutable `&NfpCache` across
the sanctioned threads). Sampling paths byte-frozen: **all pre-existing goldens unchanged.**

**Goldens (5 new cases blessed; the §9.2 set minus `nfp-multistart`, which lands with its N3
policy):** `nfp-contact-columns` (3×3 grid at exactly `E = 16/2^20` wall clearance — the artifact
killer, visible in the snapshot bytes), `nfp-concave-pocket` (a square placed INSIDE the C-part's
pocket by construction — the NFP-hole path), `nfp-l-sheet-hole`, `nfp-min-sep` (13.375 brick pitch
exactly), `nfp-decimated-circle`.

**Full-pipeline bench (NFP construction + sep tail vs sampling, same seed/budget, min_sep 0
synthetics):** pentagon K=1 23/40 @ 2.7 s (par with sampling's 23 @ 3.0 s — the sep tail is the
binding constraint on this shape, as §10's model predicted); **bricks K=1 103/120 = 93.7 % vs
sampling's 101/120 = 91.9 % (+1.8 pp at equal K)**; bricks K=8 105 par; mixed 4-type K=8 93.2 %
par, K=16 94.2 %. The min_sep-0 synthetics understate the mode (sep dominates and the □E backoff
concedes exact-contact-only wins); the decisive §10 acceptance run on the consumer corpus
(min_sep 0.375) is N4, after N3's multistart policy A/B.

### 11.4 N3 results (2026-07-10, landed)

- **Multistart policy A/B (settles §13 Q4 with data):** on the over-subscribed mixed 4-type corpus
  at K=8 over 5 seeds in NFP mode, the shipped **block split wins** — 94.01 % mean vs 93.49 % for
  all-jittered-k>0 (`examples/nfp_policy_ab.rs`, result recorded in its header). The corrected
  §5.3 premise holds: canonical starts diversify through the sampling-fallback + separation-tail
  PRNG streams. `start_order_policy` stays strategy-independent.
- **Multi-sheet:** `NfpCache` split into the container-independent **`PartsGeometry`** (footprints,
  □E movers, loss offsets, pairwise table; its own parts-side domain gate) shared via `Arc` across
  sheets, and the per-sheet IFP assemble (container-side gate + frame margin + zone folding).
  `nest_multi_with_config` lands with the spill semantics of `nest_multi_multistart_per_item`
  (Sampling config = frozen wrapper, test-locked; NFP spill + determinism test-covered).
- **`nfp-multistart` golden blessed:** overflow corpus at seed 3, K=3 — the K=3 result differs
  from BOTH canonical starts (placed areas 673 vs 646), so the winner is the trailing jittered
  start; the case pins the NFP multistart path (shared read-only cache, block policy, jitter) into
  the byte-identity contract, including under `--features parallel` (CI re-runs the same snapshot).
- Suites: 75/76 green (release default/parallel) + workspace debug; clippy `-D warnings` clean.

### 11.5 N4 (2026-07-10, in progress)

- **Facade + wheel landed:** `crates/ironnest` re-exports the config API;
  `crates/py` gains `strategy="sampling"` (default, byte-frozen) / `"nfp"` on BOTH pyfunctions,
  loud `ValueError` on unknown values and on `NestError` (the domain gate), dispatched inside the
  GIL-released solve. Wheel builds; kwarg + error surface smoke-tested through the consumer venv.
- **Consumer adapter integration** (IronNester `strategy=` + the append-only `placement_mode`
  audit key + version floor) is tracked as a `drawing_and_gcode` follow-up — that repo currently
  has unrelated uncommitted work on another branch, so the acceptance measurement runs through the
  RAW wheel API with identical inputs for both strategies (paired deltas; absolute utils differ
  slightly from the adapter's 83.44 baseline because the raw path skips edge-margin erosion).
- **ESICUP corpus + runner: deferred** (context/regression tracker, never the gate — §9.3); the
  acceptance verdict below is the gate.
- **Acceptance run (2026-07-10, raw wheel API, consumer 13-shape corpus, 120×60, min_sep 0.375,
  cardinal, budget 500, K=16, paired seeds 1–4): VERDICT = SHIP OPT-IN.**

  | seed | sampling K=16 | NFP K=16 | Δ |
  |---|---|---|---|
  | 1 | 83.22 % @ 296 s | **84.03 %** @ 339 s | +0.81 |
  | 2 | 83.22 % @ 332 s | **84.03 %** @ 285 s | +0.81 |
  | 3 | 83.22 % @ 255 s | **84.03 %** @ 273 s | +0.81 |
  | 4 | 83.42 % @ 278 s | 83.42 % @ 285 s | ±0 |
  | mean | 83.27 % | **83.88 %** | **+0.61 pp** |

  Floor: NFP ≥ sampling on every seed. Wall-clock: 1.02× mean (inside the ≤1.1× arm). 84.03 % is
  the **highest utilization ever recorded on this corpus** (previous all-time: 83.66 %). Against
  §10: +0.61 pp at equal clock with the floor held = **SHIP OPT-IN** (GO wanted ≥ +1.0 pp mean).
  Cache-build gate: 1.04 s for the full 13-type build incl. import ≈ 0.35 % of a K=16 nest —
  passes. GO remains reachable via the recorded levers: the post-separation NFP refill (§13 Q5),
  higher K (NFP's ceiling has not saturated the way sampling's has), and surfacing the
  verify-rejection counter through the wheel for the zero-rejection criterion.

**Estimated scope (revised ~2× upward by review): ~1,600–2,200 LOC feature + ~1,200–1,800 LOC
tests/examples/tooling.** Itemized: convolution + normalization 250–350; fast path + convexity
150–200; IFP/region 150–200; cache/canonicalization/Booth 250–300; placer + verify + fallback
150–200; import hoist + NestConfig + threading through 8 wrappers + facade + py + error channel
400–600; counters/CI assertions 100; goldens ~150; esicup runner ~200; bench/density additions
~120; property harness + generators + oracles 600–900. Dependencies: the i_overlay family (5
crates, exact-pinned). Decision point: this is a **4–6 week** feature at normal pace; the N0 spike
(2–4 days) buys the density evidence before committing.

---

## 12. Risks

| Risk | Sev | Mitigation |
|---|---|---|
| Density thesis fails (slide already captures most contacts; sparrow does SOTA without NFP) | **HIGH — the headline risk** | N0 kill criterion (pentagon ≥ 24 + pocket-contact share); reframe-or-stop gate before N1 (§10) |
| Sep-tail dominance makes equal-wall-clock unreachable | HIGH | per-start construction budget gate; sep-substitution hypothesis measured at N2/N4; K calibration |
| i_overlay hazard / version churn / escalating snap | MED | audit done; exact pin; Precision pinned; displacement measurement sizes E; canonicalization; vendoring path |
| Union-fill completeness on **holed** inputs (proof covers hole-free) | LOW-MED | orientation normalization (review fix) + P3/P10 differentials; CDE rejects escapes on the obstacle side; P2/P6-concave covers the IFP side |
| IFP failure direction is silent (no rejection signal) | MED | P2/P6 on concave/holed containers; fallback backstop; §2 states it honestly |
| NFP-mode multi-start diversity/floor | MED | block-split default (proven); N3 A/B; empirical floor requirement in §10 |
| Import-hoist refactor perturbs behavior | LOW | own PR, goldens as proof (N1a) |
| Cache build cost per consumer call | MED | ≤ 5 % gate; pair-NFP sharing; measured at N3 |
| Exact-fit/measure-zero placements dropped | LOW | narrowed not eliminated; documented; sampling mode unchanged |
| MSRV/licensing/packaging (1.88; 5 MIT crates; CC0 data; sdist bloat) | LOW | docs + deny allowlist + provenance README + package.exclude (N1/N4 checklists) |

## 13. Open questions (tracked; none block N0)

1. N0's construction-cost measurement (sizes K at equal cost; decides the sep-substitution target).
2. Pocket-contact share on real corpora (the density mechanism evidence — N0).
3. Whether a non-escalating i_overlay `Precision` terminates on convolution quad soups (N0; decides
   E sizing pressure).
4. NFP-mode start-policy winner (N3 A/B) and the empirical floor at calibrated K.
5. Post-separation NFP refill; separation search consuming the NFP cache for exact directional
   penetration (docs/02 §7.2) — recorded, out of scope.
6. Input-domain policy for metric consumers (explicit scale API param) — on demand.

## References

Boost.Polygon `convolve_two_polygon_sets` (tutorial `minkowski.cpp`); Deepnest `minkowski.cc` +
`background.js` (clipCache, frame trick); iShape-Rust `i_overlay` 7.0.2 family (audited
2026-07-10; `split/cross_solver.rs`, `core/solver.rs` Precision, `segm/boolean.rs` winding);
CGAL Minkowski_sum_2 manual; Rocha 2019 (arXiv 1903.11139); Burke et al. 2007; Bennell & Song 2008;
libnest2d `geometry_traits_nfp.hpp`; sparrow (arXiv 2509.13329) Tables 2–3;
`github.com/ESICUP/datasets` (CC0); docs/02 §11.1–11.2 (B3 negative result; saturation +
acceptance baselines 82.76 / 83.44 / 83.66 %).
