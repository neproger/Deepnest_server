#pragma once

#include <vector>

namespace nest {

/// Minkowski convolution (NFP computation) using the Boost.Polygon edge-pair
/// algorithm — the same engine used by deepnest-next's node-calculateNFP and the
/// repo's minkowski.dll. The implementation lives in MinkowskiConvolution.cpp so
/// the (heavy) Boost.Polygon headers stay out of every translation unit that only
/// needs the result type. Stateless / reentrant: safe to call from worker threads.
class MinkowskiConvolution {
public:
    // One material region: an outer ring plus its own holes.
    struct Region {
        std::vector<double> outer;                    // x0,y0,x1,y1,... (real coords)
        std::vector<std::vector<double>> holes;        // each: x0,y0,...
    };

    struct Result {
        std::vector<std::vector<double>> outerPaths;  // each: x0,y0,x1,y1,... (real coords)
        std::vector<std::vector<double>> holes;       // each: x0,y0,x1,y1,... (real coords)
    };

    /// Fixed scale used by callers (e.g. getInnerNfp) for kScale-precision Clipper
    /// int64 ops. The convolution itself uses an internal dynamic scale (see .cpp).
    static constexpr double kScale = 10000000.0;

    /// NFP of polygon set A against polygon set B, i.e. Minkowski sum A ⊕ (−B),
    /// referenced to B region 0's first vertex. A and B may each hold several regions
    /// (a rigid part's separate / nested bodies). The convolution is computed per
    /// (A-region, B-region) pair and unioned, which preserves nested forbidden zones
    /// (a solid ring inside another ring's hole) that a single polygon_set merges away.
    static Result compute(
        const std::vector<Region>& A,
        const std::vector<Region>& B);
};

} // namespace nest
