// Node-API bridge to the ironnest placement oracle.
//
// The whole surface is JSON-in / JSON-out on purpose: it keeps the ABI tiny and the
// binding layer free of geometry types, while the actual solve (the expensive part)
// runs in-process with no serialization boundary around it.

use ironnest::{
    nest_multi_with_config, nest_with_config, NestConfig, NestError, PlacementStrategy,
    SeparationEffort, Sheet,
};
use napi::bindgen_prelude::Result;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{json, Value};

fn default_budget() -> u64 {
    1000
}
fn default_restarts() -> usize {
    1
}
fn default_column_weight() -> u32 {
    10
}

#[derive(Deserialize)]
struct NestRequest {
    items: Vec<Vec<[f64; 2]>>,
    qty: Vec<usize>,
    container: Vec<[f64; 2]>,
    #[serde(default)]
    holes: Vec<Vec<[f64; 2]>>,
    #[serde(default)]
    min_sep: f64,
    #[serde(default)]
    rotations: Vec<Vec<f64>>,
    #[serde(default)]
    seed: u64,
    #[serde(default = "default_budget")]
    budget: u64,
    #[serde(default = "default_restarts")]
    restarts: usize,
    #[serde(default)]
    strategy: Option<String>,
    #[serde(default)]
    separation_effort: Option<String>,
    #[serde(default = "default_column_weight")]
    column_weight: u32,
}

#[derive(Deserialize)]
struct SheetRequest {
    outline: Vec<[f64; 2]>,
    #[serde(default)]
    holes: Vec<Vec<[f64; 2]>>,
}

#[derive(Deserialize)]
struct NestMultiRequest {
    items: Vec<Vec<[f64; 2]>>,
    qty: Vec<usize>,
    sheets: Vec<SheetRequest>,
    #[serde(default)]
    min_sep: f64,
    #[serde(default)]
    rotations: Vec<Vec<f64>>,
    #[serde(default)]
    seed: u64,
    #[serde(default = "default_budget")]
    budget: u64,
    #[serde(default = "default_restarts")]
    restarts: usize,
    #[serde(default)]
    strategy: Option<String>,
    #[serde(default)]
    separation_effort: Option<String>,
    #[serde(default = "default_column_weight")]
    column_weight: u32,
}

fn normalize_rotations(
    rotations: &[Vec<f64>],
    items: usize,
) -> std::result::Result<Vec<Vec<f64>>, String> {
    if rotations.is_empty() {
        return Ok(vec![vec![0.0]; items]);
    }
    if rotations.len() == 1 {
        return Ok(vec![rotations[0].clone(); items]);
    }
    if rotations.len() == items {
        return Ok(rotations.to_vec());
    }
    Err(format!(
        "rotations must be empty, a single set, or one set per item ({items}); got {}",
        rotations.len()
    ))
}

fn parse_strategy(value: &Option<String>) -> std::result::Result<PlacementStrategy, String> {
    match value.as_deref() {
        None | Some("sampling") => Ok(PlacementStrategy::Sampling),
        Some("nfp") => Ok(PlacementStrategy::Nfp),
        Some(other) => Err(format!(
            "unknown strategy '{other}' (expected \"sampling\" or \"nfp\")"
        )),
    }
}

fn parse_effort(value: &Option<String>) -> std::result::Result<SeparationEffort, String> {
    match value.as_deref() {
        None | Some("full") => Ok(SeparationEffort::Full),
        Some("fast") => Ok(SeparationEffort::Fast),
        Some("max") => Ok(SeparationEffort::Max),
        Some("off") => Ok(SeparationEffort::Off),
        Some(other) => Err(format!(
            "unknown separation_effort '{other}' (expected \"full\", \"fast\", \"max\" or \"off\")"
        )),
    }
}

struct ConfigParts {
    min_sep: f64,
    seed: u64,
    budget: u64,
    restarts: usize,
    strategy: Option<String>,
    separation_effort: Option<String>,
    column_weight: u32,
}

fn build_config(parts: ConfigParts) -> std::result::Result<NestConfig, String> {
    Ok(NestConfig {
        min_sep: parts.min_sep,
        seed: parts.seed,
        budget: parts.budget,
        restarts: parts.restarts.max(1),
        strategy: parse_strategy(&parts.strategy)?,
        separation_effort: parse_effort(&parts.separation_effort)?,
        column_weight: parts.column_weight,
    })
}

fn nest_error(error: NestError) -> napi::Error {
    napi::Error::from_reason(error.to_string())
}

fn placement_json(p: &ironnest::Placement) -> Value {
    json!({ "item": p.item, "x": p.x, "y": p.y, "rotation": p.rotation_deg })
}

/// Nest item types into a single irregular container.
///
/// `request_json`:
/// ```json
/// {
///   "items":     [ [[x,y], ...], ... ],   // one outline per part type (item-local coords)
///   "qty":       [n, ...],
///   "container": [[x,y], ...],
///   "holes":     [ [[x,y], ...], ... ],   // optional keep-out zones
///   "min_sep":   0.0,
///   "rotations": [ [0,90,180,270] ],      // [] = no rotation; 1 set = shared; else per item
///   "seed": 1, "budget": 1000, "restarts": 1,
///   "strategy": "sampling" | "nfp",
///   "separation_effort": "full" | "fast" | "max",
///   "column_weight": 10
/// }
/// ```
/// Returns `{ "placements": [{item,x,y,rotation}], "unplaced": [item, ...] }`.
#[napi]
pub fn nest(request_json: String) -> Result<String> {
    let request: NestRequest = serde_json::from_str(&request_json)
        .map_err(|e| napi::Error::from_reason(format!("invalid request json: {e}")))?;
    let rotations = normalize_rotations(&request.rotations, request.items.len())
        .map_err(napi::Error::from_reason)?;
    let config = build_config(ConfigParts {
        min_sep: request.min_sep,
        seed: request.seed,
        budget: request.budget,
        restarts: request.restarts,
        strategy: request.strategy,
        separation_effort: request.separation_effort,
        column_weight: request.column_weight,
    })
    .map_err(napi::Error::from_reason)?;

    let solution = nest_with_config(
        &request.items,
        &request.qty,
        &request.container,
        &request.holes,
        &rotations,
        &config,
    )
    .map_err(nest_error)?;

    let placements: Vec<Value> = solution.placements.iter().map(placement_json).collect();
    Ok(json!({ "placements": placements, "unplaced": solution.unplaced }).to_string())
}

/// Nest item types across several sheets in order, spilling leftovers to the next.
///
/// Same fields as [`nest`] except `container` is replaced by
/// `"sheets": [ { "outline": [...], "holes": [...] }, ... ]`.
/// Returns `{ "per_sheet": [ [ {item,x,y,rotation}, ... ], ... ], "unplaced": [...] }`.
#[napi]
pub fn nest_multi(request_json: String) -> Result<String> {
    let request: NestMultiRequest = serde_json::from_str(&request_json)
        .map_err(|e| napi::Error::from_reason(format!("invalid request json: {e}")))?;
    let rotations = normalize_rotations(&request.rotations, request.items.len())
        .map_err(napi::Error::from_reason)?;
    let config = build_config(ConfigParts {
        min_sep: request.min_sep,
        seed: request.seed,
        budget: request.budget,
        restarts: request.restarts,
        strategy: request.strategy,
        separation_effort: request.separation_effort,
        column_weight: request.column_weight,
    })
    .map_err(napi::Error::from_reason)?;

    let sheets: Vec<Sheet> = request
        .sheets
        .iter()
        .map(|s| Sheet {
            outline: s.outline.clone(),
            holes: s.holes.clone(),
        })
        .collect();

    let solution = nest_multi_with_config(
        &request.items,
        &request.qty,
        &sheets,
        &rotations,
        &config,
    )
    .map_err(nest_error)?;

    let per_sheet: Vec<Value> = solution
        .per_sheet
        .iter()
        .map(|placements| {
            Value::Array(placements.iter().map(placement_json).collect())
        })
        .collect();
    Ok(json!({ "per_sheet": per_sheet, "unplaced": solution.unplaced }).to_string())
}
