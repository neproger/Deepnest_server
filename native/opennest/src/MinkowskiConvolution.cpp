#include "MinkowskiConvolution.h"

#include <boost/polygon/polygon.hpp>
#include <vector>
#include <cmath>
#include <limits>

// Boost.Polygon pulls in <windows.h>-style min/max on some toolchains; defend
// against the macros so (std::min)/(std::max) below are well-formed.
#undef min
#undef max

namespace nest {

namespace {

using bpoint      = boost::polygon::point_data<int>;
using polygon_set = boost::polygon::polygon_set_data<int>;
using bpolygon    = boost::polygon::polygon_with_holes_data<int>;
using bedge       = std::pair<bpoint, bpoint>;
using namespace boost::polygon::operators;

// --- Edge-pair convolution kernel (direct port of minkowski.cc) ---------------

void convolve_two_segments(std::vector<bpoint>& figure, const bedge& a, const bedge& b) {
    using namespace boost::polygon;
    figure.clear();
    figure.push_back(bpoint(a.first));
    figure.push_back(bpoint(a.first));
    figure.push_back(bpoint(a.second));
    figure.push_back(bpoint(a.second));
    convolve(figure[0], b.second);
    convolve(figure[1], b.first);
    convolve(figure[2], b.first);
    convolve(figure[3], b.second);
}

template <typename itrT1, typename itrT2>
void convolve_two_point_sequences(polygon_set& result, itrT1 ab, itrT1 ae, itrT2 bb, itrT2 be) {
    using namespace boost::polygon;
    if (ab == ae || bb == be) return;
    bpoint prev_a = *ab;
    std::vector<bpoint> vec;
    bpolygon poly;
    ++ab;
    for (; ab != ae; ++ab) {
        bpoint prev_b = *bb;
        itrT2 tmpb = bb;
        ++tmpb;
        for (; tmpb != be; ++tmpb) {
            convolve_two_segments(vec, std::make_pair(prev_b, *tmpb), std::make_pair(prev_a, *ab));
            set_points(poly, vec.begin(), vec.end());
            result.insert(poly);
            prev_b = *tmpb;
        }
        prev_a = *ab;
    }
}

template <typename itrT>
void convolve_point_sequence_with_polygons(polygon_set& result, itrT b, itrT e,
                                           const std::vector<bpolygon>& polygons) {
    using namespace boost::polygon;
    for (std::size_t i = 0; i < polygons.size(); ++i) {
        convolve_two_point_sequences(result, b, e, begin_points(polygons[i]), end_points(polygons[i]));
        for (polygon_with_holes_traits<bpolygon>::iterator_holes_type itrh = begin_holes(polygons[i]);
             itrh != end_holes(polygons[i]); ++itrh) {
            convolve_two_point_sequences(result, b, e, begin_points(*itrh), end_points(*itrh));
        }
    }
}

void convolve_two_polygon_sets(polygon_set& result, const polygon_set& a, const polygon_set& b) {
    using namespace boost::polygon;
    result.clear();
    std::vector<bpolygon> a_polygons;
    std::vector<bpolygon> b_polygons;
    a.get(a_polygons);
    b.get(b_polygons);
    for (std::size_t ai = 0; ai < a_polygons.size(); ++ai) {
        convolve_point_sequence_with_polygons(result, begin_points(a_polygons[ai]),
                                              end_points(a_polygons[ai]), b_polygons);
        for (polygon_with_holes_traits<bpolygon>::iterator_holes_type itrh = begin_holes(a_polygons[ai]);
             itrh != end_holes(a_polygons[ai]); ++itrh) {
            convolve_point_sequence_with_polygons(result, begin_points(*itrh),
                                                  end_points(*itrh), b_polygons);
        }
        for (std::size_t bi = 0; bi < b_polygons.size(); ++bi) {
            bpolygon tmp_poly = a_polygons[ai];
            result.insert(convolve(tmp_poly, *(begin_points(b_polygons[bi]))));
            tmp_poly = b_polygons[bi];
            result.insert(convolve(tmp_poly, *(begin_points(a_polygons[ai]))));
        }
    }
}

// Drop consecutive-duplicate and strictly-collinear vertices from a closed
// ring (flat x,y,...). Boost.Polygon leaves redundant collinear points where
// the convolution of two parallel edges meets; removing them yields the minimal
// polygon (e.g. a true hexagon for tri+tri) and speeds up downstream clipping.
// This is the same "clean polygon" step deepnest applies to NFP output.
std::vector<double> cleanRing(const std::vector<double>& flat) {
    size_t n = flat.size() / 2;
    if (n < 3) return flat;
    std::vector<double> xs(n), ys(n);
    for (size_t i = 0; i < n; i++) { xs[i] = flat[2*i]; ys[i] = flat[2*i+1]; }
    // Collapse the explicit closing vertex if present.
    if (n >= 2 && std::fabs(xs[0]-xs[n-1]) < 1e-7 && std::fabs(ys[0]-ys[n-1]) < 1e-7) {
        xs.pop_back(); ys.pop_back(); n--;
    }
    // Scale-relative tolerance for the collinearity (triangle-area) test.
    double ext = 0;
    for (size_t i = 0; i < n; i++) ext = (std::max)(ext, (std::max)(std::fabs(xs[i]), std::fabs(ys[i])));
    double areaEps = (ext > 0 ? ext : 1.0) * 1e-7;
    double dupEps  = (ext > 0 ? ext : 1.0) * 1e-9;

    std::vector<double> ox, oy;
    ox.reserve(n); oy.reserve(n);
    for (size_t i = 0; i < n; i++) {
        // skip exact duplicate of previous kept vertex
        if (!ox.empty() && std::fabs(xs[i]-ox.back()) < dupEps && std::fabs(ys[i]-oy.back()) < dupEps)
            continue;
        ox.push_back(xs[i]); oy.push_back(ys[i]);
    }
    // wrap-around duplicate
    if (ox.size() >= 2 && std::fabs(ox.front()-ox.back()) < dupEps && std::fabs(oy.front()-oy.back()) < dupEps) {
        ox.pop_back(); oy.pop_back();
    }
    // collinearity removal (iterate until stable, considering wrap-around)
    bool changed = true;
    while (changed && ox.size() > 3) {
        changed = false;
        size_t m = ox.size();
        std::vector<double> nx, ny;
        nx.reserve(m); ny.reserve(m);
        for (size_t i = 0; i < m; i++) {
            size_t p = (i + m - 1) % m, q = (i + 1) % m;
            double cross = (ox[i]-ox[p]) * (oy[q]-oy[p]) - (oy[i]-oy[p]) * (ox[q]-ox[p]);
            if (std::fabs(cross) <= areaEps) { changed = true; continue; } // drop collinear i
            nx.push_back(ox[i]); ny.push_back(oy[i]);
        }
        if (nx.size() < 3) break; // never collapse below a triangle
        ox.swap(nx); oy.swap(ny);
    }
    std::vector<double> out;
    out.reserve(ox.size() * 2);
    for (size_t i = 0; i < ox.size(); i++) { out.push_back(ox[i]); out.push_back(oy[i]); }
    return out;
}

} // namespace

MinkowskiConvolution::Result MinkowskiConvolution::compute(
    const std::vector<Region>& A,
    const std::vector<Region>& B)
{
    using namespace boost::polygon;
    Result result;

    auto anyUsable = [](const std::vector<Region>& R) {
        for (const auto& r : R) if (r.outer.size() >= 6) return true;
        return false;
    };
    if (!anyUsable(A) || !anyUsable(B)) return result;

    // --- dynamic scaling (matches minkowski.cc to keep parity with minkowski.dll) ---
    // Bounds are seeded at 0 like the original, so they always include the origin.
    double Amaxx = 0, Aminx = 0, Amaxy = 0, Aminy = 0;
    for (const auto& r : A) {
        for (size_t i = 0; i + 1 < r.outer.size(); i += 2) {
            double x = r.outer[i], y = r.outer[i + 1];
            Amaxx = (std::max)(Amaxx, x); Aminx = (std::min)(Aminx, x);
            Amaxy = (std::max)(Amaxy, y); Aminy = (std::min)(Aminy, y);
        }
    }
    double Bmaxx = 0, Bminx = 0, Bmaxy = 0, Bminy = 0;
    for (const auto& r : B) {
        for (size_t i = 0; i + 1 < r.outer.size(); i += 2) {
            double x = r.outer[i], y = r.outer[i + 1];
            Bmaxx = (std::max)(Bmaxx, x); Bminx = (std::min)(Bminx, x);
            Bmaxy = (std::max)(Bmaxy, y); Bminy = (std::min)(Bminy, y);
        }
    }
    double Cmaxx = Amaxx + Bmaxx, Cminx = Aminx + Bminx;
    double Cmaxy = Amaxy + Bmaxy, Cminy = Aminy + Bminy;
    double maxxAbs = (std::max)(Cmaxx, std::fabs(Cminx));
    double maxyAbs = (std::max)(Cmaxy, std::fabs(Cminy));
    double maxda = (std::max)(maxxAbs, maxyAbs);
    int maxi = std::numeric_limits<int>::max();
    if (maxda < 1) maxda = 1;
    double inputscale = (0.1 * static_cast<double>(maxi)) / maxda;

    // NEAREST rounding (llround), not truncation: static_cast<int> truncates toward zero, a
    // one-sided error of up to 1/inputscale that BIASES every scaled vertex.
    auto scaled = [&](const std::vector<double>& pts, int sign) {
        std::vector<bpoint> v;
        v.reserve(pts.size() / 2);
        for (size_t i = 0; i + 1 < pts.size(); i += 2) {
            int x = sign * static_cast<int>(std::llround(inputscale * pts[i]));
            int y = sign * static_cast<int>(std::llround(inputscale * pts[i + 1]));
            v.push_back(bpoint(x, y));
        }
        return v;
    };

    // B reference shift = B region 0's first vertex.
    double xshift = 0.0, yshift = 0.0;
    for (const auto& r : B) {
        if (r.outer.size() >= 2) { xshift = r.outer[0]; yshift = r.outer[1]; break; }
    }

    // Build a polygon_set for every B region (negated), once.
    std::vector<polygon_set> bsets;
    bsets.reserve(B.size());
    for (const auto& rb : B) {
        if (rb.outer.size() < 6) continue;
        polygon_set bset;
        bpolygon poly;
        auto pts = scaled(rb.outer, -1);
        set_points(poly, pts.begin(), pts.end());
        bset += poly;
        for (const auto& h : rb.holes) {
            if (h.size() < 6) continue;
            auto hp = scaled(h, -1);
            set_points(poly, hp.begin(), hp.end());
            bset -= poly;
        }
        bsets.push_back(std::move(bset));
    }
    if (bsets.empty()) return result;

    // Convolve per (A region, B region) and append every resulting loop WITHOUT
    // unioning them in Boost. Boost's polygon_set collapses a ring nested inside
    // another ring's hole; keeping the loops separate lets Process2 classify them
    // (primary outer + forbidden lobes + holes) and the downstream NonZero winding
    // reconstruct the nested forbidden zone exactly.
    auto appendPolys = [&](std::vector<bpolygon>& polys) {
        for (std::size_t i = 0; i < polys.size(); ++i) {
            std::vector<double> pointlist;
            for (polygon_traits<bpolygon>::iterator_type itr = polys[i].begin(); itr != polys[i].end(); ++itr) {
                double x1 = static_cast<double>((*itr).get(HORIZONTAL)) / inputscale + xshift;
                double y1 = static_cast<double>((*itr).get(VERTICAL)) / inputscale + yshift;
                pointlist.push_back(x1);
                pointlist.push_back(y1);
            }
            result.outerPaths.push_back(cleanRing(pointlist));

            for (polygon_with_holes_traits<bpolygon>::iterator_holes_type itrh = begin_holes(polys[i]);
                 itrh != end_holes(polys[i]); ++itrh) {
                std::vector<double> child;
                for (polygon_traits<bpolygon>::iterator_type itr2 = (*itrh).begin(); itr2 != (*itrh).end(); ++itr2) {
                    double x1 = static_cast<double>((*itr2).get(HORIZONTAL)) / inputscale + xshift;
                    double y1 = static_cast<double>((*itr2).get(VERTICAL)) / inputscale + yshift;
                    child.push_back(x1);
                    child.push_back(y1);
                }
                result.holes.push_back(cleanRing(child));
            }
        }
    };

    for (const auto& ra : A) {
        if (ra.outer.size() < 6) continue;
        polygon_set aset;
        bpolygon poly;
        auto pts = scaled(ra.outer, 1);
        set_points(poly, pts.begin(), pts.end());
        aset += poly;
        for (const auto& h : ra.holes) {
            if (h.size() < 6) continue;
            auto hp = scaled(h, 1);
            set_points(poly, hp.begin(), hp.end());
            aset -= poly;
        }
        for (auto& bset : bsets) {
            polygon_set tmp;
            convolve_two_polygon_sets(tmp, aset, bset);
            std::vector<bpolygon> polys;
            tmp.get(polys);
            appendPolys(polys);
        }
    }

    return result;
}

} // namespace nest
