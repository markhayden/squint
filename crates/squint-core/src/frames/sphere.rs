//! Equirectangular geometry (SPEC "Frames and locations"): the square perspective view a `view`
//! location or a `view`-mapped crop frame describes, and the mapping of points between such a
//! view and the equirectangular frame it looks into.
//!
//! Conventions: an equirectangular frame's normalised `x` runs from longitude `-π` at the left
//! edge to `π` at the right, `y` from latitude `π/2` at the top to `-π/2` at the bottom. Yaw 0
//! is the centre column, increasing to the right; pitch 0 is the horizon, increasing upward;
//! `fov` is the horizontal field of view of the square view. A direction is a unit vector
//! `(cos lat cos lon, cos lat sin lon, sin lat)`.

use super::{Location, LocationKind};
use crate::{Error, Result};

/// A square perspective view into an equirectangular frame, in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub yaw: f64,
    pub pitch: f64,
    pub fov: f64,
}

impl View {
    /// The view's forward, right and up unit vectors.
    fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        let forward = [cp * cy, cp * sy, sp];
        let right = [-sy, cy, 0.0];
        let up = [-sp * cy, -sp * sy, cp];
        (forward, right, up)
    }

    fn tan_half(&self) -> f64 {
        (self.fov.to_radians() / 2.0).tan()
    }

    /// The direction a normalised view point `(nx, ny)` in `[0, 1]²` looks along.
    pub fn direction(&self, nx: f64, ny: f64) -> [f64; 3] {
        let (f, r, u) = self.basis();
        let t = self.tan_half();
        let cx = (2.0 * nx - 1.0) * t;
        let cy = (1.0 - 2.0 * ny) * t;
        normalise([
            f[0] + cx * r[0] + cy * u[0],
            f[1] + cx * r[1] + cy * u[1],
            f[2] + cx * r[2] + cy * u[2],
        ])
    }

    /// The normalised view point a direction projects to, or `None` when it lies behind the view.
    pub fn project(&self, d: [f64; 3]) -> Option<[f64; 2]> {
        let (f, r, u) = self.basis();
        let depth = dot(d, f);
        if depth <= 1e-9 {
            return None;
        }
        let t = self.tan_half();
        let cx = dot(d, r) / depth / t;
        let cy = dot(d, u) / depth / t;
        Some([(cx + 1.0) / 2.0, (1.0 - cy) / 2.0])
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let n = dot(v, v).sqrt();
    if n == 0.0 {
        return [1.0, 0.0, 0.0];
    }
    [v[0] / n, v[1] / n, v[2] / n]
}

/// The direction of a normalised equirectangular point.
pub fn direction_of_equirect(nx: f64, ny: f64) -> [f64; 3] {
    let lon = nx * 2.0 * std::f64::consts::PI - std::f64::consts::PI;
    let lat = std::f64::consts::FRAC_PI_2 - ny * std::f64::consts::PI;
    [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
}

/// The normalised equirectangular point of a direction, `x` in `[0, 1)`.
pub fn equirect_of(d: [f64; 3]) -> [f64; 2] {
    let lon = d[1].atan2(d[0]);
    let lat = d[2].clamp(-1.0, 1.0).asin();
    let x = ((lon + std::f64::consts::PI) / (2.0 * std::f64::consts::PI)).rem_euclid(1.0);
    [
        x,
        (std::f64::consts::FRAC_PI_2 - lat) / std::f64::consts::PI,
    ]
}

/// Boundary samples of a normalised box, `per_edge` per edge, clockwise from the top-left.
pub fn boundary_of(b: &[f64; 4], per_edge: usize) -> Vec<[f64; 2]> {
    boundary(b, per_edge)
}

fn boundary(b: &[f64; 4], per_edge: usize) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(4 * per_edge);
    let n = per_edge as f64;
    for i in 0..per_edge {
        let t = i as f64 / n;
        out.push([b[0] + t * (b[2] - b[0]), b[1]]);
    }
    for i in 0..per_edge {
        let t = i as f64 / n;
        out.push([b[2], b[1] + t * (b[3] - b[1])]);
    }
    for i in 0..per_edge {
        let t = i as f64 / n;
        out.push([b[2] - t * (b[2] - b[0]), b[3]]);
    }
    for i in 0..per_edge {
        let t = i as f64 / n;
        out.push([b[0], b[3] - t * (b[3] - b[1])]);
    }
    out
}

/// Whether a normalised point lies in a region: a box by containment, the whole image by the
/// unit square, a polygon by ray casting.
fn region_contains(loc: &Location, p: [f64; 2]) -> bool {
    match loc.kind {
        LocationKind::Bbox => loc
            .bbox
            .is_some_and(|b| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]),
        LocationKind::WholeImage => (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]),
        LocationKind::Polygon => loc.polygon.as_ref().is_some_and(|poly| {
            let n = poly.len();
            let mut inside = false;
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a[1] > p[1]) != (b[1] > p[1]) {
                    let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    if p[0] < x {
                        inside = !inside;
                    }
                }
            }
            inside
        }),
        LocationKind::View | LocationKind::Absent => false,
    }
}

/// The middle of a region in view coordinates: a box's centre, a polygon's vertex mean.
fn region_centre(loc: &Location) -> [f64; 2] {
    match (loc.kind, loc.bbox, &loc.polygon) {
        (LocationKind::Bbox, Some(b), _) => [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0],
        (LocationKind::Polygon, _, Some(p)) if !p.is_empty() => {
            let n = p.len() as f64;
            [
                p.iter().map(|v| v[0]).sum::<f64>() / n,
                p.iter().map(|v| v[1]).sum::<f64>() / n,
            ]
        }
        _ => [0.5, 0.5],
    }
}

/// Maps a location in a view-mapped crop to its equirectangular parent. A box becomes the
/// polygon of its mapped boundary (sixteen samples per edge); a polygon maps vertex by vertex
/// with edges subdivided while the 64-vertex limit allows. A footprint whose boundary jumps across
/// the seam is cut to the side holding the region's centre and flagged clipped; one whose region
/// holds a pole becomes a box across the full width down to the footprint's farthest latitude.
pub fn map_through_view(loc: &Location, view: &View, parent: &str) -> Result<(Location, bool)> {
    let mut out = loc.clone();
    out.frame = parent.to_string();
    match loc.kind {
        LocationKind::Absent => return Ok((out, false)),
        LocationKind::View => {
            return Err(Error::Submission(
                "a view location cannot sit on a view frame".into(),
            ));
        }
        _ => {}
    }
    let points = boundary_points(loc)?;
    let footprint = footprint(
        view,
        &points,
        |p| region_contains(loc, p),
        region_centre(loc),
    );
    out.kind = LocationKind::Polygon;
    out.bbox = None;
    out.polygon = None;
    let clipped = match footprint {
        Footprint::Polygon { vertices, clipped } => {
            out.polygon = Some(vertices);
            clipped
        }
        Footprint::Pole { y0, y1 } => {
            out.kind = LocationKind::Bbox;
            out.bbox = Some([0.0, y0, 1.0, y1]);
            false
        }
    };
    Ok((out, clipped))
}

/// A region's boundary in its view, sampled: sixteen points per box edge, a polygon's edges
/// subdivided while the 64-vertex limit allows.
fn boundary_points(loc: &Location) -> Result<Vec<[f64; 2]>> {
    Ok(match loc.kind {
        LocationKind::Bbox => {
            let b = loc
                .bbox
                .ok_or_else(|| Error::Submission("bbox location needs bbox".into()))?;
            boundary(&b, 16)
        }
        LocationKind::WholeImage => boundary(&[0.0, 0.0, 1.0, 1.0], 16),
        LocationKind::Polygon => {
            let p = loc
                .polygon
                .as_ref()
                .ok_or_else(|| Error::Submission("polygon location needs polygon".into()))?;
            let per_edge = (64 / p.len().max(1)).max(1);
            let mut pts = Vec::new();
            for (i, a) in p.iter().enumerate() {
                let b = p[(i + 1) % p.len()];
                for k in 0..per_edge {
                    let t = k as f64 / per_edge as f64;
                    pts.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
                }
            }
            pts
        }
        LocationKind::View | LocationKind::Absent => Vec::new(),
    })
}

/// Both sides of a view footprint that crosses the seam, for the contract (table-manners 9.2: a
/// part never crosses the wrap edge, a shape that does leaves as several parts). `None` when the
/// footprint does not cross the seam or holds a pole. The registry's own location keeps the one
/// side `map_through_view` cuts to, so scoring is unchanged.
pub fn seam_parts_through_view(loc: &Location, view: &View) -> Result<Option<Vec<Vec<[f64; 2]>>>> {
    if matches!(loc.kind, LocationKind::Absent | LocationKind::View) {
        return Ok(None);
    }
    let points = boundary_points(loc)?;
    let dirs: Vec<[f64; 3]> = points.iter().map(|p| view.direction(p[0], p[1])).collect();
    let holds_pole = |pole: [f64; 3]| view.project(pole).is_some_and(|p| region_contains(loc, p));
    if holds_pole([0.0, 0.0, 1.0]) || holds_pole([0.0, 0.0, -1.0]) {
        return Ok(None);
    }
    let mapped: Vec<[f64; 2]> = dirs.iter().map(|d| equirect_of(*d)).collect();
    let centre = region_centre(loc);
    let centre_x = equirect_of(normalise(view.direction(centre[0], centre[1])))[0];
    Ok(split_at_seam(&mapped, centre_x))
}

/// Splits a ring of equirectangular points that jumps across the seam into the pieces on each
/// side: the ring is unwrapped around `centre_x`, then clipped to the frame and, shifted by one
/// width, clipped again. `None` when the ring does not cross the seam.
fn split_at_seam(ring: &[[f64; 2]], centre_x: f64) -> Option<Vec<Vec<[f64; 2]>>> {
    let straddles = ring.len() > 1
        && ring
            .iter()
            .zip(ring.iter().cycle().skip(1))
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.5);
    if !straddles {
        return None;
    }
    let unwrapped: Vec<[f64; 2]> = ring
        .iter()
        .map(|p| {
            let x = if p[0] - centre_x > 0.5 {
                p[0] - 1.0
            } else if centre_x - p[0] > 0.5 {
                p[0] + 1.0
            } else {
                p[0]
            };
            [x, p[1]]
        })
        .collect();
    let shift = if centre_x < 0.5 { 1.0 } else { -1.0 };
    let shifted: Vec<[f64; 2]> = unwrapped.iter().map(|p| [p[0] + shift, p[1]]).collect();
    let parts: Vec<Vec<[f64; 2]>> = [unwrapped, shifted]
        .iter()
        .map(|ring| clip_x(&clip_x(ring, 0.0, true), 1.0, false))
        .filter(|part| part.len() >= 3)
        .collect();
    (parts.len() == 2).then_some(parts)
}

/// One Sutherland–Hodgman pass: keeps the side of the vertical line `x = edge` where
/// `x >= edge` (`keep_right`) or `x <= edge`.
fn clip_x(ring: &[[f64; 2]], edge: f64, keep_right: bool) -> Vec<[f64; 2]> {
    let inside = |p: &[f64; 2]| {
        if keep_right {
            p[0] >= edge
        } else {
            p[0] <= edge
        }
    };
    let mut out = Vec::new();
    for (i, a) in ring.iter().enumerate() {
        let b = &ring[(i + 1) % ring.len()];
        if inside(a) {
            out.push(*a);
        }
        if inside(a) != inside(b) {
            let t = (edge - a[0]) / (b[0] - a[0]);
            out.push([edge, a[1] + t * (b[1] - a[1])]);
        }
    }
    out
}

/// A region's footprint on an equirectangular frame.
pub enum Footprint {
    Polygon {
        vertices: Vec<[f64; 2]>,
        clipped: bool,
    },
    Pole {
        y0: f64,
        y1: f64,
    },
}

/// The equirectangular footprint of a region's boundary points in a view. A pole inside the
/// region (not merely inside the view) makes the footprint run all the way around.
fn footprint(
    view: &View,
    points: &[[f64; 2]],
    region_contains: impl Fn([f64; 2]) -> bool,
    centre: [f64; 2],
) -> Footprint {
    let dirs: Vec<[f64; 3]> = points.iter().map(|p| view.direction(p[0], p[1])).collect();
    let holds_pole = |pole: [f64; 3]| view.project(pole).is_some_and(&region_contains);
    footprint_impl(&dirs, view.direction(centre[0], centre[1]), holds_pole)
}

/// The equirectangular footprint of a closed boundary of directions whose region faces `centre`
/// (any vector along the region's middle): cut at the seam when it straddles it, a full-width box
/// when the boundary winds around a pole, which shows as its longitudes covering the circle.
pub fn footprint_of_directions(dirs: &[[f64; 3]], centre: [f64; 3]) -> Footprint {
    let winds = |pole: [f64; 3]| {
        if dirs.len() < 4 || centre[2] * pole[2] <= 0.0 {
            return false;
        }
        let mut lons: Vec<f64> = dirs.iter().map(|d| d[1].atan2(d[0])).collect();
        lons.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut gap = lons[0] + 2.0 * std::f64::consts::PI - lons[lons.len() - 1];
        for w in lons.windows(2) {
            gap = gap.max(w[1] - w[0]);
        }
        gap < std::f64::consts::FRAC_PI_2
    };
    footprint_impl(dirs, centre, winds)
}

fn footprint_impl(
    dirs: &[[f64; 3]],
    centre: [f64; 3],
    holds_pole: impl Fn([f64; 3]) -> bool,
) -> Footprint {
    let mapped: Vec<[f64; 2]> = dirs.iter().map(|d| equirect_of(*d)).collect();
    let centre_x = equirect_of(normalise(centre))[0];
    let ys = || {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for m in &mapped {
            lo = lo.min(m[1]);
            hi = hi.max(m[1]);
        }
        (lo, hi)
    };
    if holds_pole([0.0, 0.0, 1.0]) {
        let (_, hi) = ys();
        return Footprint::Pole {
            y0: 0.0,
            y1: hi.max(1e-6),
        };
    }
    if holds_pole([0.0, 0.0, -1.0]) {
        let (lo, _) = ys();
        return Footprint::Pole {
            y0: lo.min(1.0 - 1e-6),
            y1: 1.0,
        };
    }
    // The boundary crosses the seam when consecutive samples (the closing edge included) jump by
    // more than half the width.
    let straddles = mapped.len() > 1
        && mapped
            .iter()
            .zip(mapped.iter().cycle().skip(1))
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.5);
    if !straddles {
        return Footprint::Polygon {
            vertices: mapped,
            clipped: false,
        };
    }
    // Cut at the seam: points on the far side from the region's centre are moved to the seam
    // edge nearest the centre. SPEC asks a tool that needs both sides to emit two regions; the
    // registry keeps one and says it clipped.
    let edge = if centre_x < 0.5 { 0.0 } else { 1.0 };
    let vertices = mapped
        .iter()
        .map(|m| {
            if (m[0] - centre_x).abs() > 0.5 {
                [edge, m[1]]
            } else {
                *m
            }
        })
        .collect();
    Footprint::Polygon {
        vertices,
        clipped: true,
    }
}

/// Maps an equirectangular location into a view frame: the bounding box (for a box) or the
/// vertices (for a polygon) of the projected points, clamped to the view; points behind the view
/// are dropped and a location wholly behind or outside it is an error.
pub fn map_into_view(loc: &Location, view: &View, frame: &str) -> Result<Location> {
    let mut out = loc.clone();
    out.frame = frame.to_string();
    let project = |p: &[f64; 2]| view.project(direction_of_equirect(p[0], p[1]));
    let outside = || Error::Submission(format!("location maps wholly outside the view {frame:?}"));
    match loc.kind {
        LocationKind::Bbox | LocationKind::WholeImage => {
            let b = loc.bbox.unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let projected: Vec<[f64; 2]> = boundary(&b, 16).iter().filter_map(project).collect();
            if projected.is_empty() {
                return Err(outside());
            }
            let (mut x0, mut y0, mut x1, mut y1) = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for p in &projected {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            if x1 <= 0.0 || y1 <= 0.0 || x0 >= 1.0 || y0 >= 1.0 {
                return Err(outside());
            }
            out.kind = LocationKind::Bbox;
            out.bbox = Some([
                x0.clamp(0.0, 1.0),
                y0.clamp(0.0, 1.0),
                x1.clamp(0.0, 1.0),
                y1.clamp(0.0, 1.0),
            ]);
        }
        LocationKind::Polygon => {
            let p = loc
                .polygon
                .as_ref()
                .ok_or_else(|| Error::Submission("polygon location needs polygon".into()))?;
            let projected: Vec<[f64; 2]> = p
                .iter()
                .filter_map(project)
                .map(|v| [v[0].clamp(0.0, 1.0), v[1].clamp(0.0, 1.0)])
                .collect();
            if projected.len() < 3 {
                return Err(outside());
            }
            out.polygon = Some(projected);
        }
        LocationKind::View => {
            return Err(Error::Submission(
                "a view location does not map into a view frame".into(),
            ));
        }
        LocationKind::Absent => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(kind: LocationKind, bbox: Option<[f64; 4]>) -> Location {
        Location {
            frame: "crop:call:1:0".into(),
            kind,
            bbox,
            polygon: None,
            yaw: None,
            pitch: None,
            fov: None,
        }
    }

    #[test]
    fn the_view_centre_looks_along_yaw_and_pitch() {
        let v = View {
            yaw: 30.0,
            pitch: 10.0,
            fov: 60.0,
        };
        let d = v.direction(0.5, 0.5);
        let e = equirect_of(d);
        assert!((e[0] - (30.0 + 180.0) / 360.0).abs() < 1e-12, "{e:?}");
        assert!((e[1] - (90.0 - 10.0) / 180.0).abs() < 1e-12, "{e:?}");
        // Right of centre is a larger longitude; up is a smaller y.
        assert!(equirect_of(v.direction(0.9, 0.5))[0] > e[0]);
        assert!(equirect_of(v.direction(0.5, 0.1))[1] < e[1]);
    }

    #[test]
    fn view_to_raw_and_back_round_trips_within_half_a_pixel_at_2048() {
        for (yaw, pitch, fov) in [(0.0, 0.0, 60.0), (120.0, -35.0, 90.0), (-170.0, 60.0, 30.0)] {
            let v = View { yaw, pitch, fov };
            for i in 0..=8 {
                for j in 0..=8 {
                    let (nx, ny) = (i as f64 / 8.0, j as f64 / 8.0);
                    let raw = equirect_of(v.direction(nx, ny));
                    let back = v.project(direction_of_equirect(raw[0], raw[1])).unwrap();
                    // Within half a pixel of a 1024-pixel view.
                    assert!(
                        ((back[0] - nx) * 1024.0).abs() < 0.5,
                        "{back:?} vs {nx},{ny}"
                    );
                    assert!(
                        ((back[1] - ny) * 1024.0).abs() < 0.5,
                        "{back:?} vs {nx},{ny}"
                    );
                    // And the raw point round-trips within half a pixel at 2048 wide.
                    let again = equirect_of(v.direction(back[0], back[1]));
                    assert!(
                        ((again[0] - raw[0]) * 2048.0).abs() < 0.5,
                        "{again:?} vs {raw:?}"
                    );
                    assert!(
                        ((again[1] - raw[1]) * 1024.0).abs() < 0.5,
                        "{again:?} vs {raw:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_box_in_a_view_becomes_a_polygon_around_its_direction() {
        let v = View {
            yaw: 45.0,
            pitch: 0.0,
            fov: 60.0,
        };
        let (m, clipped) = map_through_view(
            &loc(LocationKind::Bbox, Some([0.4, 0.4, 0.6, 0.6])),
            &v,
            "raw:p",
        )
        .unwrap();
        assert!(!clipped);
        assert_eq!(m.frame, "raw:p");
        assert_eq!(m.kind, LocationKind::Polygon);
        let p = m.polygon.unwrap();
        assert_eq!(p.len(), 64);
        let centre = equirect_of(v.direction(0.5, 0.5));
        let (xs, ys): (Vec<f64>, Vec<f64>) = p.iter().map(|q| (q[0], q[1])).unzip();
        let (x0, x1) = (
            xs.iter().cloned().fold(1.0, f64::min),
            xs.iter().cloned().fold(0.0, f64::max),
        );
        let (y0, y1) = (
            ys.iter().cloned().fold(1.0, f64::min),
            ys.iter().cloned().fold(0.0, f64::max),
        );
        assert!(x0 < centre[0] && centre[0] < x1 && y0 < centre[1] && centre[1] < y1);
        // A fifth of a 60° view is about 12°, a thirtieth of the width.
        assert!((x1 - x0 - 12.0 / 360.0).abs() < 0.01, "{}", x1 - x0);
    }

    #[test]
    fn a_footprint_over_the_seam_is_cut_and_flagged_and_a_pole_spans_the_width() {
        let seam = View {
            yaw: -178.0,
            pitch: 0.0,
            fov: 60.0,
        };
        let (m, clipped) =
            map_through_view(&loc(LocationKind::WholeImage, None), &seam, "raw:p").unwrap();
        assert!(clipped);
        let p = m.polygon.unwrap();
        assert!(p.iter().all(|q| q[0] <= 0.5), "all on the left side: {p:?}");
        let pole = View {
            yaw: 0.0,
            pitch: 80.0,
            fov: 60.0,
        };
        let (m, clipped) =
            map_through_view(&loc(LocationKind::WholeImage, None), &pole, "raw:p").unwrap();
        assert!(!clipped);
        assert_eq!(m.kind, LocationKind::Bbox);
        let b = m.bbox.unwrap();
        assert_eq!((b[0], b[1], b[2]), (0.0, 0.0, 1.0));
        assert!(b[3] > 0.1 && b[3] < 0.3, "{b:?}");
        // A small box in that view below the pole holds no pole: a polygon, not the cap.
        let (m, _) = map_through_view(
            &loc(LocationKind::Bbox, Some([0.4, 0.6, 0.6, 0.8])),
            &pole,
            "raw:p",
        )
        .unwrap();
        assert_eq!(m.kind, LocationKind::Polygon, "{m:?}");
        // A region at the top edge of a yaw-0 view looking high up crosses the seam behind the
        // camera and is cut, though the view's centre is at x 0.5.
        let high = View {
            yaw: 0.0,
            pitch: 60.0,
            fov: 120.0,
        };
        let (m, clipped) = map_through_view(
            &loc(LocationKind::Bbox, Some([0.3, 0.0, 0.7, 0.1])),
            &high,
            "raw:p",
        )
        .unwrap();
        assert!(clipped, "{m:?}");
    }

    #[test]
    fn a_raw_box_maps_into_the_view_and_a_box_behind_it_is_refused() {
        let v = View {
            yaw: 0.0,
            pitch: 0.0,
            fov: 90.0,
        };
        // A box around the centre direction (x = 0.5): straight ahead.
        let raw = Location {
            frame: "raw:p".into(),
            ..loc(LocationKind::Bbox, Some([0.48, 0.45, 0.52, 0.55]))
        };
        let m = map_into_view(&raw, &v, "crop:call:1:0").unwrap();
        let b = m.bbox.unwrap();
        assert!(
            b[0] < 0.5 && b[2] > 0.5 && b[1] < 0.5 && b[3] > 0.5,
            "{b:?}"
        );
        // Behind the view (x near 0 is longitude -180°).
        let behind = Location {
            frame: "raw:p".into(),
            ..loc(LocationKind::Bbox, Some([0.0, 0.45, 0.04, 0.55]))
        };
        assert!(map_into_view(&behind, &v, "crop:call:1:0").is_err());
    }
}
