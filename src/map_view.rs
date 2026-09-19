//! The map widget: draws the tree as nested rectangles (2D) or as a city of
//! boxes (3D) and lets you fly around it.
//!
//! 2D: everything is a quad drawn by one tiny pixel shader. Makepad batches
//! all quads of the same shader into one GPU draw call, which is why drawing
//! a hundred thousand of them per frame is fine.
//! 3D: see `city.rs`.

mod city;

use crate::history::{self, History};
use crate::i18n::Language;
use crate::model::{code_inner, frame, ColorMode, Kind, Tree, CHAR_W, COL_CHARS, R};
use crate::orbit::Orbit;
use crate::scan::{self, Entry};
use makepad_widgets::*;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{channel, Receiver, Sender},
    time::Instant,
};

/// Runtime detail presets. Normal preserves the original rendering limits.
/// Lower pixel thresholds reveal distant geometry; budgets bound CPU/GPU work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailLevel {
    #[default]
    Normal,
    High,
    Ultra,
    Custom,
}

impl DetailLevel {
    fn pixels(self, normal: f64, high: f64, ultra: f64, custom_percent: f64) -> f64 {
        match self {
            Self::Normal => normal,
            Self::High => high,
            Self::Ultra => ultra,
            Self::Custom => {
                let amount = (custom_percent / 100.0).clamp(0.0, 1.0);
                normal + (ultra - normal) * amount
            }
        }
    }

    fn budget(self, normal: usize, high: usize, ultra: usize, custom_percent: f64) -> usize {
        match self {
            Self::Normal => normal,
            Self::High => high,
            Self::Ultra => ultra,
            Self::Custom => ((normal as f64 * (custom_percent / 100.0).clamp(0.1, 10.0))
                .round() as usize)
                .max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CustomDetail {
    pub geometry: f64,
    pub text: f64,
    pub budget: f64,
}

impl Default for CustomDetail {
    fn default() -> Self {
        Self {
            geometry: 70.0,
            text: 70.0,
            budget: 200.0,
        }
    }
}

impl CustomDetail {
    pub fn new(geometry: f64, text: f64, budget: f64) -> Self {
        Self {
            geometry: geometry.clamp(0.0, 100.0),
            text: text.clamp(0.0, 100.0),
            budget: budget.clamp(10.0, 1_000.0),
        }
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // The shader for every box, line strip and label pill. `self.pos` is the
    // position inside the quad (0..1) and `self.rect_size` its size in pixels,
    // just like UV coordinates and a resolution uniform in a Three.js shader.
    set_type_default() do #(DrawBlock::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #x202630
        edge: #x000000
        border: 0.0
        hatch: 0.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let d = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
            // diagonal stripes mark ignored things
            let stripe = step(0.5, fract((p.x + p.y) / 10.0)) * self.hatch
            let base = vec4(self.color.rgb * (1.0 + 0.6 * stripe), self.color.w)
            let cov = clamp(self.border - d + 0.5, 0.0, 1.0) * step(0.01, self.border)
            let c = mix(base, self.edge, cov)
            return vec4(c.rgb * c.w, c.w)
        }
    }

    // Shows the offscreen 3D render (a texture) as a flat quad in the UI,
    // like rendering a Three.js scene to a WebGLRenderTarget.
    set_type_default() do #(DrawSceneTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)
        pixel: fn() {
            let c = self.scene_texture.sample_as_bgra(self.pos)
            return vec4(c.rgb * c.w, c.w)
        }
    }

    mod.widgets.CodeMapBase = #(CodeMap::register_widget(vm))
    mod.widgets.CodeMap = set_type_default() do mod.widgets.CodeMapBase{
        width: Fill
        height: Fill
        draw_bg +: {color: #x07090c}
        draw_block +: {}
        draw_scene +: {}
        draw_cube +: {}
        draw_label +: {
            color: #xe6ebf0
            text_style: theme.font_regular{font_size: 8.0}
        }
        draw_code +: {
            color: #xc9d1d9
            text_style: theme.font_code{font_size: 6.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBlock {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    border: f32,
    #[live]
    hatch: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneTexture {
    #[deref]
    draw_super: DrawQuad,
}

/// What the app shows in the inspector panel.
#[derive(Clone, Debug, Default)]
pub struct NodeInfo {
    pub title: String,
    pub path: String,
    pub details: String,
}

#[derive(Clone, Debug, Default)]
pub enum CodeMapAction {
    #[default]
    None,
    Selected(NodeInfo),
    Status(String),
}

enum ScanMsg {
    Project(Result<scan::ProjectScan, String>, f64),
    Expanded(String, Vec<Entry>),
    History(Result<History, String>),
}

#[derive(Clone, Debug, Default)]
enum MapStatus {
    #[default]
    NoFolder,
    Scanning,
    ReadingHistory,
    Summary,
    Search,
    Scanned { seconds: f64, used_git: bool },
    ScanFailed(String),
    ExpandedIgnored { path: String, count: usize },
    HistoryFailed(String),
}

struct Label {
    rect: Rect,
    text: String,
    dim: bool,
}

#[derive(Clone, Copy)]
struct Drag {
    start: DVec2,
    last: DVec2,
    start_off: DVec2,
    moved: bool,
    taps: u32,
    /// Right button or shift: pan instead of orbit (3D only).
    pan: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct CodeMap {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_block: DrawBlock,
    #[live]
    draw_scene: DrawSceneTexture,
    #[live]
    draw_cube: DrawCube,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_code: DrawText,

    // 3D render target: its own render pass with color and depth textures.
    // `#[new]` means "create this with ::new(cx)" when the widget is built.
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    pass_ready: bool,
    /// One draw list per file whose code is shown on its tower top in 3D.
    #[rust]
    code_lists: Vec<DrawList>,

    #[rust]
    root: PathBuf,
    #[rust]
    tree: Tree,
    #[rust(true)]
    show_ignored: bool,
    #[rust]
    needs_layout: bool,
    #[rust]
    needs_fit: bool,
    /// The map's rectangle on screen, from the last draw.
    #[rust]
    view: Rect,
    /// World size of the whole map.
    #[rust]
    world: R,
    /// 2D camera: screen = view.pos + (world - cam_off) * cam_scale
    #[rust]
    cam_scale: f64,
    #[rust]
    cam_off: DVec2,
    /// Where a double click is flying the 2D camera to: (world center, scale).
    #[rust]
    flight: Option<(DVec2, f64)>,
    #[rust]
    mode_3d: bool,
    #[rust]
    detail_level: DetailLevel,
    #[rust]
    custom_detail: CustomDetail,
    #[rust]
    language: Language,
    #[rust]
    orbit: Orbit,
    /// Where the 3D camera is flying to: (target, distance).
    #[rust]
    orbit_flight: Option<(Vec3f, f32)>,
    /// Boxes drawn in the last 3D frame, for mouse picking.
    #[rust]
    picks: Vec<(usize, Vec3f, Vec3f)>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    hover_abs: DVec2,
    #[rust]
    selected: Option<usize>,
    #[rust]
    color_mode: ColorMode,
    #[rust]
    history: Option<History>,
    #[rust]
    search: String,
    #[rust]
    matches: Vec<usize>,
    #[rust]
    match_cursor: usize,
    #[rust]
    tx: Option<Sender<ScanMsg>>,
    #[rust]
    rx: Option<Receiver<ScanMsg>>,
    #[rust]
    pending: usize,
    #[rust]
    frame: NextFrame,
    /// File contents for the fully zoomed-in view, loaded on demand.
    #[rust]
    text_cache: HashMap<usize, Vec<String>>,
    #[rust]
    status: MapStatus,
    #[rust]
    message: String,
    #[rust]
    labels: Vec<Label>,
}

impl CodeMap {
    pub fn open(&mut self, cx: &mut Cx, root: PathBuf) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        self.tree = Tree::new(&name);
        self.root = root.clone();
        self.selected = None;
        self.hover = None;
        self.history = None;
        self.text_cache.clear();
        let (tx, rx) = channel();
        let worker_tx = tx.clone();
        self.tx = Some(tx);
        self.rx = Some(rx);
        self.pending = 1;
        self.set_status(cx, MapStatus::Scanning);
        // Scanning runs on a background thread so the window stays responsive.
        // `move` hands ownership of `root` and the sender to that thread.
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = scan::scan_project(&root);
            let _ = worker_tx.send(ScanMsg::Project(result, started.elapsed().as_secs_f64()));
        });
        self.frame = cx.new_next_frame();
        self.redraw(cx);
    }

    fn status_message(&self) -> String {
        match &self.status {
            MapStatus::NoFolder => self.language.no_folder().to_string(),
            MapStatus::Scanning => self.language.scanning(&self.root.display().to_string()),
            MapStatus::ReadingHistory => self.language.reading_history(),
            MapStatus::Summary => self.summary(),
            MapStatus::Search => self
                .language
                .search_matches(&fmt_num(self.matches.len() as u64), &self.search),
            MapStatus::Scanned { seconds, used_git } => {
                self.language.scanned(&self.summary(), *seconds, *used_git)
            }
            MapStatus::ScanFailed(error) => self.language.scan_failed(error),
            MapStatus::ExpandedIgnored { path, count } => self
                .language
                .expanded_ignored(path, &fmt_num(*count as u64)),
            MapStatus::HistoryFailed(error) => self.language.history_failed(error),
        }
    }

    fn publish_status(&mut self, cx: &mut Cx) {
        self.message = self.status_message();
        cx.widget_action(self.uid, CodeMapAction::Status(self.message.clone()));
    }

    fn set_status(&mut self, cx: &mut Cx, status: MapStatus) {
        self.status = status;
        self.publish_status(cx);
    }

    pub fn set_language(&mut self, cx: &mut Cx, language: Language) {
        if self.language == language {
            return;
        }
        self.language = language;
        self.publish_status(cx);
        if let Some(index) = self.selected {
            cx.widget_action(self.uid, CodeMapAction::Selected(self.info(index)));
        }
        self.redraw(cx);
    }

    pub fn set_show_ignored(&mut self, cx: &mut Cx, show: bool) {
        self.show_ignored = show;
        self.needs_layout = true;
        self.redraw(cx);
    }

    pub fn set_detail_level(&mut self, cx: &mut Cx, level: DetailLevel) {
        if self.detail_level != level {
            self.detail_level = level;
            self.redraw(cx);
        }
    }

    pub fn set_custom_detail(&mut self, cx: &mut Cx, detail: CustomDetail) {
        if self.custom_detail != detail {
            self.custom_detail = detail;
            self.redraw(cx);
        }
    }

    fn geometry_pixels(&self, normal: f64, high: f64, ultra: f64) -> f64 {
        self.detail_level
            .pixels(normal, high, ultra, self.custom_detail.geometry)
    }

    fn text_pixels(&self, normal: f64, high: f64, ultra: f64) -> f64 {
        self.detail_level
            .pixels(normal, high, ultra, self.custom_detail.text)
    }

    fn detail_budget(&self, normal: usize, high: usize, ultra: usize) -> usize {
        self.detail_level
            .budget(normal, high, ultra, self.custom_detail.budget)
    }

    pub fn set_color_mode(&mut self, cx: &mut Cx, mode: ColorMode) {
        self.color_mode = mode;
        self.tree.compute_heat(mode);
        if mode != ColorMode::FileType && self.history.is_none() {
            self.set_status(cx, MapStatus::ReadingHistory);
        }
        self.redraw(cx);
    }

    pub fn set_search(&mut self, cx: &mut Cx, query: &str) {
        self.search = query.trim().to_string();
        self.matches = self.tree.search(&self.search);
        self.match_cursor = 0;
        if self.search.is_empty() {
            self.set_status(cx, MapStatus::Summary);
        } else {
            self.set_status(cx, MapStatus::Search);
        }
        self.redraw(cx);
    }

    pub fn next_match(&mut self, cx: &mut Cx) {
        if self.matches.is_empty() {
            return;
        }
        let index = self.matches[self.match_cursor % self.matches.len()];
        self.match_cursor += 1;
        self.select(cx, index);
        self.fly_to_node(cx, index);
    }

    pub fn set_3d(&mut self, cx: &mut Cx, on: bool) {
        if on == self.mode_3d || self.cam_scale <= 0.0 {
            self.mode_3d = on;
            self.redraw(cx);
            return;
        }
        // keep looking at the same spot when switching
        let k = self.scale_3d() as f64;
        let tan_half = (self.orbit.fov_y.to_radians() * 0.5).tan() as f64;
        if on {
            let center = self.cam_off + self.view.size * 0.5 / self.cam_scale;
            self.orbit.target = vec3f(((center.x - self.world.w * 0.5) * k) as f32, 0.0, ((center.y - self.world.h * 0.5) * k) as f32);
            let visible = self.view.size.y / self.cam_scale * k;
            self.orbit.distance = (visible / (2.0 * tan_half)) as f32;
        } else {
            let t = self.orbit.target;
            let center = dvec2(t.x as f64 / k + self.world.w * 0.5, t.z as f64 / k + self.world.h * 0.5);
            let visible = self.orbit.distance as f64 * 2.0 * tan_half / k;
            self.cam_scale = self.view.size.y / visible.max(1e-9);
            self.cam_off = center - self.view.size * 0.5 / self.cam_scale;
        }
        self.mode_3d = on;
        self.flight = None;
        self.orbit_flight = None;
        self.redraw(cx);
    }

    pub fn fit(&mut self, cx: &mut Cx) {
        if self.mode_3d {
            self.orbit_flight = Some((vec3f(0.0, 0.0, 0.0), 13.0));
            self.frame = cx.new_next_frame();
        } else {
            let world = self.world;
            self.fly_to(cx, world, 1.0);
        }
    }

    fn fly_to(&mut self, cx: &mut Cx, r: R, fill: f64) {
        if self.view.size.x <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let scale = (self.view.size.x / r.w).min(self.view.size.y / r.h) * fill;
        self.flight = Some((dvec2(r.x + r.w * 0.5, r.y + r.h * 0.5), scale));
        self.frame = cx.new_next_frame();
    }

    fn fly_to_node(&mut self, cx: &mut Cx, index: usize) {
        if self.mode_3d {
            self.fly_3d(cx, index);
        } else {
            let r = self.tree.nodes[index].rect;
            self.fly_to(cx, r, 0.95);
        }
    }

    fn select(&mut self, cx: &mut Cx, index: usize) {
        self.selected = Some(index);
        cx.widget_action(self.uid, CodeMapAction::Selected(self.info(index)));
        self.redraw(cx);
    }

    fn expand(&mut self, cx: &mut Cx, index: usize) {
        let Kind::Ghost { dir, loading: false } = self.tree.nodes[index].kind else { return };
        let Some(tx) = self.tx.clone() else { return };
        self.tree.nodes[index].kind = Kind::Ghost { dir, loading: true };
        let root = self.root.clone();
        let path = self.tree.nodes[index].path.clone();
        self.pending += 1;
        std::thread::spawn(move || {
            let entries = scan::scan_ignored(&root, &path);
            let _ = tx.send(ScanMsg::Expanded(path, entries));
        });
        self.frame = cx.new_next_frame();
        self.redraw(cx);
    }

    fn summary(&self) -> String {
        let root = &self.tree.nodes[0];
        let files = fmt_num(root.total_files);
        let lines = fmt_num(root.total_lines);
        let commits = self
            .history
            .as_ref()
            .map(|history| fmt_num(history.commits_read as u64));
        self.language.summary(&files, &lines, commits.as_deref())
    }

    /// Pick up finished background work.
    fn drain(&mut self, cx: &mut Cx) {
        let Some(rx) = &self.rx else { return };
        let msgs: Vec<ScanMsg> = rx.try_iter().collect();
        for msg in msgs {
            self.pending = self.pending.saturating_sub(1);
            match msg {
                ScanMsg::Project(Ok(project), secs) => {
                    let used_git = project.used_git;
                    for entry in project.entries {
                        self.tree.insert(entry);
                    }
                    self.tree.update_weights(true);
                    self.set_status(cx, MapStatus::Scanned { seconds: secs, used_git });
                    self.needs_layout = true;
                    self.needs_fit = true;
                    if used_git {
                        // git history is slower than the scan, so load it separately
                        if let Some(tx) = self.tx.clone() {
                            let root = self.root.clone();
                            self.pending += 1;
                            std::thread::spawn(move || {
                                let _ = tx.send(ScanMsg::History(history::load(&root)));
                            });
                        }
                    }
                }
                ScanMsg::Project(Err(err), _) => {
                    self.set_status(cx, MapStatus::ScanFailed(err));
                }
                ScanMsg::Expanded(path, entries) => {
                    if let Some(index) = self.tree.find(&path) {
                        let count = entries.len();
                        self.tree.graft(index, entries);
                        self.after_tree_change();
                        self.selected = Some(index);
                        self.set_status(cx, MapStatus::ExpandedIgnored { path, count });
                    }
                }
                ScanMsg::History(Ok(history)) => {
                    self.tree.apply_history(&history);
                    self.history = Some(history);
                    self.tree.compute_heat(self.color_mode);
                    self.set_status(cx, MapStatus::Summary);
                }
                ScanMsg::History(Err(err)) => {
                    self.set_status(cx, MapStatus::HistoryFailed(err));
                }
            }
            self.redraw(cx);
        }
    }

    fn after_tree_change(&mut self) {
        self.needs_layout = true;
        if let Some(history) = &self.history {
            self.tree.apply_history(history);
        }
        self.tree.compute_heat(self.color_mode);
        if !self.search.is_empty() {
            self.matches = self.tree.search(&self.search);
        }
    }

    fn relayout(&mut self) {
        self.tree.update_weights(self.show_ignored);
        if self.world.w <= 0.0 {
            // the world is the size of the first view, so zoom 1.0 is "fit"
            self.world = R { x: 0.0, y: 0.0, w: self.view.size.x.max(100.0), h: self.view.size.y.max(100.0) };
        }
        self.tree.layout(self.world);
        self.needs_layout = false;
    }

    fn to_screen(&self, r: R) -> Rect {
        Rect {
            pos: self.view.pos + (dvec2(r.x, r.y) - self.cam_off) * self.cam_scale,
            size: dvec2(r.w, r.h) * self.cam_scale,
        }
    }

    fn to_world(&self, abs: DVec2) -> DVec2 {
        self.cam_off + (abs - self.view.pos) / self.cam_scale
    }

    fn hit(&self, abs: DVec2) -> Option<usize> {
        if self.tree.is_empty() || self.cam_scale <= 0.0 {
            return None;
        }
        if self.mode_3d {
            return self.pick_3d(abs);
        }
        let p = self.to_world(abs);
        self.tree.hit(p.x, p.y, 3.0 / self.cam_scale)
    }

    fn step_flight(&mut self, cx: &mut Cx) {
        let Some((center, scale)) = self.flight else { return };
        let half = self.view.size * 0.5;
        let now_center = self.cam_off + half / self.cam_scale;
        let k = 0.2;
        // interpolate zoom in log space so zooming in and out feel the same
        let new_scale = (self.cam_scale.ln() + (scale.ln() - self.cam_scale.ln()) * k).exp();
        let new_center = now_center + (center - now_center) * k;
        self.cam_scale = new_scale;
        self.cam_off = new_center - half / new_scale;
        if (new_scale / scale - 1.0).abs() < 0.002 && (new_center - center).length() * new_scale < 0.5 {
            self.cam_scale = scale;
            self.cam_off = center - half / scale;
            self.flight = None;
        } else {
            self.frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }

    fn info(&self, index: usize) -> NodeInfo {
        let n = &self.tree.nodes[index];
        let mut details = match n.kind {
            Kind::Dir => self.language.folder_details(
                &fmt_num(n.total_files),
                &fmt_num(n.total_lines),
                &self.language.bytes(n.bytes),
                n.children.len(),
            ),
            Kind::Text => {
                let comments = n.lines.iter().filter(|l| l.comment).count();
                self.language.text_file_details(
                    &fmt_num(n.lines.len() as u64),
                    &fmt_num(comments as u64),
                    &self.language.bytes(n.bytes),
                )
            }
            Kind::Binary => self.language.binary_details(&self.language.bytes(n.bytes)),
            Kind::Ghost { dir, loading } => self.language.ignored_details(dir, loading),
        };
        if self.history.is_some() && !matches!(n.kind, Kind::Ghost { .. }) {
            if n.commits > 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                details.push_str(&self.language.git_details(
                    &fmt_num(n.commits as u64),
                    &self.language.age(now - n.last_change),
                ));
            } else {
                details.push_str(&self.language.no_commits());
            }
        }
        if n.ignored && !matches!(n.kind, Kind::Ghost { .. }) {
            details.push_str(&self.language.matched_gitignore());
        }
        NodeInfo {
            title: n.name.clone(),
            path: if n.path.is_empty() { self.root.display().to_string() } else { n.path.clone() },
            details,
        }
    }

    fn search_active(&self) -> bool {
        !self.search.is_empty()
    }

    /// Base color of a file for the current color mode.
    fn file_color(&self, index: usize) -> Vec4f {
        let node = &self.tree.nodes[index];
        match self.color_mode {
            ColorMode::FileType => match node.kind {
                Kind::Binary => vec4(0.45, 0.45, 0.47, 1.0),
                _ => hsv(ext_hue(&node.name), 0.5, 0.8),
            },
            _ => heat_color(node.heat),
        }
    }

    fn draw_map(&mut self, cx: &mut Cx2d) {
        let view = self.view;
        let searching = self.search_active();
        let quad_budget = self.detail_budget(400_000, 800_000, 1_200_000);
        let min_node_px = self.geometry_pixels(0.5, 0.2, 0.1);
        let child_px = self.geometry_pixels(4.0, 2.0, 0.75);
        let mut quads = 0usize;
        let mut stack = vec![0usize];
        while let Some(index) = stack.pop() {
            let node = &self.tree.nodes[index];
            if node.weight <= 0.0 {
                continue;
            }
            let r = self.to_screen(node.rect);
            if r.pos.x > view.pos.x + view.size.x
                || r.pos.y > view.pos.y + view.size.y
                || r.pos.x + r.size.x < view.pos.x
                || r.pos.y + r.size.y < view.pos.y
                || (r.size.x < min_node_px && r.size.y < min_node_px)
            {
                continue;
            }
            if quads > quad_budget {
                break;
            }
            // search: things off the path to a match fade into the background
            let fade = if searching && !node.on_path { 0.22 } else { 1.0 };
            let small = r.size.x.min(r.size.y);
            let border = if small > 8.0 { 1.0 } else { 0.0 };
            match node.kind {
                Kind::Dir => {
                    let shade = 0.07 + 0.03 * (node.depth.min(5) as f32);
                    let color = if node.ignored { vec4(shade * 1.3, shade, shade * 0.8, 1.0) } else { vec4(shade * 0.85, shade, shade * 1.25, 1.0) };
                    block(&mut self.draw_block, cx, r, scale_rgb(color, fade), vec4(0.0, 0.0, 0.0, 0.6), border, if node.ignored { 0.4 } else { 0.0 });
                    quads += 1;
                    if small > child_px {
                        stack.extend(node.children.iter().rev());
                        let (_, head) = frame(node.rect);
                        if head * self.cam_scale >= 13.0 && r.size.x > 50.0 && (!searching || node.on_path) {
                            self.labels.push(Label { rect: r, text: node.name.clone(), dim: node.ignored });
                        }
                    }
                }
                Kind::Text => {
                    let base = self.file_color(index);
                    let dark = if node.ignored { 0.12 } else { 0.17 };
                    let bg = scale_rgb(base, dark * fade);
                    let edge = scale_rgb(base, 0.35 * fade);
                    block(&mut self.draw_block, cx, r, bg, edge, border, if node.ignored { 0.3 } else { 0.0 });
                    quads += 1;
                    quads += self.draw_code_lines(cx, index, base, fade);
                }
                Kind::Binary => {
                    let base = self.file_color(index);
                    block(&mut self.draw_block, cx, r, scale_rgb(base, 0.35 * fade), scale_rgb(base, 0.6 * fade), border, 0.5);
                    quads += 1;
                    if r.size.x > 60.0 && r.size.y > 18.0 {
                        self.labels.push(Label { rect: r, text: node.name.clone(), dim: true });
                    }
                }
                Kind::Ghost { loading, .. } => {
                    let color = if loading { vec4(0.20, 0.17, 0.08, 1.0) } else { vec4(0.10, 0.09, 0.08, 1.0) };
                    block(&mut self.draw_block, cx, r, scale_rgb(color, fade), vec4(0.45, 0.38, 0.25, fade), border, 1.0);
                    quads += 1;
                    if r.size.x > 60.0 && r.size.y > 18.0 {
                        let suffix = if loading {
                            self.language.reading_suffix()
                        } else {
                            self.language.ignored_suffix()
                        };
                        let text = format!("{} ({suffix})", node.name);
                        self.labels.push(Label { rect: r, text, dim: true });
                    }
                }
            }
        }
        // outline search matches so they pop out even when small
        if searching {
            for k in 0..self.matches.len().min(5000) {
                let index = self.matches[k];
                let r = self.to_screen(self.tree.nodes[index].rect);
                if r.size.x >= 2.0 && self.tree.nodes[index].weight > 0.0 {
                    block(&mut self.draw_block, cx, r, vec4(0.0, 0.0, 0.0, 0.0), vec4(1.0, 0.85, 0.2, 1.0), 2.0, 0.0);
                }
            }
        }
    }

    /// Returns how many quads were drawn.
    fn draw_code_lines(&mut self, cx: &mut Cx2d, index: usize, base: Vec4f, fade: f32) -> usize {
        let node = &self.tree.nodes[index];
        let line_px = node.line_h * self.cam_scale;
        let r_screen = self.to_screen(node.rect);
        let searching = self.search_active();
        let strip_px = self.text_pixels(0.6, 0.35, 0.2);
        let text_px = self.text_pixels(9.0, 7.0, 6.0);
        let label_budget = self.detail_budget(500, 1_000, 1_500);
        let wants_label = r_screen.size.x > 60.0 && r_screen.size.y > 18.0 && (!searching || node.on_path);
        if line_px < strip_px || node.lines.is_empty() {
            if wants_label && self.labels.len() < label_budget {
                self.labels.push(Label { rect: r_screen, text: node.name.clone(), dim: false });
            }
            return 0;
        }
        let inner = self.to_screen(code_inner(node.rect));
        let cols = node.cols.max(1) as usize;
        let rows = node.lines.len().div_ceil(cols);
        let char_px = line_px * CHAR_W;
        let col_px = COL_CHARS * char_px;
        let view = self.view;
        let text_mode = line_px >= text_px;
        if text_mode {
            self.ensure_text(index);
        }
        let node = &self.tree.nodes[index];
        let code_color = scale_rgb(mix_rgb(base, vec4(1.0, 1.0, 1.0, 1.0), 0.35), 0.85 * fade);
        let text_color = scale_rgb(mix_rgb(base, vec4(1.0, 1.0, 1.0, 1.0), 0.75), fade);
        let comment_color = vec4(0.30 * fade, 0.40 * fade, 0.30 * fade, 1.0);
        let mut drawn = 0;
        // only visit the columns and rows that are actually on screen
        let first_col = ((view.pos.x - inner.pos.x) / col_px).floor().max(0.0) as usize;
        let last_col = ((((view.pos.x + view.size.x) - inner.pos.x) / col_px).ceil().max(0.0) as usize).min(cols);
        let first_row = ((view.pos.y - inner.pos.y) / line_px).floor().max(0.0) as usize;
        let last_row = ((((view.pos.y + view.size.y) - inner.pos.y) / line_px).ceil().max(0.0) as usize).min(rows);
        if text_mode {
            self.draw_code.text_style.font_size = (line_px * 0.6) as f32;
        }
        for col in first_col..last_col {
            for row in first_row..last_row {
                let k = col * rows + row;
                let Some(line) = node.lines.get(k) else { break };
                let x = inner.pos.x + col as f64 * col_px;
                let y = inner.pos.y + row as f64 * line_px;
                if text_mode {
                    if let Some(text) = self.text_cache.get(&index).and_then(|t| t.get(k)) {
                        let clipped: String = text.chars().take(COL_CHARS as usize).collect();
                        self.draw_code.color = if line.comment { scale_rgb(vec4(0.5, 0.65, 0.5, 1.0), fade) } else { text_color };
                        self.draw_code.draw_abs(cx, dvec2(x, y), &clipped);
                    }
                } else if line.len > 0 {
                    let color = if line.comment { comment_color } else { code_color };
                    let w = (line.len as f64 * char_px).min(col_px - line.indent as f64 * char_px).max(0.5);
                    let rect = Rect {
                        pos: dvec2(x + line.indent as f64 * char_px, y + line_px * 0.18),
                        size: dvec2(w, (line_px * 0.64).max(0.5)),
                    };
                    block(&mut self.draw_block, cx, rect, color, vec4(0.0, 0.0, 0.0, 0.0), 0.0, 0.0);
                    drawn += 1;
                }
            }
        }
        if wants_label && self.labels.len() < label_budget {
            self.labels.push(Label { rect: r_screen, text: node.name.clone(), dim: false });
        }
        drawn
    }

    /// Load a file's text for the fully zoomed-in view (cached).
    fn ensure_text(&mut self, index: usize) {
        if self.text_cache.contains_key(&index) {
            return;
        }
        if self.text_cache.len() > 300 {
            self.text_cache.clear();
        }
        let content = std::fs::read(self.root.join(&self.tree.nodes[index].path)).unwrap_or_default();
        let text = String::from_utf8_lossy(&content).into_owned();
        self.text_cache.insert(index, text.lines().map(|l| l.replace('\t', "    ")).collect());
    }

    fn outline(&mut self, cx: &mut Cx2d, index: usize, color: Vec4f, width: f32) {
        let r = self.to_screen(self.tree.nodes[index].rect);
        block(&mut self.draw_block, cx, r, vec4(0.0, 0.0, 0.0, 0.0), color, width, 0.0);
    }

    fn draw_labels(&mut self, cx: &mut Cx2d) {
        let labels = std::mem::take(&mut self.labels);
        let budget = self.detail_budget(500, 1_000, 1_500);
        for label in labels.iter().take(budget) {
            let pos = dvec2(label.rect.pos.x.max(self.view.pos.x) + 4.0, label.rect.pos.y.max(self.view.pos.y) + 3.0);
            let room = label.rect.pos.x + label.rect.size.x - pos.x - 8.0;
            if room < 20.0 {
                continue;
            }
            let max_chars = (room / 6.0) as usize;
            let text: String = if label.text.chars().count() > max_chars {
                label.text.chars().take(max_chars.saturating_sub(1)).chain(['…']).collect()
            } else {
                label.text.clone()
            };
            let w = (text.chars().count() as f64 * 5.6 + 8.0).min(room + 4.0);
            block(&mut self.draw_block, cx, Rect { pos: pos - dvec2(3.0, 1.0), size: dvec2(w, 14.0) }, vec4(0.0, 0.0, 0.0, 0.72), vec4(0.0, 0.0, 0.0, 0.0), 0.0, 0.0);
            self.draw_label.color = if label.dim { vec4(0.75, 0.68, 0.55, 1.0) } else { vec4(0.92, 0.94, 0.96, 1.0) };
            self.draw_label.draw_abs(cx, pos, &text);
        }
        self.labels = labels;
        self.labels.clear();
    }

    fn handle_2d_input(&mut self, cx: &mut Cx, hit: Hit) {
        match hit {
            Hit::FingerMove(e) => {
                if let Some(mut drag) = self.drag {
                    if (e.abs - drag.start).length() > 4.0 {
                        drag.moved = true;
                    }
                    if drag.moved {
                        cx.set_cursor(MouseCursor::Grabbing);
                        self.cam_off = drag.start_off - (e.abs - drag.start) / self.cam_scale;
                        self.redraw(cx);
                    }
                    self.drag = Some(drag);
                }
            }
            Hit::FingerScroll(e) => {
                self.flight = None;
                // zoom around the point under the mouse
                let factor = (-e.scroll.y * 0.01).exp();
                let anchor = self.to_world(e.abs);
                let min_scale = (self.view.size.x / self.world.w).min(self.view.size.y / self.world.h) * 0.5;
                self.cam_scale = (self.cam_scale * factor).clamp(min_scale, min_scale * 200_000.0);
                self.cam_off = anchor - (e.abs - self.view.pos) / self.cam_scale;
                self.hover = self.hit(e.abs);
                self.redraw(cx);
            }
            _ => {}
        }
    }
}

impl Widget for CodeMap {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // begin_turtle clips everything we draw to the widget's own rectangle
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, rect);
        self.view = rect;

        if self.tree.is_empty() {
            self.draw_label.color = vec4(0.7, 0.7, 0.7, 1.0);
            let message = if self.message.is_empty() {
                self.language.no_folder().to_string()
            } else {
                self.message.clone()
            };
            self.draw_label.draw_abs(cx, rect.pos + dvec2(16.0, 16.0), &message);
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        }
        if self.needs_layout {
            self.relayout();
        }
        if self.needs_fit && rect.size.x > 0.0 {
            self.cam_scale = (rect.size.x / self.world.w).min(rect.size.y / self.world.h);
            let half = rect.size * 0.5;
            self.cam_off = dvec2(self.world.w * 0.5, self.world.h * 0.5) - half / self.cam_scale;
            self.needs_fit = false;
        }

        self.labels.clear();
        if self.mode_3d {
            self.draw_3d(cx, rect);
        } else {
            self.draw_map(cx);
            if let Some(h) = self.hover {
                if h < self.tree.nodes.len() {
                    self.outline(cx, h, vec4(1.0, 1.0, 1.0, 0.5), 1.5);
                }
            }
            if let Some(s) = self.selected {
                if s < self.tree.nodes.len() {
                    self.outline(cx, s, vec4(0.35, 0.85, 1.0, 1.0), 2.5);
                }
            }
        }
        self.draw_labels(cx);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.frame.is_event(event).is_some() {
            self.drain(cx);
            self.step_flight(cx);
            self.step_orbit_flight(cx);
            if self.pending > 0 {
                self.frame = cx.new_next_frame();
            }
        }
        let hit = event.hits(cx, self.area);
        match &hit {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hover = self.hit(e.abs);
                self.hover_abs = e.abs;
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(e) => {
                self.flight = None;
                self.orbit_flight = None;
                self.drag = Some(Drag {
                    start: e.abs,
                    last: e.abs,
                    start_off: self.cam_off,
                    moved: false,
                    taps: e.tap_count,
                    pan: !e.device.is_primary_hit() || e.modifiers.shift,
                });
            }
            Hit::FingerUp(e) => {
                cx.set_cursor(MouseCursor::Default);
                if let Some(drag) = self.drag.take() {
                    if !drag.moved {
                        if let Some(index) = self.hit(e.abs) {
                            self.select(cx, index);
                            if matches!(self.tree.nodes[index].kind, Kind::Ghost { .. }) {
                                self.expand(cx, index);
                            } else if drag.taps >= 2 {
                                self.fly_to_node(cx, index);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if self.cam_scale <= 0.0 {
            return;
        }
        if self.mode_3d {
            self.handle_3d_input(cx, hit);
        } else {
            self.handle_2d_input(cx, hit);
        }
    }
}

// `CodeMapRef` is generated by `#[derive(Widget)]`: a handle the app uses to
// reach this widget from outside.
impl CodeMapRef {
    pub fn open(&self, cx: &mut Cx, root: PathBuf) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx, root);
        }
    }
    pub fn set_show_ignored(&self, cx: &mut Cx, show: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_show_ignored(cx, show);
        }
    }
    pub fn fit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.fit(cx);
        }
    }
    pub fn set_color_mode(&self, cx: &mut Cx, mode: ColorMode) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color_mode(cx, mode);
        }
    }
    pub fn set_detail_level(&self, cx: &mut Cx, level: DetailLevel) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_detail_level(cx, level);
        }
    }
    pub fn set_custom_detail(&self, cx: &mut Cx, detail: CustomDetail) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_custom_detail(cx, detail);
        }
    }
    pub fn set_language(&self, cx: &mut Cx, language: Language) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_language(cx, language);
        }
    }
    pub fn set_search(&self, cx: &mut Cx, query: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_search(cx, query);
        }
    }
    pub fn next_match(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.next_match(cx);
        }
    }
    pub fn set_3d(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_3d(cx, on);
        }
    }
}

fn block(draw: &mut DrawBlock, cx: &mut Cx2d, rect: Rect, color: Vec4f, edge: Vec4f, border: f32, hatch: f32) {
    draw.color = color;
    draw.edge = edge;
    draw.border = border;
    draw.hatch = hatch;
    draw.draw_abs(cx, rect);
}

/// A stable color per file extension, with hand-picked ones for common types.
fn ext_hue(name: &str) -> f32 {
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "rs" => 0.07,
        "clj" | "cljc" | "edn" => 0.33,
        "cljs" => 0.45,
        "js" | "mjs" | "jsx" => 0.14,
        "ts" | "tsx" => 0.58,
        "css" | "scss" => 0.78,
        "html" => 0.02,
        "json" | "toml" | "yaml" | "yml" => 0.5,
        "md" | "txt" => 0.62,
        _ => {
            let h = ext.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
            (h % 1000) as f32 / 1000.0
        }
    }
}

/// Heatmap ramp: cold blue, through magenta and orange, to hot yellow.
/// Grey when a file has no git history.
fn heat_color(heat: f32) -> Vec4f {
    if heat < 0.0 {
        return vec4(0.30, 0.30, 0.32, 1.0);
    }
    let stops = [(0.10, 0.22, 0.60), (0.62, 0.22, 0.62), (0.98, 0.45, 0.18), (1.0, 0.92, 0.35)];
    let t = heat.clamp(0.0, 1.0) * 3.0;
    let i = (t.floor() as usize).min(2);
    let f = t - i as f32;
    let (a, b) = (stops[i], stops[i + 1]);
    vec4(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f, 1.0)
}

fn hsv(h: f32, s: f32, v: f32) -> Vec4f {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    vec4(f(5.0), f(3.0), f(1.0), 1.0)
}

fn scale_rgb(c: Vec4f, k: f32) -> Vec4f {
    vec4(c.x * k, c.y * k, c.z * k, c.w)
}

fn mix_rgb(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w)
}

pub fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{CustomDetail, DetailLevel};

    #[test]
    fn custom_detail_interpolates_thresholds_and_scales_budgets() {
        assert_eq!(DetailLevel::Custom.pixels(10.0, 5.0, 2.0, 0.0), 10.0);
        assert_eq!(DetailLevel::Custom.pixels(10.0, 5.0, 2.0, 100.0), 2.0);
        assert_eq!(DetailLevel::Custom.budget(500, 1_000, 1_500, 250.0), 1_250);
    }

    #[test]
    fn custom_detail_clamps_unsafe_external_values() {
        assert_eq!(
            CustomDetail::new(-5.0, 130.0, 2_000.0),
            CustomDetail {
                geometry: 0.0,
                text: 100.0,
                budget: 1_000.0,
            }
        );
    }
}
