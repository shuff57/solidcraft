//! reSHape local patch (not upstream): Wrap Sketch. A sketch wrapped onto a curved face (cylinder,
//! sphere, freeform) or across the edge between two flat faces, then cut, raised or marked.
//!
//! The dialog gathers the pick and the sizes. The reSHape layer does the rest from a Node helper
//! (`packages/layer/tools/surface-panel/server.mjs --once`): it reads the surface, plans the
//! sketch, shows it wrapping on, builds it through this app's control port, measures the result
//! and keeps it only when it matches its own prediction, otherwise it puts the design back and
//! says why. The answer arrives on the status line.
use std::sync::Mutex;

use serde_json::json;

use crate::selection::{EDGES, FACES, FEATURES, PROFILES, SelInput};
use solidcraft_engine::Sel;
use crate::{SolveApp, dialogs::Kind, ui_kit};

pub const ID: &str = "surface.wrap_sketch";
const SHAPES: [&str; 4] = ["circle", "rect", "slot", "ellipse"];
const OPS: [&str; 3] = ["cut", "raise", "mark"];

#[derive(Clone, Debug, PartialEq)]
pub struct Wrap {
    pub shape: usize,
    pub a: f64,
    pub b: f64,
    pub turn: f64,
    pub across: f64,
    pub along: f64,
    pub op: usize,
    pub depth: f64,
    pub square: bool,
    pub show_wrap: bool,
}

impl Default for Wrap {
    fn default() -> Self {
        Wrap { shape: 0, a: 5.0, b: 4.0, turn: 0.0, across: 0.0, along: 0.0, op: 0, depth: 1.0, square: false, show_wrap: true }
    }
}

/// What the last Wrap Sketch left behind: kept between dialogs, so the next one starts from it.
static LAST: Mutex<Option<Wrap>> = Mutex::new(None);
/// The helper's answer, waiting for the window thread: (text, error).
static DONE: Mutex<Option<(String, bool)>> = Mutex::new(None);
static RUNNING: Mutex<bool> = Mutex::new(false);

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == ID).then(|| {
        let w = LAST.lock().ok().and_then(|l| l.clone()).unwrap_or_default();
        (Kind::Wrap(w), vec![SelInput::new("Face, corner edge or surface sketch", FACES | EDGES | PROFILES | FEATURES, false)])
    })
}

/// A surface sketch (one of its profiles, or the sketch) is picked: its own curves are the shape.
fn sketch_picked(inputs: &[SelInput]) -> bool {
    matches!(crate::selection::items(inputs, 0).first(), Some(Sel::Profile { .. } | Sel::Feature { .. }))
}

pub fn rows(app: &SolveApp, ui: &mut egui::Ui, w: &mut Wrap, inputs: &[SelInput]) {
    let units = app.session.doc.units.clone();
    if sketch_picked(inputs) {
        ui_kit::text(ui, "The surface sketch's own curves are wrapped.", ui_kit::Text::Note("wrap_sketch"));
    } else {
        shape_rows(ui, w, &units);
    }
    op_rows(ui, w, &units);
}

fn shape_rows(ui: &mut egui::Ui, w: &mut Wrap, units: &str) {
    let units = units.to_string();
    ui_kit::row(ui, "Shape", |ui| ui_kit::segmented(ui, &mut w.shape, &["Circle", "Rect", "Slot", "Ellipse"], Default::default()));
    let (la, lb) = match w.shape {
        0 => ("Radius", None),
        1 => ("Width", Some("Height")),
        2 => ("Length", Some("End radius")),
        _ => ("Radius 1", Some("Radius 2")),
    };
    ui_kit::row(ui, la, |ui| ui_kit::bounded_length_field(ui, &mut w.a, &units, 3, 0.05..=500.0, 0.1));
    if let Some(lb) = lb {
        ui_kit::row(ui, lb, |ui| ui_kit::bounded_length_field(ui, &mut w.b, &units, 3, 0.05..=500.0, 0.1));
    }
    ui_kit::row(ui, "Turn", |ui| ui_kit::angle_field(ui, &mut w.turn));
    ui_kit::row(ui, "Move across", |ui| ui_kit::bounded_length_field(ui, &mut w.across, &units, 2, -500.0..=500.0, 0.1));
    ui_kit::row(ui, "Move along", |ui| ui_kit::bounded_length_field(ui, &mut w.along, &units, 2, -500.0..=500.0, 0.1));
}

fn op_rows(ui: &mut egui::Ui, w: &mut Wrap, units: &str) {
    let units = units.to_string();
    ui_kit::row(ui, "Operation", |ui| ui_kit::segmented(ui, &mut w.op, &["Cut", "Raise", "Mark"], Default::default()));
    if w.op != 2 {
        ui_kit::row(ui, if w.op == 1 { "Height" } else { "Depth" }, |ui| {
            ui_kit::bounded_length_field(ui, &mut w.depth, &units, 3, 0.01..=100.0, 0.05)
        });
        ui_kit::row(ui, "Square to surface", |ui| ui_kit::checkbox(ui, "", &mut w.square));
    }
    ui_kit::row(ui, "Show the wrap", |ui| ui_kit::checkbox(ui, "", &mut w.show_wrap));
    ui_kit::text(
        ui,
        "Moves are measured along the surface. reSHape measures the result and keeps it only if it matches; otherwise the design goes back and the status line says why.",
        ui_kit::Text::Note("wrap"),
    );
}

/// The helper: `RESHAPE_WRAP_HELPER` or the reSHape checkout this build was made for.
fn helper() -> std::path::PathBuf {
    std::env::var_os("RESHAPE_WRAP_HELPER").map(Into::into).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        home.join("Documents/GitHub/reshape-cad/packages/layer/tools/surface-panel/server.mjs")
    })
}

fn app_port() -> Result<u16, String> {
    solidcraft_engine::control_auth::APP.read_current_port().ok_or_else(|| "This app has no control port open (Preferences › Assistant › agent control).".to_string())
}

fn helper_cmd(port: u16) -> Result<std::process::Command, String> {
    let helper = helper();
    if !helper.exists() {
        return Err(format!("The reSHape helper was not found at {} (set RESHAPE_WRAP_HELPER).", helper.display()));
    }
    let mut cmd = std::process::Command::new("node");
    cmd.arg(&helper).arg("--port").arg(port.to_string());
    if let Some(cli) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("solidcraft-cli"))) {
        cmd.arg("--cli").arg(cli);
    }
    Ok(cmd)
}

static WATCHER: Mutex<Option<std::process::Child>> = Mutex::new(None);

/// The helper that keeps every surface sketch drawn wrapped on its face (`reshape.overlay`),
/// started once and stopped when this app exits (it watches our process id).
fn ensure_watcher(port: u16) {
    let Ok(mut w) = WATCHER.lock() else { return };
    if let Some(child) = w.as_mut()
        && matches!(child.try_wait(), Ok(None))
    {
        return;
    }
    if let Ok(mut cmd) = helper_cmd(port) {
        cmd.arg("--watch").arg("--app-pid").arg(std::process::id().to_string());
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        *w = cmd.spawn().ok();
    }
}

/// Run the helper once in the background; its JSON answer goes to the status line.
fn helper_once(port: u16, args: &[&str], ok_text: impl Fn(&serde_json::Value) -> String + Send + 'static) -> Result<(), String> {
    let mut cmd = helper_cmd(port)?;
    cmd.args(args);
    if let Ok(mut r) = RUNNING.lock() {
        *r = true;
    }
    std::thread::spawn(move || {
        let answer = match cmd.output() {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                let last = text.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
                match serde_json::from_str::<serde_json::Value>(last) {
                    Ok(v) if v["ok"].as_bool() == Some(true) => (ok_text(&v), false),
                    Ok(v) => {
                        let mut s = v["refusal"].as_str().unwrap_or("refused").to_string();
                        if let Some(d) = v["detail"].as_str() {
                            s.push_str(&format!(" ({d})"));
                        }
                        (format!("reSHape refused: {s}"), true)
                    }
                    Err(_) => {
                        let err = String::from_utf8_lossy(&out.stderr);
                        (format!("reSHape: the helper failed: {}", err.lines().last().unwrap_or("no answer")), true)
                    }
                }
            }
            Err(e) => (format!("reSHape: could not start node ({e})"), true),
        };
        if let Ok(mut d) = DONE.lock() {
            *d = Some(answer);
        }
        if let Ok(mut r) = RUNNING.lock() {
            *r = false;
        }
    });
    Ok(())
}

/// New Sketch on a curved face: reSHape lays a sketch on the plane touching the face where it was
/// clicked (its lengths are lengths along the surface) and draws it wrapped on the face as it is
/// drawn. Wrap Sketch then cuts, raises or marks it.
pub fn start_surface_sketch(app: &mut SolveApp, sel: &Sel) -> Result<(), String> {
    let port = app_port()?;
    ensure_watcher(port);
    let sel = serde_json::to_value(sel).map_err(|e| e.to_string())?.to_string();
    app.set_status("Surface sketch: reSHape is reading the face…", false);
    helper_once(port, &["--sketch", &sel], |v| {
        format!(
            "Surface sketch {} on the {}: draw on it as usual, it shows wrapped on the face. Finish, then Wrap Sketch cuts or raises it.",
            v["name"].as_str().unwrap_or(""),
            v["label"].as_str().unwrap_or("face")
        )
    })
}

pub fn run(app: &mut SolveApp, w: &Wrap, inputs: &[SelInput]) -> Result<(), String> {
    let Some(sel) = crate::selection::items(inputs, 0).first() else {
        return Err("Pick a curved face, or the edge between two flat faces.".into());
    };
    if RUNNING.lock().map(|r| *r).unwrap_or(false) {
        return Err("A Wrap Sketch is still being built and checked.".into());
    }
    let sel = serde_json::to_value(sel).map_err(|e| e.to_string())?;
    let req = json!({
        "sel": sel, "shape": SHAPES[w.shape.min(3)], "a": w.a, "b": w.b, "turn": w.turn, "du": w.across, "dv": w.along,
        "op": OPS[w.op.min(2)], "depth": w.depth, "square": w.square, "animate": w.show_wrap,
    });
    if let Ok(mut l) = LAST.lock() {
        *l = Some(w.clone());
    }
    let port = app_port()?;
    ensure_watcher(port);
    app.set_status("Wrap Sketch: reSHape is building and checking it…", false);
    let req = req.to_string();
    helper_once(port, &["--once", &req], |v| format!("Wrap Sketch {}: {}", v["id"].as_str().unwrap_or(""), v["result"].as_str().unwrap_or("built")))
}

/// Each frame: the helper's answer, once, onto the status line.
pub fn poll(app: &mut SolveApp, ctx: &egui::Context) {
    if RUNNING.lock().map(|r| *r).unwrap_or(false) {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    if let Some((text, error)) = DONE.lock().ok().and_then(|mut d| d.take()) {
        app.set_status(text, error);
    }
}
