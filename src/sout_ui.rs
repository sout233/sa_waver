use nih_plug_egui::egui::{
    self,
    style::{self, Selection}, Color32, Shadow, Stroke, Visuals,
};

/// Apply the given theme to a [`Context`](egui::Context).
pub fn set_theme(ctx: &egui::Context, theme: SoutTheme) {
    let _old = ctx.style().visuals.clone();
    ctx.set_visuals(theme.visuals());
}

/// Apply the given theme to a [`Style`](egui::Style).
pub fn set_style_theme(style: &mut egui::Style, theme: SoutTheme) {
    style.visuals = theme.visuals();
}

fn make_widget_visual(old: style::WidgetVisuals, _theme: &SoutTheme, bg_fill: egui::Color32) -> style::WidgetVisuals {
    style::WidgetVisuals {
        bg_fill,
        weak_bg_fill: bg_fill,
        bg_stroke: egui::Stroke {
            color: Color32::from_hex("#34302E").unwrap(),
            ..old.bg_stroke
        },
        fg_stroke: egui::Stroke {
            color: Color32::from_hex("#FFEAD0").unwrap(),
            ..old.fg_stroke
        },
        ..old
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct SoutTheme {}

impl SoutTheme {
    pub fn new() -> Self {
        SoutTheme {}
    }

    fn visuals(&self) -> egui::Visuals {
        let old = egui::Visuals::dark();

        egui::Visuals {
            dark_mode: true,
            override_text_color: Some(Color32::from_hex("#FFEAD0").unwrap()),
            widgets: style::Widgets {
                noninteractive: make_widget_visual(old.widgets.noninteractive, self, Color32::from_hex("#34302E").unwrap()),
                inactive: make_widget_visual(old.widgets.inactive, self, Color32::from_hex("#34302E").unwrap()),
                hovered: make_widget_visual(old.widgets.hovered, self, Color32::from_hex("#34302E").unwrap().additive()),
                active: make_widget_visual(old.widgets.active, self, Color32::from_hex("#34302E").unwrap()),
                open: make_widget_visual(old.widgets.open, self, Color32::from_hex("#34302E").unwrap()),
            },
            selection: Selection {
                bg_fill: Color32::from_hex("#FFCBA8").unwrap().gamma_multiply(0.2),
                stroke: Stroke {
                    width: 1.0,
                    color: Color32::from_hex("#FFCBA8").unwrap().blend(Color32::WHITE),
                },
            },
            // hyperlink_color: todo!(),
            // faint_bg_color: todo!(),
            // extreme_bg_color: todo!(),
            // code_bg_color: todo!(),
            // warn_fg_color: todo!(),
            // error_fg_color: todo!(),
            // window_corner_radius: todo!(),
            // window_shadow: todo!(),
            window_fill: Color32::from_hex("#1C1917").unwrap(),
            // window_stroke: todo!(),
            // window_highlight_topmost: todo!(),
            // menu_corner_radius: todo!(),
            // panel_fill: todo!(),
            popup_shadow: Shadow {
                offset: [4, 8],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(80),
            },
            // resize_corner_size: todo!(),
            // text_cursor: todo!(),
            // clip_rect_margin: todo!(),
            // button_frame: todo!(),
            // collapsing_header_frame: todo!(),
            // indent_has_left_vline: todo!(),
            // striped: todo!(),
            // slider_trailing_fill: todo!(),
            // handle_shape: todo!(),
            // interact_cursor: todo!(),
            // image_loading_spinners: todo!(),
            // numeric_color_space: todo!(),
            ..old
        }
    }
}

pub fn make_ghost_button_visuals(visuals: &mut Visuals) {
    let stroke_color = egui::Color32::from_hex("#FFEAD0").unwrap();
    let stroke = egui::Stroke::new(1.0, stroke_color);

    visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(0.0, stroke_color); // 文字颜色
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(4);

    visuals.widgets.hovered.weak_bg_fill = stroke_color.gamma_multiply(0.1);
    visuals.widgets.hovered.bg_stroke = stroke;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, stroke_color);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(4);

    visuals.widgets.active.weak_bg_fill = stroke_color.gamma_multiply(0.25);
    visuals.widgets.active.bg_stroke = stroke;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, stroke_color);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(4);
}

pub fn make_combobox_visuals(visuals: &mut Visuals, bg: Color32) {
    let stroke_color = egui::Color32::from_hex("#FFEAD0").unwrap();
    let stroke = egui::Stroke::new(1.0, stroke_color);

    visuals.widgets.inactive.weak_bg_fill = bg;
    visuals.widgets.inactive.bg_stroke = stroke;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, stroke_color);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(4);

    visuals.widgets.hovered.weak_bg_fill = bg.gamma_multiply(0.8);
    visuals.widgets.hovered.bg_stroke = stroke;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, stroke_color);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(4);

    visuals.widgets.active.weak_bg_fill = bg.gamma_multiply(0.6);
    visuals.widgets.active.bg_stroke = stroke;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, stroke_color);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(4);
}

pub fn make_btn_visuals(visuals: &mut Visuals, bg: Color32, stroke: Color32, fg: Color32) {
    let stroke_color = stroke;
    let stroke = egui::Stroke::new(1.0, stroke_color);

    visuals.widgets.inactive.weak_bg_fill = bg;
    visuals.widgets.inactive.bg_stroke = stroke;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, fg);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(4);

    visuals.widgets.hovered.weak_bg_fill = bg.gamma_multiply(0.8);
    visuals.widgets.hovered.bg_stroke = stroke;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, fg);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(4);

    visuals.widgets.active.weak_bg_fill = bg.gamma_multiply(0.6);
    visuals.widgets.active.bg_stroke = stroke;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, fg);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(4);
}
