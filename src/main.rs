//! Code Map: fly over a codebase as a zoomable GPU treemap.
//!
//!   cargo run --release -- /path/to/project [--3d]
//!   cargo run --release --bin scan -- /path/to/project   (no window, just stats)

pub use makepad_widgets;

mod history;
mod i18n;
mod map_view;
mod model;
mod orbit;
mod scan;

use makepad_widgets::*;
use i18n::Language;
use map_view::{CodeMapAction, CodeMapWidgetRefExt, CustomDetail, DetailLevel};
use model::ColorMode;
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let InfoLabel = Label{
        width: Fill
        draw_text +: {color: #xaab4be text_style +: {font_size: 9.0}}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x07090c
                body +: {
                    flow: Down
                    toolbar := View{
                        width: Fill height: 40
                        flow: Right spacing: 14
                        padding: Inset{left: 12, right: 12}
                        align: Align{y: 0.5}
                        show_bg: true
                        draw_bg +: {color: #x12161c}
                        title := Label{
                            text: "Code Map"
                            draw_text +: {color: #xffffff text_style +: {font_size: 10.0}}
                        }
                        search := TextInput{
                            width: 220 height: 28
                            empty_text: "Search files and folders"
                        }
                        color_mode := DropDown{
                            width: 150
                            labels: ["File type" "Recently changed" "Most changed"]
                            popup_menu_position: #(makepad_widgets::drop_down::PopupMenuPosition::BelowInput)
                        }
                        three_d := CheckBox{text: "3D" active: false}
                        detail_level := DropDown{
                            width: 110
                            labels: ["Normal" "High" "Ultra" "Custom"]
                            selected_item: 0
                            popup_menu_position: #(makepad_widgets::drop_down::PopupMenuPosition::BelowInput)
                        }
                        language := DropDown{
                            width: 100
                            labels: ["English" "简体中文"]
                            selected_item: 0
                            popup_menu_position: #(makepad_widgets::drop_down::PopupMenuPosition::BelowInput)
                        }
                        fit_button := Button{text: "Fit"}
                        show_ignored := CheckBox{text: "Show ignored" active: true}
                        status := Label{
                            width: Fill
                            text: ""
                            draw_text +: {color: #x8a949e text_style +: {font_size: 9.0}}
                        }
                    }
                    View{
                        width: Fill height: Fill
                        flow: Right
                        map := CodeMap{}
                        inspector := View{
                            width: 290 height: Fill
                            flow: Down spacing: 8
                            padding: Inset{left: 14, right: 14, top: 14, bottom: 14}
                            show_bg: true
                            draw_bg +: {color: #x12161c}
                            inspector_heading := Label{
                                text: "INSPECTOR"
                                draw_text +: {color: #x5a6570 text_style +: {font_size: 8.0}}
                            }
                            custom_detail_panel := View{
                                visible: false
                                width: Fill height: Fit
                                flow: Down spacing: 5
                                padding: Inset{top: 4, bottom: 8}
                                custom_detail_heading := Label{
                                    text: "CUSTOM DETAIL"
                                    draw_text +: {color: #x5a6570 text_style +: {font_size: 8.0}}
                                }
                                custom_geometry := Slider{
                                    width: Fill
                                    text: "Geometry detail"
                                    min: 0 max: 100 default: 70 step: 1 precision: 0
                                }
                                custom_text := Slider{
                                    width: Fill
                                    text: "Text detail"
                                    min: 0 max: 100 default: 70 step: 1 precision: 0
                                }
                                custom_budget := Slider{
                                    width: Fill
                                    text: "Render budget (%)"
                                    min: 50 max: 400 default: 200 step: 10 precision: 0
                                }
                            }
                            info_title := Label{
                                width: Fill
                                text: "Click something on the map"
                                draw_text +: {color: #xffffff text_style +: {font_size: 11.0}}
                            }
                            info_path := InfoLabel{text: ""}
                            info_details := InfoLabel{text: ""}
                            View{width: Fill height: Fill}
                            help := InfoLabel{
                                text: "Scroll: zoom\nDrag: pan (2D) or orbit (3D)\nShift or right drag: pan (3D)\nClick: inspect\nDouble click: fly to it\nEnter in search: next match\nStriped boxes are ignored by git.\nClick one to read it."
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolbarDropdown {
    ColorMode,
    DetailLevel,
    Language,
}

#[derive(Default)]
struct DropdownReleaseGuard {
    opening: Option<(ToolbarDropdown, usize, f64, DVec2)>,
    // Backstop for sweep capture transferred to a popup row during MouseMove.
    // In that case redirecting MouseUp alone cannot stop the row's action.
    quick_selection: Option<(ToolbarDropdown, usize)>,
}

impl DropdownReleaseGuard {
    const QUICK_RELEASE_SECS: f64 = 0.2;

    fn press(
        &mut self,
        dropdown: Option<(ToolbarDropdown, usize)>,
        now: f64,
        abs: DVec2,
    ) {
        // A new physical press starts a separate gesture, including clicks
        // on popup rows, and must never be debounced as part of opening.
        self.quick_selection = None;
        self.opening = dropdown.map(|(kind, selected)| (kind, selected, now, abs));
    }

    fn quick_release_origin(&mut self, now: f64) -> Option<DVec2> {
        let (kind, selected, opened_at, origin) = self.opening.take()?;
        if now >= opened_at && now - opened_at < Self::QUICK_RELEASE_SECS {
            // Prefer to prevent accidental selection before widget dispatch.
            // If MouseMove already transferred capture to an item, the item
            // can still emit Select on MouseUp, so retain a fallback as well.
            self.quick_selection = Some((kind, selected));
            Some(origin)
        } else {
            None
        }
    }

    fn take_accidental_selection(&mut self, kind: ToolbarDropdown) -> Option<usize> {
        if self.quick_selection.map(|(pending, _)| pending) == Some(kind) {
            self.quick_selection.take().map(|(_, selected)| selected)
        } else {
            None
        }
    }

    fn clear_pending(&mut self) {
        self.quick_selection = None;
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    language: Language,
    #[rust]
    has_selection: bool,
    #[rust]
    dropdown_release_guard: DropdownReleaseGuard,
    // A newly opened popup may create its rows during its first draw.
    #[rust]
    dropdown_redraws_remaining: u8,
}

/// The folder to map: first command line argument, or the current folder.
fn project_path() -> PathBuf {
    let arg = std::env::args().skip(1).find(|a| !a.starts_with("--"));
    let path = arg.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    path.canonicalize().unwrap_or(path)
}

impl App {
    fn restore_open_dropdown(&mut self, cx: &mut Cx, kind: ToolbarDropdown, selected: usize) {
        let path = match kind {
            ToolbarDropdown::ColorMode => ids!(color_mode),
            ToolbarDropdown::DetailLevel => ids!(detail_level),
            ToolbarDropdown::Language => ids!(language),
        };
        let dropdown = self.ui.drop_down(cx, path);
        dropdown.set_selected_item(cx, selected);
        if let Some(mut inner) = dropdown.borrow_mut() {
            // DropDown has already closed the popup when it emits Select.
            // Restore its open state as well as the selection.
            inner.set_active(cx);
        };
        self.dropdown_redraws_remaining = 1;
        cx.redraw_all();
    }

    fn toolbar_dropdown_at(&self, cx: &mut Cx, abs: DVec2) -> Option<ToolbarDropdown> {
        [
            (ToolbarDropdown::ColorMode, ids!(color_mode)),
            (ToolbarDropdown::DetailLevel, ids!(detail_level)),
            (ToolbarDropdown::Language, ids!(language)),
        ]
        .into_iter()
        .find_map(|(kind, path)| {
            self.ui
                .widget(cx, path)
                .area()
                .rect(cx)
                .contains(abs)
                .then_some(kind)
        })
    }


    fn custom_detail(&self, cx: &mut Cx) -> CustomDetail {
        CustomDetail::new(
            self.ui
                .slider(cx, ids!(custom_geometry))
                .value()
                .unwrap_or(70.0),
            self.ui
                .slider(cx, ids!(custom_text))
                .value()
                .unwrap_or(70.0),
            self.ui
                .slider(cx, ids!(custom_budget))
                .value()
                .unwrap_or(200.0),
        )
    }

    fn apply_i18n(&mut self, cx: &mut Cx) {
        let language = self.language;
        self.ui
            .text_input(cx, ids!(search))
            .set_empty_text(cx, language.search_placeholder());
        self.ui.drop_down(cx, ids!(color_mode)).set_labels(
            cx,
            language.color_modes().into_iter().collect(),
        );
        self.ui.drop_down(cx, ids!(detail_level)).set_labels(
            cx,
            language.detail_levels().into_iter().collect(),
        );
        self.ui.drop_down(cx, ids!(language)).set_labels(
            cx,
            language.language_names().into_iter().collect(),
        );
        self.ui.button(cx, ids!(fit_button)).set_text(cx, &language.fit());
        self.ui
            .check_box(cx, ids!(show_ignored))
            .set_text(&language.show_ignored());
        self.ui
            .label(cx, ids!(inspector_heading))
            .set_text(cx, &language.inspector());
        self.ui
            .label(cx, ids!(custom_detail_heading))
            .set_text(cx, &language.custom_detail());
        // Pass owned strings to Makepad; a borrowed &str would require 'static.
        // Update only the caption, never the slider value.
        let label = language.geometry_detail();
        let mut slider = self.ui.widget(cx, ids!(custom_geometry));
        script_apply_eval!(cx, slider, {text: #(label)});

        let label = language.text_detail();
        let mut slider = self.ui.widget(cx, ids!(custom_text));
        script_apply_eval!(cx, slider, {text: #(label)});

        let label = language.render_budget();
        let mut slider = self.ui.widget(cx, ids!(custom_budget));
        script_apply_eval!(cx, slider, {text: #(label)});
        if !self.has_selection {
            self.ui
                .label(cx, ids!(info_title))
                .set_text(cx, &language.select_hint());
        }
        self.ui
            .label(cx, ids!(help))
            .set_text(cx, &language.help());
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.language = Language::detect();
        self.ui
            .drop_down(cx, ids!(language))
            .set_selected_item(cx, self.language.index());
        self.apply_i18n(cx);
        let path = project_path();
        self.ui.label(cx, ids!(title)).set_text(cx, &format!("Code Map: {}", path.display()));
        let map = self.ui.code_map(cx, ids!(map));
        map.set_language(cx, self.language);
        map.open(cx, path);
        if std::env::args().any(|a| a == "--3d") {
            self.ui.check_box(cx, ids!(three_d)).set_active(cx, true, Animate::No);
            map.set_3d(cx, true);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let map = self.ui.code_map(cx, ids!(map));
        if let Some(index) = self.ui.drop_down(cx, ids!(language)).changed(actions) {
            if let Some(selected) = self.dropdown_release_guard
                .take_accidental_selection(ToolbarDropdown::Language)
            {
                self.restore_open_dropdown(cx, ToolbarDropdown::Language, selected);
            } else {
                self.language = Language::from_index(index);
                self.apply_i18n(cx);
                map.set_language(cx, self.language);
            }
        }
        if self.ui.button(cx, ids!(fit_button)).clicked(actions) {
            map.fit(cx);
        }
        if let Some(show) = self.ui.check_box(cx, ids!(show_ignored)).changed(actions) {
            map.set_show_ignored(cx, show);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(three_d)).changed(actions) {
            map.set_3d(cx, on);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(detail_level)).changed(actions) {
            if let Some(selected) = self.dropdown_release_guard
                .take_accidental_selection(ToolbarDropdown::DetailLevel)
            {
                self.restore_open_dropdown(cx, ToolbarDropdown::DetailLevel, selected);
            } else {
                let level = match index {
                    1 => DetailLevel::High,
                    2 => DetailLevel::Ultra,
                    3 => DetailLevel::Custom,
                    _ => DetailLevel::Normal,
                };
                let custom = level == DetailLevel::Custom;
                self.ui
                    .view(cx, ids!(custom_detail_panel))
                    .set_visible(cx, custom);
                if custom {
                    let detail = self.custom_detail(cx);
                    map.set_custom_detail(cx, detail);
                }
                map.set_detail_level(cx, level);
            }
        }
        let custom_changed = self
            .ui
            .slider(cx, ids!(custom_geometry))
            .slided(actions)
            .is_some()
            || self
                .ui
                .slider(cx, ids!(custom_text))
                .slided(actions)
                .is_some()
            || self
                .ui
                .slider(cx, ids!(custom_budget))
                .slided(actions)
                .is_some();
        if custom_changed {
            let detail = self.custom_detail(cx);
            map.set_custom_detail(cx, detail);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(color_mode)).changed(actions) {
            if let Some(selected) = self.dropdown_release_guard
                .take_accidental_selection(ToolbarDropdown::ColorMode)
            {
                self.restore_open_dropdown(cx, ToolbarDropdown::ColorMode, selected);
            } else {
                let mode = match index {
                    1 => ColorMode::Recent,
                    2 => ColorMode::Churn,
                    _ => ColorMode::FileType,
                };
                map.set_color_mode(cx, mode);
            }
        }
        let search = self.ui.text_input(cx, ids!(search));
        if let Some(query) = search.changed(actions) {
            map.set_search(cx, &query);
        }
        if search.returned(actions).is_some() {
            map.next_match(cx);
        }
        if search.escaped(actions) {
            search.set_text(cx, "");
            map.set_search(cx, "");
        }
        // A widget can emit several actions in one batch, so look at all of them.
        let uid = map.widget_uid();
        for action in actions {
            let Some(wa) = action.as_widget_action() else { continue };
            if wa.widget_uid != uid {
                continue;
            }
            match wa.cast::<CodeMapAction>() {
                CodeMapAction::Status(text) => self.ui.label(cx, ids!(status)).set_text(cx, &text),
                CodeMapAction::Selected(info) => {
                    self.has_selection = true;
                    self.ui.label(cx, ids!(info_title)).set_text(cx, &info.title);
                    self.ui.label(cx, ids!(info_path)).set_text(cx, &info.path);
                    self.ui.label(cx, ids!(info_details)).set_text(cx, &info.details);
                }
                CodeMapAction::None => {}
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::map_view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // The library supports sweep-selection: a release over a newly opened
        // row selects it immediately and closes the menu. For a quick opening
        // gesture, deliver MouseUp at its original toolbar position instead.
        // The release still reaches the widget, so mouse capture/hover can end
        // normally. A held drag (>= 200 ms) or a second click is not changed.
        let safe_release = if let Event::MouseUp(up) = event {
            if up.button.is_primary() {
                self.dropdown_release_guard
                    .quick_release_origin(cx.seconds_since_app_start())
                    .map(|origin| {
                        let mut up = up.clone();
                        up.abs = origin;
                        Event::MouseUp(up)
                    })
            } else {
                None
            }
        } else {
            None
        };

        if matches!(event, Event::KeyDown(_)) {
            // A later keyboard selection is unrelated to the opening release.
            self.dropdown_release_guard.clear_pending();
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, safe_release.as_ref().unwrap_or(event), &mut Scope::empty());

        if let Event::MouseDown(down) = event {
            let pressed_dropdown = if down.button.is_primary() {
                self.toolbar_dropdown_at(cx, down.abs).map(|kind| {
                    let path = match kind {
                        ToolbarDropdown::ColorMode => ids!(color_mode),
                        ToolbarDropdown::DetailLevel => ids!(detail_level),
                        ToolbarDropdown::Language => ids!(language),
                    };
                    (kind, self.ui.drop_down(cx, path).selected_item())
                })
            } else {
                None
            };
            self.dropdown_release_guard.press(
                pressed_dropdown,
                cx.seconds_since_app_start(),
                down.abs,
            );

            if pressed_dropdown.is_some() {
                // Ask Makepad to redraw the popup pass, not just the root UI.
                // First-use menu rows can be constructed on the first draw.
                self.dropdown_redraws_remaining = 1;
                cx.redraw_all();
            }
        } else if matches!(event, Event::Draw(_)) && self.dropdown_redraws_remaining > 0 {
            // Paint the rows constructed by the first draw; bounded to avoid
            // a redraw loop. No changes to the Makepad dependency required.
            self.dropdown_redraws_remaining -= 1;
            cx.redraw_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DropdownReleaseGuard, ToolbarDropdown};
    use makepad_widgets::dvec2;

    #[test]
    fn quick_opening_release_is_routed_to_the_field() {
        let mut guard = DropdownReleaseGuard::default();
        let original = dvec2(80.0, 25.0);
        guard.press(Some((ToolbarDropdown::DetailLevel, 2)), 10.0, original);
        assert_eq!(guard.quick_release_origin(10.1), Some(original));
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::DetailLevel), Some(2));
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::DetailLevel), None);
        assert_eq!(guard.quick_release_origin(10.15), None);
    }

    #[test]
    fn held_opening_release_keeps_sweep_selection() {
        let mut guard = DropdownReleaseGuard::default();
        guard.press(Some((ToolbarDropdown::Language, 1)), 20.0, dvec2(80.0, 25.0));
        assert_eq!(guard.quick_release_origin(20.3), None);
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::Language), None);
    }

    #[test]
    fn second_click_on_popup_row_is_not_redirected() {
        let mut guard = DropdownReleaseGuard::default();
        guard.press(Some((ToolbarDropdown::Language, 0)), 30.0, dvec2(80.0, 25.0));
        guard.press(None, 30.05, dvec2(80.0, 65.0));
        assert_eq!(guard.quick_release_origin(30.1), None);
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::Language), None);
    }

    #[test]
    fn no_redirect_without_opening_press() {
        let mut guard = DropdownReleaseGuard::default();
        guard.press(None, 40.0, dvec2(80.0, 25.0));
        assert_eq!(guard.quick_release_origin(40.1), None);
    }

    #[test]
    fn release_before_press_time_is_not_redirected() {
        let mut guard = DropdownReleaseGuard::default();
        guard.press(Some((ToolbarDropdown::ColorMode, 1)), 50.0, dvec2(80.0, 25.0));
        assert_eq!(guard.quick_release_origin(49.9), None);
    }

    #[test]
    fn rapid_move_capture_fallback_only_applies_to_opening_menu() {
        let mut guard = DropdownReleaseGuard::default();
        guard.press(Some((ToolbarDropdown::ColorMode, 2)), 60.0, dvec2(80.0, 25.0));
        assert_eq!(guard.quick_release_origin(60.1), Some(dvec2(80.0, 25.0)));
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::Language), None);
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::ColorMode), Some(2));
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::ColorMode), None);
    }

    #[test]
    fn keyboard_action_clears_stale_opening_suppression() {
        let mut guard = DropdownReleaseGuard::default();
        guard.press(Some((ToolbarDropdown::DetailLevel, 1)), 70.0, dvec2(80.0, 25.0));
        assert!(guard.quick_release_origin(70.1).is_some());
        guard.clear_pending();
        assert_eq!(guard.take_accidental_selection(ToolbarDropdown::DetailLevel), None);
    }
}
