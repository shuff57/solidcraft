//! reSHape local patch (not upstream).
//!
//! - `surface.wrap_sketch`: the ribbon entry for Wrap Sketch. The work is done from its dialog in
//!   the app (`ui-egui/src/dialogs_wrap.rs`) by the reSHape layer through the control port; as a
//!   command on its own it only says so.
//! - `reshape.overlay`: polylines the viewport draws on top of the model, set by the reSHape
//!   helper: a surface sketch shown wrapped on its face while it is drawn flat. View-only state,
//!   outside the document and the undo history.
//! - `reshape.surface_warp`: for each surface sketch, a grid of where its plane's points lie on
//!   the curved face. The viewport draws the sketch through it (on the face, not on the flat
//!   plane) and turns clicks on the face back into sketch points. View-only too: the sketch
//!   itself stays an ordinary flat sketch.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use crate::geom::prelude::*;
use solidcore_record::{Plane, Vec2};

use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::bad;
use crate::{Result, Session, Vec3};

const CMD: &str = "surface.wrap_sketch";
const OVERLAY: &str = "reshape.overlay";
const WARP: &str = "reshape.surface_warp";
pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(CMD, "Wrap Sketch", run)
        .at("SOLID", "CREATE")
        .icon("project_surface")
        .noundo()
        .params("Opens its dialog: a sketch wrapped onto a curved face or across the edge between two flat faces, then cut, raised or marked. reSHape builds and checks it through the control port."),
    CommandSpec::new(OVERLAY, "reSHape Overlay", set_overlay)
        .noundo()
        .params("lines: [[[x,y,z], ...], ...] world polylines drawn over the model (view only; [] clears)"),
    CommandSpec::new(WARP, "reSHape Surface Warp", set_warps)
        .noundo()
        .params("warps: [{plane:{origin,x_dir,y_dir}, u0, v0, du, dv, nu, nv, points:[[x,y,z]|null, ...] (row by row, u fastest)}] (view only; [] clears)"),
];

fn run(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(bad(CMD, "Wrap Sketch runs from its dialog in the app: reSHape builds and checks it"))
}

type Lines = Vec<Vec<[f32; 3]>>;
static LINES: Mutex<(u64, Lines)> = Mutex::new((0, Vec::new()));
static GEN: AtomicU64 = AtomicU64::new(0);

fn set_overlay(_s: &mut Session, p: &Value) -> Result<Value> {
    let lines: Vec<Vec<[f64; 3]>> =
        serde_json::from_value(p.get("lines").cloned().unwrap_or(json!([]))).map_err(|_| bad(OVERLAY, "lines must be [[[x,y,z], ...], ...]"))?;
    let lines: Lines = lines
        .into_iter()
        .filter(|l| l.len() >= 2 && l.len() <= 100_000 && l.iter().flatten().all(|v| v.is_finite()))
        .map(|l| l.into_iter().map(|[x, y, z]| [x as f32, y as f32, z as f32]).collect())
        .take(10_000)
        .collect();
    let n = lines.len();
    if let Ok(mut g) = LINES.lock() {
        g.0 += 1;
        g.1 = lines;
    }
    GEN.fetch_add(1, Ordering::Relaxed);
    Ok(json!({ "lines": n }))
}

/// The overlay's polylines (world, mm).
pub fn overlay() -> Lines {
    LINES.lock().map(|g| g.1.clone()).unwrap_or_default()
}

/// Changes each time the overlay is set: part of the viewport's scene key.
pub fn generation() -> u64 {
    GEN.load(Ordering::Relaxed)
}

/// A surface sketch's plane, and where its points lie on the face: grid point (i, j) is sketch
/// point (u0 + i·du, v0 + j·dv). Between grid points the face is bilinear.
struct Warp {
    plane: Plane,
    n: Vec3,
    u0: f64,
    v0: f64,
    du: f64,
    dv: f64,
    nu: usize,
    nv: usize,
    pts: Vec<Option<Vec3>>,
}

#[derive(serde::Deserialize)]
struct WarpIn {
    plane: Plane,
    u0: f64,
    v0: f64,
    du: f64,
    dv: f64,
    nu: usize,
    nv: usize,
    points: Vec<Option<[f64; 3]>>,
}

static WARPS: RwLock<Vec<Warp>> = RwLock::new(Vec::new());
static ANY: AtomicBool = AtomicBool::new(false);

fn set_warps(_s: &mut Session, p: &Value) -> Result<Value> {
    let ins: Vec<WarpIn> = serde_json::from_value(p.get("warps").cloned().unwrap_or(json!([])))
        .map_err(|_| bad(WARP, "warps must be [{plane, u0, v0, du, dv, nu, nv, points}]"))?;
    let out: Vec<Warp> = ins
        .into_iter()
        .take(64)
        .filter(|w| w.nu >= 2 && w.nv >= 2 && w.nu * w.nv == w.points.len() && w.points.len() <= 250_000)
        .filter(|w| w.du > 0.0 && w.dv > 0.0 && [w.u0, w.v0, w.du, w.dv].iter().all(|v| v.is_finite()))
        .map(|w| Warp {
            n: w.plane.x.cross(w.plane.y),
            plane: w.plane,
            u0: w.u0,
            v0: w.v0,
            du: w.du,
            dv: w.dv,
            nu: w.nu,
            nv: w.nv,
            pts: w.points.into_iter().map(|q| q.filter(|q| q.iter().all(|v| v.is_finite())).map(Vec3::from)).collect(),
        })
        .collect();
    let n = out.len();
    if let Ok(mut g) = WARPS.write() {
        *g = out;
    }
    ANY.store(n > 0, Ordering::Relaxed);
    GEN.fetch_add(1, Ordering::Relaxed);
    Ok(json!({ "warps": n }))
}

fn same(a: &Plane, b: &Plane) -> bool {
    a.origin.dist(b.origin) < 1e-7 && a.x.dist(b.x) < 1e-9 && a.y.dist(b.y) < 1e-9
}

impl Warp {
    fn at(&self, u: f64, v: f64) -> Option<Vec3> {
        let (fi, fj) = ((u - self.u0) / self.du, (v - self.v0) / self.dv);
        if !(fi >= 0.0 && fj >= 0.0 && fi <= (self.nu - 1) as f64 && fj <= (self.nv - 1) as f64) {
            return None;
        }
        let (i, j) = ((fi.floor() as usize).min(self.nu - 2), (fj.floor() as usize).min(self.nv - 2));
        let (s, t) = (fi - i as f64, fj - j as f64);
        let g = |i: usize, j: usize| self.pts[j * self.nu + i];
        let (a, b, c, d) = (g(i, j)?, g(i + 1, j)?, g(i, j + 1)?, g(i + 1, j + 1)?);
        Some(a * ((1.0 - s) * (1.0 - t)) + b * (s * (1.0 - t)) + c * ((1.0 - s) * t) + d * (s * t))
    }
    fn local(&self, p: Vec3) -> Option<(f64, f64)> {
        let d = p - self.plane.origin;
        (d.dot(self.n).abs() <= 1e-6).then(|| (d.dot(self.plane.x), d.dot(self.plane.y)))
    }
    /// The sketch point whose place on the face the ray meets first.
    fn hit(&self, o: Vec3, dir: Vec3) -> Option<Vec2> {
        let mut best: Option<(f64, Vec2)> = None;
        let uv = |i: usize, j: usize| Vec2::new(self.u0 + i as f64 * self.du, self.v0 + j as f64 * self.dv);
        for j in 0..self.nv - 1 {
            for i in 0..self.nu - 1 {
                let g = |i: usize, j: usize| self.pts[j * self.nu + i];
                let (Some(a), Some(b), Some(c), Some(d)) = (g(i, j), g(i + 1, j), g(i, j + 1), g(i + 1, j + 1)) else { continue };
                for (p, q) in [([a, b, d], [uv(i, j), uv(i + 1, j), uv(i + 1, j + 1)]), ([a, d, c], [uv(i, j), uv(i + 1, j + 1), uv(i, j + 1)])] {
                    let (e1, e2) = (p[1] - p[0], p[2] - p[0]);
                    let h = dir.cross(e2);
                    let det = e1.dot(h);
                    if det.abs() < 1e-14 {
                        continue;
                    }
                    let f = 1.0 / det;
                    let s = o - p[0];
                    let b1 = f * s.dot(h);
                    if !(-1e-9..=1.0 + 1e-9).contains(&b1) {
                        continue;
                    }
                    let k = s.cross(e1);
                    let b2 = f * dir.dot(k);
                    if b2 < -1e-9 || b1 + b2 > 1.0 + 1e-9 {
                        continue;
                    }
                    let t = f * e2.dot(k);
                    if t > 0.0 && best.is_none_or(|(bt, _)| t < bt) {
                        best = Some((t, q[0] * (1.0 - b1 - b2) + q[1] * b1 + q[2] * b2));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }
}

/// A point on any surface sketch's plane, put where it lies on its face (`None`: not on one).
pub fn warp_point(p: Vec3) -> Option<Vec3> {
    if !ANY.load(Ordering::Relaxed) {
        return None;
    }
    let g = WARPS.read().ok()?;
    g.iter().find_map(|w| w.local(p).and_then(|(u, v)| w.at(u, v)))
}

/// Is `plane` a surface sketch's plane, drawn on its face?
pub fn warps(plane: &Plane) -> bool {
    ANY.load(Ordering::Relaxed) && WARPS.read().is_ok_and(|g| g.iter().any(|w| same(&w.plane, plane)))
}

/// The segment `a`–`b` on `plane`, as short pieces on the face (just `a`–`b` when the plane is
/// not a surface sketch's). Pieces off the face are left out.
pub fn warp_pieces(plane: &Plane, a: Vec3, b: Vec3, step: f64) -> Vec<(Vec3, Vec3)> {
    let Some(g) = ANY.load(Ordering::Relaxed).then(|| WARPS.read().ok()).flatten() else { return vec![(a, b)] };
    let Some(w) = g.iter().find(|w| same(&w.plane, plane)) else { return vec![(a, b)] };
    let n = ((a.dist(b) / step).ceil() as usize).clamp(1, 1024);
    let on = |t: f64| w.local(a.lerp(b, t)).and_then(|(u, v)| w.at(u, v));
    let pts: Vec<Option<Vec3>> = (0..=n).map(|k| on(k as f64 / n as f64)).collect();
    pts.windows(2).filter_map(|p| Some((p[0]?, p[1]?))).collect()
}

/// The same polylines on `plane` with points every `step` (so they bend with the face when
/// drawn through it). Unchanged when `plane` is not a surface sketch's.
pub fn densify(plane: &Plane, parts: &mut [Vec<Vec3>], step: f64) {
    if !warps(plane) {
        return;
    }
    for part in parts.iter_mut() {
        let mut out = Vec::with_capacity(part.len());
        for (k, w) in part.windows(2).enumerate() {
            let n = ((w[0].dist(w[1]) / step).ceil() as usize).clamp(1, 1024);
            if k == 0 {
                out.push(w[0]);
            }
            out.extend((1..=n).map(|i| w[0].lerp(w[1], i as f64 / n as f64)));
        }
        if !out.is_empty() {
            *part = out;
        }
    }
}

/// Where the ray meets `plane`, or for a surface sketch, the point of its plane whose place on
/// the face the ray meets (the plane itself when the ray misses the face).
pub fn intersect_ray(plane: &Plane, o: Vec3, dir: Vec3) -> Option<Vec3> {
    if ANY.load(Ordering::Relaxed)
        && let Ok(g) = WARPS.read()
        && let Some(w) = g.iter().find(|w| same(&w.plane, plane))
        && let Some(q) = w.hit(o, dir)
    {
        return Some(plane.to_world(q));
    }
    plane.intersect_ray(o, dir)
}

/// Is the body face at `p` planar (`None`: no face there)?
pub fn face_is_planar(s: &Session, p: Vec3) -> Option<bool> {
    super::face::face_is_planar(s, p)
}
