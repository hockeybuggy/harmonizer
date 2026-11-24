use crate::params::{self, HarmonizerParams, ModeParam, QualityParam};
use crate::telemetry::Telemetry;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use nice_plug::context::gui::GuiContext;
use nice_plug::editor::dpi::LogicalSize;
use nice_plug::prelude::*;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, NiceEguiApp, RepaintNotifier,
    create_egui_editor, widgets::ParamSlider,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const WINDOW_SIZE: LogicalSize<f32> = LogicalSize::new(620.0, 330.0);
const LOWEST_KEY: u8 = 24;
const HIGHEST_KEY: u8 = 108;

const BACKGROUND: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x24);
const ACCENT: Color32 = Color32::from_rgb(0xf2, 0xb1, 0x34);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x90, 0x9c);

pub fn default_state() -> Arc<EguiEditorState> {
    EguiEditorState::from_size(WINDOW_SIZE, 1.0)
}

pub fn create(
    params: Arc<HarmonizerParams>,
    telemetry: Arc<Telemetry>,
    state: Arc<EguiEditorState>,
    repaint: RepaintNotifier,
) -> Option<EguiEditor<HarmonizerEditor>> {
    create_egui_editor(
        state,
        repaint,
        EguiNiceSettings::new(),
        HarmonizerEditor {
            params,
            telemetry,
            gui: None,
        },
    )
}

pub struct HarmonizerEditor {
    params: Arc<HarmonizerParams>,
    telemetry: Arc<Telemetry>,
    gui: Option<GuiContext>,
}

impl NiceEguiApp for HarmonizerEditor {
    fn build(
        &mut self,
        _egui_ctx: egui::Context,
        gui: GuiContext,
        _frame: &mut nice_plug_egui::Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        self.gui = Some(gui);
        Ok(())
    }

    fn editor_closed(&mut self) {
        self.gui = None;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut nice_plug_egui::Frame) {
        if let Some(gui) = &self.gui {
            self.draw(ui, &gui.param_setter());
        }
    }
}

impl HarmonizerEditor {
    fn draw(&self, ui: &mut egui::Ui, setter: &ParamSetter) {
        let params = &self.params;
        let relative = params.mode.value() == ModeParam::Relative;

        egui::Frame::new()
            .fill(BACKGROUND)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("HARMONIZER")
                            .font(FontId::proportional(20.0))
                            .color(ACCENT)
                            .strong(),
                    );
                    ui.add_space(16.0);
                    enum_buttons(
                        ui,
                        setter,
                        &params.mode,
                        &[
                            (ModeParam::Absolute, "Absolute"),
                            (ModeParam::Relative, "Relative"),
                        ],
                    );
                    ui.add_space(16.0);
                    enum_buttons(
                        ui,
                        setter,
                        &params.quality,
                        &[
                            (QualityParam::LowLatency, "Low latency"),
                            (QualityParam::Balanced, "Balanced"),
                            (QualityParam::HighQuality, "High quality"),
                        ],
                    );
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(self.status_text(relative)).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(self.latency_text()).color(MUTED));
                    });
                });

                ui.add_space(6.0);
                self.keyboard(ui, relative);
                ui.add_space(10.0);

                egui::Grid::new("params")
                    .num_columns(4)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        param_row(ui, setter, "Wet", &params.wet, "Dry", &params.dry);
                        param_row(
                            ui,
                            setter,
                            "Formant",
                            &params.formant,
                            "Spread",
                            &params.stereo_spread,
                        );
                        param_row(
                            ui,
                            setter,
                            "Attack",
                            &params.attack,
                            "Release",
                            &params.release,
                        );
                        param_row(
                            ui,
                            setter,
                            "Velocity",
                            &params.velocity_sensitivity,
                            "Bend range",
                            &params.pitch_bend_range,
                        );
                        ui.add_enabled_ui(relative, |ui| ui.label("Reference"));
                        ui.add_enabled_ui(relative, |ui| {
                            ui.add(ParamSlider::for_param(&params.reference_note, setter))
                        });
                        ui.end_row();
                    });
            });
    }

    fn status_text(&self, relative: bool) -> String {
        if relative {
            return format!(
                "Notes transpose relative to {}",
                note_name(self.params.reference_note.value() as f32)
            );
        }
        let pitch = self.telemetry.detected_pitch.load(Ordering::Relaxed);
        if pitch.is_finite() && pitch > 0.0 {
            let note = 69.0 + 12.0 * (pitch / 440.0).log2();
            let cents = ((note - note.round()) * 100.0).round();
            format!(
                "Input  {}  {:+.0}¢  ({:.1} Hz)",
                note_name(note.round()),
                cents,
                pitch
            )
        } else {
            "Input  —".to_owned()
        }
    }

    fn latency_text(&self) -> String {
        let samples = self.telemetry.latency_samples.load(Ordering::Relaxed);
        let rate = self.telemetry.sample_rate.load(Ordering::Relaxed);
        format!("Latency {:.1} ms", samples as f32 * 1000.0 / rate)
    }

    fn keyboard(&self, ui: &mut egui::Ui, relative: bool) {
        let white_keys: Vec<u8> = (LOWEST_KEY..=HIGHEST_KEY)
            .filter(|&n| !is_black(n))
            .collect();
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 70.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let key_width = rect.width() / white_keys.len() as f32;
        let level = |n: u8| self.telemetry.note_levels[n as usize].load(Ordering::Relaxed);
        let lit = |base: Color32, n: u8| base.lerp_to_gamma(ACCENT, level(n).clamp(0.0, 1.0));

        let mut white_x = std::collections::HashMap::new();
        for (i, &n) in white_keys.iter().enumerate() {
            let x = rect.left() + i as f32 * key_width;
            white_x.insert(n, x);
            let key = Rect::from_min_size(
                Pos2::new(x, rect.top()),
                Vec2::new(key_width - 1.0, rect.height()),
            );
            painter.rect_filled(key, CornerRadius::same(2), lit(Color32::from_gray(225), n));
            if n % 12 == 0 {
                painter.text(
                    Pos2::new(key.center().x, key.bottom() - 4.0),
                    Align2::CENTER_BOTTOM,
                    note_name(n as f32),
                    FontId::proportional(9.0),
                    Color32::from_gray(90),
                );
            }
        }
        for n in (LOWEST_KEY..=HIGHEST_KEY).filter(|&n| is_black(n)) {
            let Some(&left) = white_x.get(&(n - 1)) else {
                continue;
            };
            let key = Rect::from_min_size(
                Pos2::new(left + key_width * 0.65, rect.top()),
                Vec2::new(key_width * 0.7, rect.height() * 0.6),
            );
            painter.rect_filled(key, CornerRadius::same(2), lit(Color32::from_gray(40), n));
        }

        let marker = if relative {
            Some(self.params.reference_note.value() as f32)
        } else {
            let pitch = self.telemetry.detected_pitch.load(Ordering::Relaxed);
            (pitch.is_finite() && pitch > 0.0).then(|| 69.0 + 12.0 * (pitch / 440.0).log2())
        };
        if let Some(note) = marker.filter(|n| (LOWEST_KEY as f32..=HIGHEST_KEY as f32).contains(n))
        {
            let x = key_centre_x(note, &white_x, key_width);
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(2.0, Color32::from_rgb(0x4f, 0xc3, 0xf7)),
            );
        }
    }
}

fn key_centre_x(note: f32, white_x: &std::collections::HashMap<u8, f32>, key_width: f32) -> f32 {
    let lower = note.floor() as u8;
    let centre = |n: u8| {
        if is_black(n) {
            white_x.get(&(n - 1)).map(|x| x + key_width)
        } else {
            white_x.get(&n).map(|x| x + key_width * 0.5)
        }
    };
    let a = centre(lower).unwrap_or(0.0);
    let b = centre(lower.saturating_add(1)).unwrap_or(a);
    a + (b - a) * note.fract()
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

fn note_name(note: f32) -> String {
    params::note_name(note.round() as i32)
}

fn enum_buttons<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &EnumParam<T>,
    options: &[(T, &str)],
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for &(value, label) in options {
            if ui.selectable_label(param.value() == value, label).clicked() {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, value);
                setter.end_set_parameter(param);
            }
        }
    });
}

fn param_row<A: Param, B: Param>(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    a_label: &str,
    a: &A,
    b_label: &str,
    b: &B,
) {
    ui.label(a_label);
    ui.add(ParamSlider::for_param(a, setter));
    ui.label(b_label);
    ui.add(ParamSlider::for_param(b, setter));
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::prelude::{GuiContextInner, ParamPtr, PluginApi, PluginState};

    struct NoHost;

    impl GuiContextInner for NoHost {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Vst3
        }
        unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
        unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
            unsafe { param._internal_set_normalized_value(normalized) };
        }
        unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
        fn get_state(&self) -> PluginState {
            unimplemented!()
        }
        fn set_state(&self, _state: PluginState) {}
        fn request_restart(&self) {}
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored to write target/editor-*.png"]
    fn render_editor_snapshots() {
        for mode in [ModeParam::Absolute, ModeParam::Relative] {
            let params = Arc::new(HarmonizerParams::default());
            if mode == ModeParam::Relative {
                let setter = ParamSetter::new(&NoHost);
                setter.set_parameter(&params.mode, mode);
            }
            let telemetry = Arc::new(Telemetry::default());
            telemetry.detected_pitch.store(221.3, Ordering::Relaxed);
            telemetry.latency_samples.store(1200, Ordering::Relaxed);
            for (note, level) in [(57, 1.0), (60, 0.9), (64, 0.6), (67, 0.3)] {
                telemetry.note_levels[note].store(level, Ordering::Relaxed);
            }
            let editor = HarmonizerEditor {
                params,
                telemetry,
                gui: None,
            };
            let host = NoHost;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(Vec2::new(WINDOW_SIZE.width, WINDOW_SIZE.height))
                .build_ui(|ui| editor.draw(ui, &ParamSetter::new(&host)));
            harness.run();
            let image = harness.render().expect("render editor");
            let path = format!(
                "{}/../../target/editor-{mode:?}.png",
                env!("CARGO_MANIFEST_DIR")
            )
            .to_lowercase();
            image.save(&path).unwrap();
        }
    }
}
