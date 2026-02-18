use std::f32::consts::PI;
use std::sync::{Arc, LazyLock};

use nih_plug::prelude::{Param, ParamSetter};
use nih_plug_egui::egui::{
    self, vec2, Color32, CornerRadius, Key, Response, Sense, Stroke, TextEdit, Ui, Vec2, Widget, WidgetText,
};
use nih_plug_egui::widgets::util::add_hsv;
use parking_lot::Mutex;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::POINT;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

static DRAG_START_SCREEN_POS_ID: LazyLock<egui::Id> = LazyLock::new(|| egui::Id::new((file!(), "drag_start_screen_pos")));

/// When shift+dragging a parameter, one pixel dragged corresponds to this much change in the
/// noramlized parameter.
const GRANULAR_DRAG_MULTIPLIER: f32 = 0.0015;

/// Standard drag sensitivity for the knob (non-granular)
const STANDARD_DRAG_MULTIPLIER: f32 = 0.005;

const HOVER_SCALE_FACTOR: f32 = 0.95;

static DRAG_NORMALIZED_START_VALUE_MEMORY_ID: LazyLock<egui::Id> = LazyLock::new(|| egui::Id::new((file!(), 0)));
static DRAG_AMOUNT_MEMORY_ID: LazyLock<egui::Id> = LazyLock::new(|| egui::Id::new((file!(), 1)));
static VALUE_ENTRY_MEMORY_ID: LazyLock<egui::Id> = LazyLock::new(|| egui::Id::new((file!(), 2)));

/// A knob widget that knows about NIH-plug parameters ranges and can get values for it.
/// Supports double click/ctrl-click to reset, shift+drag for granular dragging,
/// and text value entry by clicking on the value text.
#[must_use = "You should put this widget in an ui with `ui.add(widget);`"]
pub struct ParamKnob<'a, P: Param> {
    param: &'a P,
    setter: &'a ParamSetter<'a>,

    draw_value: bool,
    diameter: f32,

    color: Color32,

    /// Will be set in the `ui()` function so we can request keyboard input focus on Alt+click.
    keyboard_focus_id: Option<egui::Id>,
}

impl<'a, P: Param> ParamKnob<'a, P> {
    /// Create a new knob for a parameter.
    pub fn for_param(param: &'a P, setter: &'a ParamSetter<'a>) -> Self {
        Self {
            param,
            setter,

            draw_value: true,
            diameter: 30.0,

            keyboard_focus_id: None,

            color: Color32::from_hex("#FFEAD0").unwrap(),
        }
    }

    /// Don't draw the text value below the knob.
    pub fn without_value(mut self) -> Self {
        self.draw_value = false;
        self
    }

    /// Set a custom diameter for the knob.
    pub fn with_diameter(mut self, diameter: f32) -> Self {
        self.diameter = diameter;
        self
    }

    pub fn with_color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    fn plain_value(&self) -> P::Plain {
        self.param.modulated_plain_value()
    }

    fn normalized_value(&self) -> f32 {
        self.param.modulated_normalized_value()
    }

    fn string_value(&self) -> String {
        self.param.to_string()
    }

    /// Enable the keyboard entry part of the widget.
    fn begin_keyboard_entry(&self, ui: &Ui) {
        ui.memory_mut(|mem| mem.request_focus(self.keyboard_focus_id.unwrap()));

        let value_entry_mutex = ui.memory_mut(|mem| {
            mem.data
                .get_temp_mut_or_default::<Arc<Mutex<String>>>(*VALUE_ENTRY_MEMORY_ID)
                .clone()
        });
        *value_entry_mutex.lock() = self.string_value();
    }

    fn keyboard_entry_active(&self, ui: &Ui) -> bool {
        ui.memory(|mem| mem.has_focus(self.keyboard_focus_id.unwrap()))
    }

    fn begin_drag(&self) {
        self.setter.begin_set_parameter(self.param);
    }

    fn set_normalized_value(&self, normalized: f32) {
        let value = self.param.preview_plain(normalized);
        if value != self.plain_value() {
            self.setter.set_parameter(self.param, value);
        }
    }

    fn set_from_string(&self, string: &str) -> bool {
        match self.param.string_to_normalized_value(string) {
            Some(normalized_value) => {
                self.set_normalized_value(normalized_value);
                true
            }
            None => false,
        }
    }

    fn reset_param(&self) {
        self.setter.set_parameter(self.param, self.param.default_plain_value());
    }

    fn granular_drag(&self, ui: &Ui, drag_delta: Vec2) {
        let start_value = if Self::get_drag_amount_memory(ui) == 0.0 {
            Self::set_drag_normalized_start_value_memory(ui, self.normalized_value());
            self.normalized_value()
        } else {
            Self::get_drag_normalized_start_value_memory(ui)
        };

        let delta_val = -drag_delta.y;

        let total_drag_distance = delta_val + Self::get_drag_amount_memory(ui);
        Self::set_drag_amount_memory(ui, total_drag_distance);

        self.set_normalized_value((start_value + (total_drag_distance * GRANULAR_DRAG_MULTIPLIER)).clamp(0.0, 1.0));
    }

    fn end_drag(&self) {
        self.setter.end_set_parameter(self.param);
    }

    fn get_drag_normalized_start_value_memory(ui: &Ui) -> f32 {
        ui.memory(|mem| mem.data.get_temp(*DRAG_NORMALIZED_START_VALUE_MEMORY_ID).unwrap_or(0.5))
    }

    fn set_drag_normalized_start_value_memory(ui: &Ui, amount: f32) {
        ui.memory_mut(|mem| mem.data.insert_temp(*DRAG_NORMALIZED_START_VALUE_MEMORY_ID, amount));
    }

    fn get_drag_amount_memory(ui: &Ui) -> f32 {
        ui.memory(|mem| mem.data.get_temp(*DRAG_AMOUNT_MEMORY_ID).unwrap_or(0.0))
    }

    fn set_drag_amount_memory(ui: &Ui, amount: f32) {
        ui.memory_mut(|mem| mem.data.insert_temp(*DRAG_AMOUNT_MEMORY_ID, amount));
    }

    #[cfg(target_os = "windows")]
    fn get_cursor_screen_pos() -> POINT {
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt).unwrap_or(()) };
        pt
    }

    #[cfg(target_os = "windows")]
    fn set_cursor_screen_pos(pt: POINT) {
        unsafe { SetCursorPos(pt.x, pt.y).unwrap_or(()) };
    }

    fn knob_ui(&self, ui: &Ui, response: &mut Response) {
        if response.drag_started() {
            self.begin_drag();
            Self::set_drag_amount_memory(ui, 0.0);

            let start_pos = Self::get_cursor_screen_pos();
            ui.memory_mut(|mem| {
                mem.data.insert_temp(*DRAG_START_SCREEN_POS_ID, start_pos);
            });

            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::None);
        }

        if response.dragged() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::None);

            let lock_pos = ui.memory(|mem| mem.data.get_temp::<POINT>(*DRAG_START_SCREEN_POS_ID));

            if let Some(lock_pos) = lock_pos {
                let current_pos = Self::get_cursor_screen_pos();

                let dy = lock_pos.y - current_pos.y;

                if dy != 0 {
                    let mut delta = dy as f32 * STANDARD_DRAG_MULTIPLIER;

                    if ui.input(|i| i.modifiers.shift) {
                        delta *= 0.1;
                    }

                    let new_value = (self.normalized_value() + delta).clamp(0.0, 1.0);
                    self.set_normalized_value(new_value);
                    response.mark_changed();

                    Self::set_cursor_screen_pos(lock_pos);
                }
            }
        }

        if let Some(_) = response.interact_pointer_pos() {
            if ui.input(|i| i.modifiers.command) && response.clicked() {
                self.reset_param();
                response.mark_changed();
            }
        }

        if response.double_clicked() {
            self.reset_param();
            response.mark_changed();
        }

        if response.drag_stopped() {
            self.end_drag();
            #[cfg(target_os = "windows")]
            {
                ui.memory_mut(|mem| mem.data.remove::<POINT>(*DRAG_START_SCREEN_POS_ID));
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Default);
            }
        }

        if ui.is_rect_visible(response.rect) {
            let painter = ui.painter();
            let rect = response.rect;
            let center = rect.center();
            let radius = rect.width().min(rect.height()) / 2.0;

            let anim_id = response.id.with("hover_anim");
            let is_active = response.hovered() || response.dragged();

            let linear_t = ui.ctx().animate_bool_with_time(anim_id, is_active, 0.5);

            let ease_t = if is_active {
                1.0 - (1.0 - linear_t).powi(6)
            } else {
                linear_t.powi(6)
            };

            let current_scale = egui::lerp(1.0..=0.92, ease_t);

            let inner_radius = radius * current_scale;

            {
                let shadow = egui::epaint::Shadow {
                    color: egui::Color32::from_black_alpha(100),
                    offset: [2, 2],
                    blur: 12,
                    spread: 0,
                };

                let shadow_rect = egui::Rect::from_center_size(center + vec2(0.0, 2.0), Vec2::splat(radius * 2.0));

                let shadow_mesh = shadow.as_shape(shadow_rect, CornerRadius::same(64));

                painter.add(shadow_mesh);
            }

            let light_anim_id = response.id.with("shadow_anim");
            let light_linear_t = ui.ctx().animate_bool_with_time(light_anim_id, response.dragged(), 0.5);

            let light_ease_t = if response.dragged() {
                1.0 - (1.0 - light_linear_t).powi(6)
            } else {
                light_linear_t.powi(6)
            };

            let _current_alpha = egui::lerp(0.0..=0.2, light_ease_t);

            if response.dragged() {
                let is_active = response.dragged();

                let light_linear_t = ui.ctx().animate_bool_with_time(light_anim_id, is_active, 8.0);

                let light_ease_t = if is_active {
                    1.0 - (1.0 - light_linear_t).powi(6)
                } else {
                    light_linear_t.powi(6)
                };

                let current_alpha = egui::lerp(0.0..=0.2, light_ease_t);

                let shadow = egui::epaint::Shadow {
                    color: self.color.gamma_multiply(current_alpha),
                    offset: [0, 0],
                    blur: 48,
                    spread: 0,
                };

                let shadow_rect = egui::Rect::from_center_size(center + vec2(0.0, 0.0), Vec2::splat(radius * 2.0));

                let shadow_mesh = shadow.as_shape(shadow_rect, CornerRadius::same(64));

                painter.add(shadow_mesh);
            }

            // 0.75 PI (135 deg) to 2.25 PI (405 deg)
            let start_angle = 0.75 * PI;
            let total_angle = 1.5 * PI;
            let current_val = self.normalized_value();
            let end_angle = start_angle + (total_angle * current_val);

            let draw_arc = |start: f32, end: f32, color: egui::Color32, thickness: f32| {
                let n_points = 32; // 弧线平滑度
                let points: Vec<egui::Pos2> = (0..=n_points)
                    .map(|i| {
                        let t = i as f32 / n_points as f32;
                        let angle = egui::lerp(start..=end, t);
                        center + vec2(angle.cos(), angle.sin()) * radius
                    })
                    .collect();

                painter.add(egui::Shape::line(points, Stroke::new(thickness, color)));
            };

            // bg arc
            draw_arc(start_angle, start_angle + total_angle, self.color.gamma_multiply(0.2), 8.0);

            // fg arc
            let active_color = if response.dragged() {
                add_hsv(self.color, 0.0, -0.1, 0.1)
            } else {
                // ui.visuals().selection.bg_fill
                self.color.gamma_multiply(1.0)
            };

            if current_val > 0.001 {
                draw_arc(start_angle, end_angle, active_color, 8.0);
            }

            painter.circle(center, inner_radius, Color32::from_hex("#1C1917").unwrap(), Stroke::NONE);

            // 指示器
            let (sin, cos) = end_angle.sin_cos();
            let _pointer_len = radius * 0.4;
            let inner_radius = inner_radius * 0.75;

            let _p_end = center + vec2(cos * radius, sin * radius);
            let p_start = center + vec2(cos * inner_radius, sin * inner_radius);

            // painter.line_segment([p_start, p_end], Stroke::new(2.0, ui.visuals().text_color()));
            painter.circle(p_start, 2.0, self.color, Stroke::NONE);
        }
    }

    fn value_ui(&self, ui: &mut Ui, show_value_text: bool) {
        let visuals = ui.visuals().widgets.inactive;
        // let should_draw_frame = ui.visuals().button_frame;
        let should_draw_frame = show_value_text;
        let padding = ui.spacing().button_padding;

        // Handle keyboard entry focus
        let keyboard_focus_id = self.keyboard_focus_id.unwrap();

        let font_id = egui::FontId::monospace(10.0);

        if self.keyboard_entry_active(ui) {
            let value_entry_mutex = ui.memory_mut(|mem| {
                mem.data
                    .get_temp_mut_or_default::<Arc<Mutex<String>>>(*VALUE_ENTRY_MEMORY_ID)
                    .clone()
            });
            let mut value_entry = value_entry_mutex.lock();

            // When editing, show a TextEdit box
            ui.add(
                TextEdit::singleline(&mut *value_entry)
                    .id(keyboard_focus_id)
                    .font(font_id.clone())
                    .horizontal_align(egui::Align::Center) // Center text for knobs
                    .desired_width(self.diameter + padding.x * 2.0),
            );

            if ui.input(|i| i.key_pressed(Key::Escape)) {
                ui.memory_mut(|mem| mem.surrender_focus(keyboard_focus_id));
            } else if ui.input(|i| i.key_pressed(Key::Enter)) {
                self.begin_drag();
                self.set_from_string(&value_entry);
                self.end_drag();
                ui.memory_mut(|mem| mem.surrender_focus(keyboard_focus_id));
            }
        } else {
            let content = if show_value_text {
                self.string_value()
            } else {
                self.param.name().to_string()
            };

            let text = WidgetText::from(content).into_galley(
                ui,
                None,
                ui.available_width(),
                font_id,
            );

            // Center alignment calc
            let text_size = text.size();
            let available_width = self.diameter + (padding.x * 2.0);
            let hit_width = text_size.x.max(available_width);

            let response = ui.allocate_response(vec2(hit_width, text_size.y + padding.y * 2.0), Sense::click());

            if response.clicked() {
                self.begin_keyboard_entry(ui);
            }

            if ui.is_rect_visible(response.rect) {
                if should_draw_frame {
                    let fill = visuals.bg_fill;
                    let stroke = visuals.bg_stroke;
                    ui.painter().rect(
                        response.rect.expand(visuals.expansion),
                        visuals.corner_radius,
                        fill,
                        stroke,
                        egui::StrokeKind::Middle,
                    );
                }

                // Calculate centered position
                let text_pos = response.rect.center() - text_size / 2.0;

                ui.painter().add(egui::epaint::TextShape::new(
                    text_pos,
                    text,
                    if show_value_text {
                        ui.visuals().text_color()
                    } else {
                        visuals.fg_stroke.color
                    },
                ));
            }
        }
    }
}

impl<P: Param> Widget for ParamKnob<'_, P> {
    fn ui(mut self, ui: &mut Ui) -> Response {
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            let knob_size = Vec2::splat(self.diameter);

            let (_knob_rect, mut response) = ui.allocate_exact_size(knob_size, Sense::click_and_drag());

            self.keyboard_focus_id = Some(response.id.with("kb_focus"));

            // Draw and handle knob interaction
            self.knob_ui(ui, &mut response);

            if self.draw_value {
                ui.add_space(2.0);
                let show_value = response.dragged() || response.hovered();
                self.value_ui(ui, show_value);
            }

            response
        })
        .inner
    }
}
