//! The 3D city view. The same treemap layout, but every folder is a plate
//! stacked on its parent and every file is a tower on top of its folder.
//! Tower height: size of the file (File type mode) or git activity (heatmap modes).
//!
//! How it renders: we draw into our own offscreen render pass with a depth
//! buffer (so near boxes hide far ones, like WebGL depth testing), using
//! Makepad's built-in instanced `DrawCube`. The pass output is a texture,
//! which we then show as a normal 2D quad, with 2D labels drawn on top.
//!
//! This is a child module of `map_view`, so it can use the widget's private fields.

use super::*;
use crate::orbit::{ray_box, Frame};

/// Width of the whole map in 3D units.
const CITY_SIZE: f32 = 10.0;
/// Thickness of one folder plate.
const PLATE: f32 = 0.035;

/// A file tower whose top is close enough to show code on it.
struct CodePanel {
    index: usize,
    /// Screen pixels per line of code at this tower.
    line_px: f32,
    /// World position of the top face's corner (layout x/y minimum).
    corner: Vec3f,
}

impl CodeMap {
    /// World units (2D layout) to 3D units.
    pub(super) fn scale_3d(&self) -> f32 {
        CITY_SIZE / self.world.w.max(1.0) as f32
    }

    fn ensure_pass(&mut self, cx: &mut Cx) {
        if self.pass_ready {
            return;
        }
        self.pass_ready = true;
        self.color_texture = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true });
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
    }

    pub(super) fn draw_3d(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.ensure_pass(cx.cx);
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(cx, &self.color_texture, DrawPassClearColor::ClearWith(vec4(0.027, 0.035, 0.047, 1.0)));
        self.pass.set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        let frame = self.orbit.frame(rect);
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        set_pass_camera(cx.cx, &self.pass, &frame);
        let mut label_spots = Vec::new();
        {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_list.begin_always(cx3d);
            cx3d.begin_scene_3d(SceneState3D {
                time: 0.0,
                camera_pos: frame.eye,
                view: frame.view,
                projection: frame.projection,
                viewport_rect: rect,
            });
            let previous = cx3d.set_scene_world_transform_3d(Mat4f::identity());
            self.draw_city(cx3d, &frame, &mut label_spots);
            if let Some(previous) = previous {
                cx3d.set_scene_world_transform_3d(previous);
            }
            cx3d.end_scene_3d();
            self.draw_list.end(cx3d);
        }
        cx.end_pass(&self.pass);

        self.draw_scene.draw_super.draw_vars.set_texture(0, &self.color_texture);
        self.draw_scene.draw_abs(cx, rect);
        cx.set_pass_area(&self.pass, self.draw_scene.draw_super.draw_vars.area);

        // biggest folders get labels first
        label_spots.sort_by(|a: &(f32, usize, Vec3f), b| b.0.total_cmp(&a.0));
        let label_budget = self.detail_budget(120, 240, 400);
        for (px, index, point) in label_spots.into_iter().take(label_budget) {
            if let Some(p) = frame.project(point) {
                let width = (px as f64).clamp(40.0, 260.0);
                self.labels.push(Label {
                    rect: Rect { pos: p - dvec2(width * 0.5, 8.0), size: dvec2(width, 16.0) },
                    text: self.tree.nodes[index].name.clone(),
                    dim: self.tree.nodes[index].ignored,
                });
            }
        }
        if let Some(h) = self.hover {
            if h < self.tree.nodes.len() {
                self.labels.push(Label {
                    rect: Rect { pos: self.hover_abs + dvec2(14.0, 12.0), size: dvec2(320.0, 16.0) },
                    text: self.tree.nodes[h].path.clone(),
                    dim: false,
                });
            }
        }
    }

    fn draw_city(&mut self, cx: &mut Cx3d, frame: &Frame, label_spots: &mut Vec<(f32, usize, Vec3f)>) {
        let k = self.scale_3d();
        let (ox, oz) = (self.world.w as f32 * 0.5, self.world.h as f32 * 0.5);
        let searching = self.search_active();
        let cube_budget = self.detail_budget(250_000, 350_000, 500_000);
        let min_px = self.geometry_pixels(1.0, 0.35, 0.15) as f32;
        let children_px = self.geometry_pixels(10.0, 4.0, 1.5) as f32;
        let roof_px = self.geometry_pixels(40.0, 20.0, 10.0) as f32;
        let label_px = self.geometry_pixels(90.0, 45.0, 25.0) as f32;
        let mut panels: Vec<CodePanel> = Vec::new();
        self.picks.clear();
        self.draw_cube.transform = Mat4f::identity();
        self.draw_cube.depth_clip = 0.0;
        self.draw_cube.begin_many_instances(cx);

        // (node, height of the surface it stands on)
        let mut stack: Vec<(usize, f32)> = vec![(0, 0.0)];
        while let Some((index, base)) = stack.pop() {
            let node = &self.tree.nodes[index];
            if node.weight <= 0.0 {
                continue;
            }
            if self.picks.len() >= cube_budget {
                break;
            }
            let r = node.rect;
            let (x, z, w, d) = ((r.x as f32 - ox) * k, (r.y as f32 - oz) * k, r.w as f32 * k, r.h as f32 * k);
            let footprint = w.max(d);
            let height = match node.kind {
                Kind::Dir => PLATE,
                Kind::Ghost { .. } => PLATE * 0.6,
                Kind::Text | Kind::Binary => {
                    let h = match self.color_mode {
                        ColorMode::FileType => 0.04 + 0.5 * ((node.lines.len() as f32 + 1.0).log10() / 5.0).min(1.0),
                        _ if node.heat < 0.0 => 0.02,
                        _ => 0.04 + 1.1 * node.heat * node.heat,
                    };
                    // no needles: a tiny file can't be a skyscraper
                    h.min(footprint * 3.0).max(0.002)
                }
            };
            let center = vec3f(x + w * 0.5, base + height * 0.5, z + d * 0.5);
            let radius = 0.5 * (w * w + d * d + height * height).sqrt();
            if !frame.sphere_visible(center, radius) {
                continue;
            }
            let px = frame.pixels_for(center, footprint);
            if px < min_px {
                continue;
            }
            let is_dir = node.kind == Kind::Dir;
            // small gap between files so neighbours read as separate towers
            let inset = if is_dir { 0.0 } else { w.min(d) * 0.08 };
            let min = vec3f(x + inset, base, z + inset);
            let max = vec3f(x + w - inset, base + height, z + d - inset);

            let mut color = if is_dir {
                let hue = (self.tree.top_level(index) as f32 * 0.618_034).fract();
                hsv(hue, 0.35, 0.16 + 0.05 * node.depth.min(6) as f32)
            } else if let Kind::Ghost { loading, .. } = node.kind {
                if loading { vec4(0.5, 0.4, 0.15, 1.0) } else { vec4(0.22, 0.2, 0.17, 1.0) }
            } else {
                self.file_color(index)
            };
            if node.ignored {
                color = scale_rgb(color, 0.6);
            }
            if searching {
                if !node.on_path {
                    color = scale_rgb(color, 0.15);
                } else if node.lit && !is_dir {
                    color = mix_rgb(color, vec4(1.0, 0.9, 0.3, 1.0), 0.5);
                }
            }
            if self.selected == Some(index) {
                color = mix_rgb(color, vec4(0.35, 0.85, 1.0, 1.0), 0.6);
            } else if self.hover == Some(index) {
                color = mix_rgb(color, vec4(1.0, 1.0, 1.0, 1.0), 0.25);
            }

            self.draw_cube.color = color;
            self.draw_cube.cube_pos = vec3f((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, (min.z + max.z) * 0.5);
            self.draw_cube.cube_size = max - min;
            self.draw_cube.draw(cx);
            self.picks.push((index, min, max));

            if node.kind == Kind::Text && !node.lines.is_empty() && px > roof_px {
                // measure from the closest point of the roof: using the tower's
                // center breaks down when the camera hovers right above it
                let roof = base + height;
                let eye = frame.eye;
                let nearest = vec3f(eye.x.clamp(x, x + w), roof, eye.z.clamp(z, z + d));
                let line_px = frame.pixels_at_distance((nearest - eye).length(), node.line_h as f32 * k);
                if line_px >= self.text_pixels(0.6, 0.35, 0.2) as f32 {
                    // lift the code a hair above the roof so it doesn't flicker (z-fighting)
                    let lift = (center - frame.eye).length() * 0.001;
                    panels.push(CodePanel { index, line_px, corner: vec3f(x, base + height + lift, z) });
                }
            }
            if is_dir && px > children_px {
                let top = base + height;
                stack.extend(node.children.iter().rev().map(|&c| (c, top)));
                if px > label_px && (!searching || node.on_path) {
                    label_spots.push((px, index, vec3f(center.x, top, center.z)));
                }
            }
        }
        self.draw_cube.end_many_instances(cx);

        panels.sort_by(|a, b| b.line_px.total_cmp(&a.line_px));
        panels.truncate(self.detail_budget(48, 96, 160));
        self.draw_code_panels(cx, panels);
    }

    /// Draw code on top of towers. Each panel is ordinary 2D drawing (the same
    /// strips and text as the flat view) into its own draw list, and the list's
    /// view transform matrix lays that 2D plane onto the roof in 3D.
    /// Makepad's XR mode puts whole UI panels into 3D space the same way.
    fn draw_code_panels(&mut self, cx: &mut Cx3d, panels: Vec<CodePanel>) {
        let k = self.scale_3d() as f64;
        let mut text_budget = self.detail_budget(12_000, 24_000, 48_000);
        let mut strip_budget = self.detail_budget(250_000, 500_000, 750_000);
        for (slot, panel) in panels.iter().enumerate() {
            while self.code_lists.len() <= slot {
                self.code_lists.push(DrawList::new(cx.cx));
            }
            let text_mode = panel.line_px >= self.text_pixels(9.0, 7.0, 6.0) as f32 && text_budget > 0;
            if text_mode {
                self.ensure_text(panel.index);
            }
            let node = &self.tree.nodes[panel.index];
            // Choose local units so one line is about as many units as it has
            // screen pixels: text is laid out at a size that stays sharp.
            let local_line = (panel.line_px as f64).clamp(6.0, 48.0);
            let s = node.line_h / local_line; // layout units per local unit
            let r = node.rect;
            let size = dvec2(r.w / s, r.h / s);
            let sk = (s * k) as f32;
            // column-major 4x4: local x -> world X, local y -> world Z, local z ignored
            let matrix = Mat4f {
                v: [sk, 0.0, 0.0, 0.0, 0.0, 0.0, sk, 0.0, 0.0, 0.0, 0.0, 0.0, panel.corner.x, panel.corner.y, panel.corner.z, 1.0],
            };

            let cx2d = &mut Cx2d::new(cx.cx);
            let list = &mut self.code_lists[slot];
            list.begin_always(cx2d);
            list.set_view_transform(cx2d, &matrix);
            cx2d.begin_root_turtle(size, Layout::flow_down());

            let inner = code_inner(r);
            let origin = dvec2((inner.x - r.x) / s, (inner.y - r.y) / s);
            let cols = node.cols.max(1) as usize;
            let rows = node.lines.len().div_ceil(cols);
            let char_w = local_line * CHAR_W;
            let col_w = COL_CHARS * char_w;
            let base = self.file_color(panel.index);
            let code_color = mix_rgb(base, vec4(1.0, 1.0, 1.0, 1.0), 0.35);
            let text_color = mix_rgb(base, vec4(1.0, 1.0, 1.0, 1.0), 0.8);
            let comment_color = vec4(0.35, 0.48, 0.35, 1.0);
            if text_mode {
                self.draw_code.text_style.font_size = (local_line * 0.6) as f32;
            }
            let lines = if text_mode { node.lines.len().min(self.detail_budget(3_000, 6_000, 9_000)) } else { node.lines.len().min(strip_budget) };
            if !text_mode {
                strip_budget -= lines;
            }
            for i in 0..lines {
                let line = node.lines[i];
                let x = origin.x + (i / rows) as f64 * col_w;
                let y = origin.y + (i % rows) as f64 * local_line;
                if text_mode {
                    if text_budget == 0 {
                        break;
                    }
                    if let Some(text) = self.text_cache.get(&panel.index).and_then(|t| t.get(i)) {
                        if !text.trim().is_empty() {
                            let clipped: String = text.chars().take(COL_CHARS as usize).collect();
                            self.draw_code.color = if line.comment { comment_color } else { text_color };
                            self.draw_code.draw_abs(cx2d, dvec2(x, y), &clipped);
                            text_budget -= 1;
                        }
                    }
                } else if line.len > 0 {
                    let w = (line.len as f64 * char_w).min(col_w - line.indent as f64 * char_w).max(0.5);
                    let rect = Rect {
                        pos: dvec2(x + line.indent as f64 * char_w, y + local_line * 0.18),
                        size: dvec2(w, local_line * 0.64),
                    };
                    let color = if line.comment { comment_color } else { code_color };
                    block(&mut self.draw_block, cx2d, rect, color, vec4(0.0, 0.0, 0.0, 0.0), 0.0, 0.0);
                }
            }

            cx2d.end_pass_sized_turtle();
            self.code_lists[slot].end(cx2d);
        }
    }

    /// The box under the mouse: closest hit along the ray.
    pub(super) fn pick_3d(&self, abs: DVec2) -> Option<usize> {
        let frame = self.orbit.frame(self.view);
        let (origin, dir) = frame.ray(abs);
        let mut best: Option<(f32, usize)> = None;
        for &(index, min, max) in &self.picks {
            if let Some(t) = ray_box(origin, dir, min, max) {
                if best.is_none_or(|(bt, _)| t < bt) {
                    best = Some((t, index));
                }
            }
        }
        best.map(|(_, index)| index)
    }

    pub(super) fn fly_3d(&mut self, cx: &mut Cx, index: usize) {
        let k = self.scale_3d();
        let r = self.tree.nodes[index].rect;
        let target = vec3f(
            ((r.x + r.w * 0.5) as f32 - self.world.w as f32 * 0.5) * k,
            0.0,
            ((r.y + r.h * 0.5) as f32 - self.world.h as f32 * 0.5) * k,
        );
        let size = (r.w.max(r.h) as f32) * k;
        let distance = size / (2.0 * (self.orbit.fov_y.to_radians() * 0.5).tan()) * 1.4;
        self.orbit_flight = Some((target, distance.max(0.05)));
        self.frame = cx.new_next_frame();
    }

    pub(super) fn step_orbit_flight(&mut self, cx: &mut Cx) {
        let Some((target, distance)) = self.orbit_flight else { return };
        let t = 0.18;
        self.orbit.target = self.orbit.target + (target - self.orbit.target) * t;
        self.orbit.distance = (self.orbit.distance.ln() + (distance.ln() - self.orbit.distance.ln()) * t).exp();
        let close = (self.orbit.target - target).length() < distance * 0.002 && (self.orbit.distance / distance - 1.0).abs() < 0.002;
        if close {
            self.orbit.target = target;
            self.orbit.distance = distance;
            self.orbit_flight = None;
        } else {
            self.frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }

    pub(super) fn handle_3d_input(&mut self, cx: &mut Cx, hit: Hit) {
        match hit {
            Hit::FingerMove(e) => {
                if let Some(mut drag) = self.drag {
                    if (e.abs - drag.start).length() > 4.0 {
                        drag.moved = true;
                    }
                    if drag.moved {
                        let delta = e.abs - drag.last;
                        if drag.pan {
                            self.orbit.pan(delta, self.view);
                        } else {
                            // drag sideways to spin, up and down to tilt
                            self.orbit.yaw -= delta.x as f32 * 0.008;
                            self.orbit.pitch = (self.orbit.pitch - delta.y as f32 * 0.006).clamp(-1.55, -0.08);
                        }
                        cx.set_cursor(MouseCursor::Grabbing);
                        self.redraw(cx);
                    }
                    drag.last = e.abs;
                    self.drag = Some(drag);
                }
            }
            Hit::FingerScroll(e) => {
                // zoom toward the point on the ground under the mouse
                let frame = self.orbit.frame(self.view);
                let (origin, dir) = frame.ray(e.abs);
                let old = self.orbit.distance;
                let new = (old * (e.scroll.y as f32 * 0.01).exp()).clamp(0.02, 60.0);
                if dir.y < -0.01 {
                    let t = -origin.y / dir.y;
                    let ground = origin + dir * t;
                    self.orbit.target = ground + (self.orbit.target - ground) * (new / old);
                }
                self.orbit.distance = new;
                self.redraw(cx);
            }
            _ => {}
        }
    }
}

fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, frame: &Frame) {
    let camera_inv = frame.view.invert();
    let u = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    u.camera_projection = frame.projection;
    u.camera_projection_r = frame.projection;
    u.camera_view = frame.view;
    u.camera_view_r = frame.view;
    u.depth_projection = frame.projection;
    u.depth_projection_r = frame.projection;
    u.depth_view = frame.view;
    u.depth_view_r = frame.view;
    u.camera_inv = camera_inv;
    u.camera_inv_r = camera_inv;
}
