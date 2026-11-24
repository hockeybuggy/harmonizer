use crate::params::{self, HarmonizerParams, ModeParam, QualityParam};
use crate::telemetry::Telemetry;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Layout, Painter, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};
use nice_plug::context::gui::GuiContext;
use nice_plug::editor::dpi::LogicalSize;
use nice_plug::prelude::*;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, NiceEguiApp, RepaintNotifier,
    create_egui_editor, widgets::ParamSlider,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const WINDOW_SIZE: LogicalSize<f32> = LogicalSize::new(640.0, 390.0);
const LOWEST_KEY: u8 = 36;
const HIGHEST_KEY: u8 = 96;
const LIVE_HEIGHT: f32 = 104.0;
const GAP: f32 = 8.0;
const SECTION_GAP: f32 = 12.0;

const BACKGROUND: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x24);
const SURFACE: Color32 = Color32::from_rgb(0x26, 0x29, 0x31);
const WELL: Color32 = Color32::from_rgb(0x17, 0x19, 0x1e);
const BORDER: Color32 = Color32::from_rgb(0x33, 0x37, 0x41);
const TRACK: Color32 = Color32::from_rgb(0x33, 0x37, 0x3f);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x90, 0x9c);
const ACCENT: Color32 = Color32::from_rgb(0xf2, 0xb1, 0x34);
// Settings stay a step quieter than the live readouts and sounding notes.
const SLIDER_FILL: Color32 = Color32::from_rgb(0xa8, 0x7d, 0x2c);
const INPUT_PITCH: Color32 = Color32::from_rgb(0x4f, 0xc3, 0xf7);
const WARNING: Color32 = Color32::from_rgb(0xff, 0x8a, 0x65);
const CLIP: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

pub fn default_state() -> Arc<EguiEditorState> {
    EguiEditorState::from_size(WINDOW_SIZE, 1.0)
}

pub fn create(
    params: Arc<HarmonizerParams>,
    telemetry: Arc<Telemetry>,
    state: Arc<EguiEditorState>,
    repaint: RepaintNotifier,
    sidechain: bool,
) -> Option<EguiEditor<HarmonizerEditor>> {
    create_egui_editor(
        state,
        repaint,
        EguiNiceSettings::new(),
        HarmonizerEditor {
            params,
            telemetry,
            sidechain,
            gui: None,
        },
    )
}

pub struct HarmonizerEditor {
    params: Arc<HarmonizerParams>,
    telemetry: Arc<Telemetry>,
    sidechain: bool,
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

    fn ui(&mut self, ui: &mut Ui, _frame: &mut nice_plug_egui::Frame) {
        if let Some(gui) = &self.gui {
            self.draw(ui, &gui.param_setter());
        }
    }
}

impl HarmonizerEditor {
    fn draw(&self, ui: &mut Ui, setter: &ParamSetter) {
        let viewport = ui.ctx().content_rect();
        ui.painter()
            .with_clip_rect(viewport)
            .rect_filled(viewport, 0, BACKGROUND);
        let relative = self.params.mode.value() == ModeParam::Relative;
        let hz = self.telemetry.detected_pitch.load(Ordering::Relaxed);
        let pitch = (!relative && hz.is_finite() && hz > 0.0)
            .then(|| (hz, 69.0 + 12.0 * (hz / 440.0).log2()));

        ui.scope_builder(UiBuilder::new().max_rect(viewport.shrink(16.0)), |ui| {
            editor_style(ui);
            self.header(ui, setter);
            ui.add_space(SECTION_GAP);
            ui.horizontal(|ui| {
                self.input_card(ui, relative, pitch);
                self.keyboard(ui, pitch.map(|(_, note)| note), self.hint(relative));
            });
            ui.add_space(SECTION_GAP);
            self.parameter_cards(ui, setter, relative);
        });
    }

    fn header(&self, ui: &mut Ui, setter: &ParamSetter) {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 32.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(egui::text::LayoutJob::single_section(
                    "HARMONIZER".to_owned(),
                    egui::TextFormat {
                        font_id: FontId::proportional(14.0),
                        color: ACCENT,
                        extra_letter_spacing: 0.9,
                        ..Default::default()
                    },
                ));
                ui.add_space(16.0);
                enum_buttons(
                    ui,
                    setter,
                    &self.params.mode,
                    &[
                        (ModeParam::Absolute, "Absolute", 82.0),
                        (ModeParam::Relative, "Relative", 82.0),
                    ],
                    28.0,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let samples = self.telemetry.latency_samples.load(Ordering::Relaxed);
                    let rate = self.telemetry.sample_rate.load(Ordering::Relaxed);
                    let latency = if rate > 0.0 {
                        format!("{:.0} ms", samples as f32 * 1000.0 / rate)
                    } else {
                        "— ms".into()
                    };
                    ui.add_sized(
                        [40.0, 22.0],
                        egui::Label::new(RichText::new(latency).size(11.0).color(MUTED)),
                    )
                    .on_hover_text("Processing latency, compensated by the host");
                    let options = [
                        (QualityParam::LowLatency, "Low", 36.0),
                        (QualityParam::Balanced, "Balanced", 64.0),
                        (QualityParam::HighQuality, "High", 40.0),
                    ];
                    let width = options.iter().map(|o| o.2 + 2.0).sum::<f32>();
                    ui.allocate_ui_with_layout(
                        Vec2::new(width, 22.0),
                        Layout::left_to_right(Align::Center),
                        |ui| enum_buttons(ui, setter, &self.params.quality, &options, 22.0),
                    )
                    .response
                    .on_hover_text("Lower latency for playing live; higher quality for mixing");
                });
            },
        );
    }

    fn input_card(&self, ui: &mut Ui, relative: bool, pitch: Option<(f32, f32)>) {
        let rect = card_rect(ui, Vec2::new(128.0, LIVE_HEIGHT), SURFACE);
        let painter = ui.painter_at(rect);
        let top = rect.min + Vec2::splat(12.0);
        painter.text(
            top,
            Align2::LEFT_TOP,
            "INPUT",
            FontId::proportional(11.0),
            MUTED,
        );
        let note = if relative {
            Some(self.params.reference_note.value() as f32)
        } else {
            pitch.map(|(_, note)| note)
        };
        let note_rect = painter.text(
            top + Vec2::new(0.0, 16.0),
            Align2::LEFT_TOP,
            note.map_or_else(|| "—".into(), note_name),
            FontId::proportional(28.0),
            if note.is_some() { TEXT } else { MUTED },
        );
        if let Some((_, n)) = pitch {
            let cents = ((n - n.round()) * 100.0).round();
            painter.text(
                Pos2::new(note_rect.right() + 8.0, note_rect.center().y + 1.0),
                Align2::LEFT_CENTER,
                format!("{cents:+.0}¢"),
                FontId::proportional(12.0),
                TEXT,
            );
        }
        let caption = if relative {
            "reference note".into()
        } else {
            pitch.map_or_else(
                || "No pitch detected".into(),
                |(hz, _)| format!("{hz:.1} Hz"),
            )
        };
        painter.text(
            top + Vec2::new(0.0, 48.0),
            Align2::LEFT_TOP,
            caption,
            FontId::proportional(11.0),
            MUTED,
        );
        self.input_meter(&painter, rect);
    }

    fn input_meter(&self, painter: &Painter, rect: Rect) {
        let y = rect.bottom() - 16.0;
        painter.text(
            Pos2::new(rect.left() + 12.0, y),
            Align2::LEFT_CENTER,
            "IN",
            FontId::proportional(10.0),
            MUTED,
        );
        let track = Rect::from_min_max(
            Pos2::new(rect.left() + 32.0, y - 4.0),
            Pos2::new(rect.right() - 12.0, y + 4.0),
        );
        painter.rect_filled(track, 2, TRACK);
        let level = self.telemetry.input_meter.load(Ordering::Relaxed);
        let db = 20.0 * level.max(1e-6).log10();
        let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        if fill > 0.0 {
            let mut bar = track;
            bar.set_width(track.width() * fill);
            painter.rect_filled(bar, 2, if db > -1.0 { CLIP } else { ACCENT });
        }
    }

    fn keyboard(&self, ui: &mut Ui, pitch: Option<f32>, hint: Option<&str>) {
        let rect = card_rect(ui, Vec2::new(ui.available_width(), LIVE_HEIGHT), WELL);
        let painter = ui.painter_at(rect);
        let title = painter.text(
            rect.min + Vec2::new(12.0, 12.0),
            Align2::LEFT_TOP,
            "VOICES",
            FontId::proportional(11.0),
            MUTED,
        );
        let keys = Rect::from_min_max(
            rect.min + Vec2::new(12.0, 32.0),
            rect.max - Vec2::splat(12.0),
        );
        let white_count = (LOWEST_KEY..=HIGHEST_KEY)
            .filter(|&note| !is_black(note))
            .count();
        let key_width = keys.width() / white_count as f32;
        let level = |n: u8| {
            self.telemetry.note_levels[n as usize]
                .load(Ordering::Relaxed)
                .clamp(0.0, 1.0)
        };
        let centre = |note| keys.left() + key_centre(note) * key_width;
        for n in (LOWEST_KEY..=HIGHEST_KEY).filter(|&note| !is_black(note)) {
            let x = centre(n);
            let key = Rect::from_center_size(
                Pos2::new(x, keys.center().y),
                Vec2::new(key_width - 1.0, keys.height()),
            );
            let strength = level(n);
            painter.rect_filled(
                key,
                2,
                Color32::from_rgb(0xd5, 0xd9, 0xe0).lerp_to_gamma(ACCENT, strength),
            );
            if strength > 0.0 || n.is_multiple_of(12) {
                painter.text(
                    Pos2::new(x, key.bottom() - 4.0),
                    Align2::CENTER_BOTTOM,
                    note_name(n as f32),
                    FontId::proportional(8.5),
                    if strength > 0.0 {
                        WELL
                    } else {
                        Color32::from_gray(88)
                    },
                );
            }
        }
        for n in (LOWEST_KEY..=HIGHEST_KEY).filter(|&note| is_black(note)) {
            let height = keys.height() * 0.57;
            let key = Rect::from_center_size(
                Pos2::new(centre(n), keys.top() + height * 0.5),
                Vec2::new(key_width * 0.66, height),
            );
            painter.rect_filled(key, 2, TRACK.lerp_to_gamma(ACCENT, level(n)));
        }
        if let Some(note) =
            pitch.filter(|note| (LOWEST_KEY as f32..=HIGHEST_KEY as f32).contains(note))
        {
            let a = centre(note.floor() as u8);
            let b = centre((note.ceil() as u8).min(HIGHEST_KEY));
            let x = a + (b - a) * note.fract();
            let top = Pos2::new(x, keys.top());
            painter.line_segment(
                [top, Pos2::new(x, keys.bottom())],
                Stroke::new(2.0, INPUT_PITCH),
            );
            triangle(
                &painter,
                top + Vec2::new(0.0, -5.0),
                Vec2::new(0.0, 4.0),
                INPUT_PITCH,
            );
        }
        if (0..LOWEST_KEY).any(|note| level(note) > 0.0) {
            triangle(
                &painter,
                Pos2::new(title.right() + 12.0, title.center().y),
                Vec2::new(-4.0, 0.0),
                ACCENT,
            );
        }
        if (HIGHEST_KEY + 1..128).any(|note| level(note) > 0.0) {
            triangle(
                &painter,
                Pos2::new(keys.right() - 4.0, title.center().y),
                Vec2::new(4.0, 0.0),
                ACCENT,
            );
        }
        if let Some(hint) = hint {
            let mut job = egui::text::LayoutJob::simple(
                hint.to_owned(),
                FontId::proportional(13.0),
                WARNING,
                keys.width() - 48.0,
            );
            job.halign = Align::Center;
            let text = painter.layout_job(job);
            painter.rect_filled(
                keys.expand(2.0),
                3,
                Color32::from_rgba_unmultiplied(23, 25, 30, 225),
            );
            // A centred galley is laid out around x = 0, so anchor it at the centre line.
            let origin = Pos2::new(keys.center().x, keys.center().y - text.rect.height() / 2.0);
            painter.galley(origin, text, WARNING);
        }
    }

    fn hint(&self, relative: bool) -> Option<&'static str> {
        let telemetry = &self.telemetry;
        if telemetry.input_missing.load(Ordering::Relaxed) {
            return Some(if self.sidechain {
                "No sidechain audio: pick a source track in the Sidechain menu"
            } else {
                "No audio coming in: play a clip on this track, or set its Monitor to In"
            });
        }
        if !telemetry.notes_seen.load(Ordering::Relaxed) {
            return Some(if self.sidechain {
                "Waiting for notes: arm this track, or play a MIDI clip on it"
            } else {
                "Waiting for notes: set a MIDI track's MIDI To to this track and Harmonizer"
            });
        }
        if !relative && telemetry.input_unpitched.load(Ordering::Relaxed) {
            return Some(
                "No clear pitch: Absolute needs one voice at a time; try Relative for chords",
            );
        }
        None
    }

    fn parameter_cards(&self, ui: &mut Ui, setter: &ParamSetter, relative: bool) {
        let p = &self.params;
        let (row, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let width = (row.width() - 2.0 * GAP) / 3.0;
        let card = |index: usize| {
            let left = row.left() + index as f32 * (width + GAP);
            Rect::from_min_size(Pos2::new(left, row.top()), Vec2::new(width, row.height()))
        };
        parameter_card(ui, card(0), "VOICE", |ui| {
            control(ui, setter, "Formant", &p.formant);
            if relative {
                control(ui, setter, "Reference", &p.reference_note);
            } else {
                control(ui, setter, "Expression", &p.expression);
            }
        });
        parameter_card(ui, card(1), "MIX", |ui| {
            control(ui, setter, "Wet", &p.wet);
            control(ui, setter, "Dry", &p.dry);
            control(ui, setter, "Spread", &p.stereo_spread);
        });
        parameter_card(ui, card(2), "NOTES", |ui| {
            control(ui, setter, "Attack", &p.attack);
            control(ui, setter, "Release", &p.release);
            control(ui, setter, "Velocity", &p.velocity_sensitivity);
            control(ui, setter, "Bend range", &p.pitch_bend_range);
        });
    }
}

fn editor_style(ui: &mut Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = Vec2::new(GAP, 0.0);
    style.spacing.button_padding = Vec2::new(6.0, 3.0);
    style.spacing.interact_size.y = 22.0;
    style.spacing.text_edit_width = 64.0;
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.0));
    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = SLIDER_FILL;
    visuals.selection.stroke = Stroke::new(1.0, WELL);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.fg_stroke.color = TEXT;
        widget.bg_stroke = Stroke::new(1.0, BORDER);
        widget.corner_radius = CornerRadius::same(4);
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.inactive.bg_fill = TRACK;
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.hovered.bg_fill = TRACK;
    visuals.widgets.hovered.weak_bg_fill = TRACK;
    visuals.widgets.hovered.bg_stroke.color = ACCENT;
    visuals.widgets.active.bg_fill = BORDER;
    visuals.widgets.active.weak_bg_fill = TRACK;
    visuals.widgets.active.bg_stroke.color = ACCENT;
    visuals.widgets.open.bg_fill = WELL;
    visuals.widgets.open.weak_bg_fill = WELL;
    visuals.extreme_bg_color = WELL;
}

fn card_rect(ui: &mut Ui, size: Vec2, colour: Color32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        6,
        colour,
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    rect
}

fn parameter_card(ui: &mut Ui, rect: Rect, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    ui.painter().rect(
        rect,
        6,
        SURFACE,
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    let mut inner = ui.new_child(
        UiBuilder::new()
            .id_salt(title)
            .max_rect(rect.shrink(12.0))
            .layout(Layout::top_down(Align::Min)),
    );
    inner.label(RichText::new(title).size(11.0).color(MUTED));
    inner.add_space(8.0);
    add_contents(&mut inner);
}

fn control<P: Param>(ui: &mut Ui, setter: &ParamSetter, label: &str, param: &P) {
    ui.push_id(label, |ui| {
        let caption = ui.label(RichText::new(label).size(12.0).color(MUTED));
        let track_width = ui.available_width() - 64.0 - GAP;
        ui.add(ParamSlider::for_param(param, setter).with_width(track_width))
            .labelled_by(caption.id);
    });
    ui.add_space(4.0);
}

fn key_centre(note: u8) -> f32 {
    let offset = note - LOWEST_KEY;
    let within_octave = [0.5, 1.0, 1.5, 2.0, 2.5, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5];
    f32::from(offset / 12 * 7) + within_octave[(offset % 12) as usize]
}

fn triangle(painter: &Painter, centre: Pos2, direction: Vec2, colour: Color32) {
    let across = Vec2::new(-direction.y, direction.x);
    painter.add(egui::Shape::convex_polygon(
        vec![
            centre + direction,
            centre - direction + across,
            centre - direction - across,
        ],
        colour,
        Stroke::NONE,
    ));
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

fn note_name(note: f32) -> String {
    params::note_name(note.round() as i32)
}

fn enum_buttons<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    setter: &ParamSetter,
    param: &EnumParam<T>,
    options: &[(T, &str, f32)],
    height: f32,
) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for &(value, label, width) in options {
            let selected = param.value() == value;
            let text = RichText::new(label)
                .size(if height > 22.0 { 13.0 } else { 11.0 })
                .color(if selected { WELL } else { TEXT });
            let mut button = egui::Button::new(text).corner_radius(4);
            if selected {
                button = button.fill(ACCENT).stroke(Stroke::new(1.0, ACCENT));
            }
            if ui.add_sized([width, height], button).clicked() {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, value);
                setter.end_set_parameter(param);
            }
        }
    });
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
        let cases = [
            ("absolute", ModeParam::Absolute, false, true, false, 1.0),
            ("relative", ModeParam::Relative, false, true, false, 1.0),
            ("waiting", ModeParam::Absolute, false, false, false, 1.0),
            ("no-sidechain", ModeParam::Absolute, true, true, true, 1.0),
            ("out-of-range", ModeParam::Absolute, false, true, false, 1.0),
            ("absolute-2x", ModeParam::Absolute, false, true, false, 2.0),
        ];
        for (name, mode, sidechain, notes_seen, input_missing, scale) in cases {
            let params = Arc::new(HarmonizerParams::default());
            if mode == ModeParam::Relative {
                let setter = ParamSetter::new(&NoHost);
                setter.set_parameter(&params.mode, mode);
            }
            let telemetry = Arc::new(Telemetry::default());
            telemetry.latency_samples.store(
                if mode == ModeParam::Relative {
                    2048
                } else {
                    1200
                },
                Ordering::Relaxed,
            );
            telemetry.notes_seen.store(notes_seen, Ordering::Relaxed);
            telemetry
                .input_missing
                .store(input_missing, Ordering::Relaxed);
            if !input_missing {
                telemetry.detected_pitch.store(221.3, Ordering::Relaxed);
                let level = if name == "out-of-range" { 0.98 } else { 0.30 };
                telemetry.input_meter.store(level, Ordering::Relaxed);
            }
            if notes_seen && !input_missing {
                for (note, level) in [(57, 1.0), (60, 0.9), (64, 0.6), (67, 0.3)] {
                    telemetry.note_levels[note].store(level, Ordering::Relaxed);
                }
            }
            if name == "out-of-range" {
                for note in [33, 100] {
                    telemetry.note_levels[note].store(1.0, Ordering::Relaxed);
                }
            }
            let editor = HarmonizerEditor {
                params,
                telemetry,
                sidechain,
                gui: None,
            };
            let host = NoHost;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(Vec2::new(WINDOW_SIZE.width, WINDOW_SIZE.height))
                .with_pixels_per_point(scale)
                .build_ui(|ui| editor.draw(ui, &ParamSetter::new(&host)));
            harness.run();
            let image = harness.render().expect("render editor");
            let path = format!(
                "{}/../../target/editor-{name}.png",
                env!("CARGO_MANIFEST_DIR")
            );
            image.save(&path).unwrap();
            let (width, height) = image.dimensions();
            for (x, y) in [(0, 0), (width - 1, height / 2), (width / 2, height - 1)] {
                assert_eq!(
                    image.get_pixel(x, y).0,
                    BACKGROUND.to_array(),
                    "{name}: exposed window background at {x},{y}"
                );
            }
        }
    }
}
