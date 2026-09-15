//! Apple-inspired theme.
//!
//! Translates the macOS design language onto egui: the San Francisco
//! system typeface (loaded at runtime, never redistributed), chrome vs
//! viewer surface separation, hairline strokes, 6/8/12 px corner radii,
//! soft wide shadows, and the macOS system-blue accent for selections
//! and primary actions. Light and dark palettes follow AppKit materials
//! (window chrome #F5F5F7 / #26262A over a neutral viewer background).

use eframe::egui;
use egui::{Color32, CornerRadius, FontId, Margin, Shadow, Stroke, TextStyle};

/// Registered semibold face of the system font (see [`install_fonts`]).
pub const FAMILY_SEMIBOLD: &str = "SF Semibold";

/// Install the macOS system font as the proportional family.
/// Returns `true` when the face was found and registered; egui's
/// bundled defaults are kept otherwise.
fn install_fonts(ctx: &egui::Context) -> bool {
    let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") else {
        return false;
    };
    let mut fonts = egui::FontDefinitions::default();

    let mut semibold = egui::FontData::from_owned(bytes.clone());
    semibold.tweak.coords = epaint::text::VariationCoords::new([(b"wght", 590.0)]);

    fonts
        .font_data
        .insert("SF".into(), egui::FontData::from_owned(bytes).into());
    fonts
        .font_data
        .insert(FAMILY_SEMIBOLD.into(), semibold.into());

    let proportional = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    proportional.insert(0, "SF".into());

    let semis = fonts
        .families
        .entry(egui::FontFamily::Name(FAMILY_SEMIBOLD.into()))
        .or_default();
    semis.push(FAMILY_SEMIBOLD.into());
    semis.push("SF".into());

    ctx.set_fonts(fonts);
    true
}

/// Apply the full theme (fonts, type scale, spacing, visuals) to both
/// light and dark styles.
pub fn apply(ctx: &egui::Context) {
    let has_sf = install_fonts(ctx);
    ctx.all_styles_mut(move |style| theme_style(style, has_sf));
}

fn theme_style(style: &mut egui::Style, sf: bool) {
    // — Spacing — macOS 8 px rhythm, roomier widget padding.
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 6.0);
    s.button_padding = egui::vec2(12.0, 6.0);
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(6);
    s.interact_size = egui::vec2(32.0, 24.0);
    s.tooltip_width = 320.0;

    // — Type scale — macOS-style hierarchy (heading 20 semibold,
    // body 13, caption 11). Falls back to the default face off-macOS.
    let heading = if sf {
        FontId::new(20.0, egui::FontFamily::Name(FAMILY_SEMIBOLD.into()))
    } else {
        FontId::proportional(20.0)
    };
    style.text_styles = [
        (TextStyle::Heading, heading),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Small, FontId::proportional(11.0)),
    ]
    .into();

    let v = &mut style.visuals;
    let dark = v.dark_mode;

    let (chrome, extreme, faint, text_strong, text, hairline, hairline_strong, accent) = if dark {
        (
            Color32::from_rgb(38, 38, 42),    // window chrome #26262A
            Color32::from_rgb(26, 26, 28),    // fields #1A1A1C
            Color32::from_rgb(46, 46, 50),    // faint fill
            Color32::from_rgb(245, 245, 247), // label
            Color32::from_rgb(222, 222, 226), // text
            Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            Color32::from_rgb(10, 132, 255), // #0A84FF
        )
    } else {
        (
            Color32::from_rgb(245, 245, 247), // window chrome #F5F5F7
            Color32::WHITE,                   // fields
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(20, 20, 22), // label
            Color32::from_rgb(45, 45, 50),
            Color32::from_rgba_unmultiplied(0, 0, 0, 26),
            Color32::from_rgba_unmultiplied(0, 0, 0, 50),
            Color32::from_rgb(0, 122, 255), // #007AFF
        )
    };

    v.panel_fill = chrome;
    v.extreme_bg_color = extreme;
    v.faint_bg_color = faint;
    v.hyperlink_color = accent;

    v.selection.bg_fill = accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 2,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if dark { 140 } else { 55 }),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if dark { 115 } else { 45 }),
    };

    let (idle_fill, hover_fill, active_fill) = if dark {
        (
            Color32::from_rgba_unmultiplied(255, 255, 255, 26),
            Color32::from_rgba_unmultiplied(255, 255, 255, 48),
            Color32::from_rgba_unmultiplied(255, 255, 255, 70),
        )
    } else {
        (
            Color32::WHITE,
            Color32::from_rgb(250, 250, 252),
            Color32::from_rgb(235, 235, 240),
        )
    };

    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, text);
    w.noninteractive.bg_fill = faint;
    w.noninteractive.bg_stroke = Stroke::new(1.0, hairline);
    w.noninteractive.corner_radius = CornerRadius::same(6);

    for (visual, fill) in [
        (&mut w.inactive, idle_fill),
        (&mut w.hovered, hover_fill),
        (&mut w.active, active_fill),
        (&mut w.open, hover_fill),
    ] {
        visual.fg_stroke = Stroke::new(1.0, text);
        visual.weak_bg_fill = fill;
        visual.bg_fill = extreme;
        visual.bg_stroke = Stroke::new(1.0, hairline);
        visual.corner_radius = CornerRadius::same(6);
    }
    w.hovered.fg_stroke = Stroke::new(1.0, text_strong);
    w.active.fg_stroke = Stroke::new(1.0, text_strong);
    w.open.fg_stroke = Stroke::new(1.0, text_strong);
    w.hovered.bg_stroke = Stroke::new(1.0, hairline_strong);
    w.active.bg_stroke = Stroke::new(1.0, hairline_strong);
}

// — Runtime helpers (read the active style) —

/// The macOS system-blue accent of the current theme.
pub fn accent(ui: &egui::Ui) -> Color32 {
    ui.visuals().selection.bg_fill
}

/// Viewer (central document area) background of the current theme.
pub fn viewer_bg(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(28, 28, 30)
    } else {
        Color32::from_rgb(236, 236, 241)
    }
}

/// Chrome (toolbar / sidebars / panels) background of the current theme.
pub fn chrome(ui: &egui::Ui) -> Color32 {
    ui.visuals().panel_fill
}

/// A 1 px hairline appropriate to the current theme.
pub fn hairline(ui: &egui::Ui) -> Stroke {
    Stroke::new(
        1.0,
        if ui.visuals().dark_mode {
            Color32::from_rgba_unmultiplied(255, 255, 255, 20)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 26)
        },
    )
}

/// Secondary-label text color of the current theme.
pub fn secondary_text(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(152, 152, 159)
    } else {
        Color32::from_rgb(110, 110, 115)
    }
}

/// System green / red that stay legible in dark mode.
pub fn system_green(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(48, 209, 88)
    } else {
        Color32::from_rgb(52, 199, 89)
    }
}
pub fn system_red(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(255, 69, 58)
    } else {
        Color32::from_rgb(255, 59, 48)
    }
}

/// A macOS-style default (primary) action button: accent fill, white label.
pub fn primary_button(ui: &egui::Ui, label: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).strong())
        .fill(accent(ui))
        .stroke(Stroke::NONE)
        .corner_radius(6)
}

/// Frame for toolbar / status bar / sidebars: chrome fill over the viewer.
pub fn chrome_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new().fill(chrome(ui))
}

/// Paint a hairline across the bottom edge of `ui`'s panel (toolbar).
pub fn hairline_bottom(ui: &mut egui::Ui) {
    let rect = ui.clip_rect();
    let stroke = hairline(ui);
    ui.painter()
        .hline(rect.left()..=rect.right(), rect.bottom() - 0.5, stroke);
}

/// Paint a hairline across the top edge of `ui`'s panel (status bar).
pub fn hairline_top(ui: &mut egui::Ui) {
    let rect = ui.clip_rect();
    let stroke = hairline(ui);
    ui.painter()
        .hline(rect.left()..=rect.right(), rect.top() + 0.5, stroke);
}

/// Paint a hairline down the right edge of `ui`'s panel (left sidebar).
pub fn hairline_right(ui: &mut egui::Ui) {
    let rect = ui.clip_rect();
    let stroke = hairline(ui);
    ui.painter()
        .vline(rect.right() - 0.5, rect.top()..=rect.bottom(), stroke);
}

/// Paint a hairline down the left edge of `ui`'s panel (right-side panels).
pub fn hairline_left(ui: &mut egui::Ui) {
    let rect = ui.clip_rect();
    let stroke = hairline(ui);
    ui.painter()
        .vline(rect.left() + 0.5, rect.top()..=rect.bottom(), stroke);
}

/// Soft drop shadow shape placed under a page rect in the viewer.
pub fn page_shadow(ui: &egui::Ui, rect: egui::Rect) -> egui::Shape {
    let dark = ui.visuals().dark_mode;
    Shadow {
        offset: [0, 4],
        blur: 18,
        spread: 1,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if dark { 130 } else { 70 }),
    }
    .as_shape(rect, 2)
    .into()
}

/// macOS-style segmented control: renders `segments` as a pill track with
/// an elevated active segment. Returns the index of the segment that was
/// clicked, if any. `active` is the currently selected index.
pub fn segmented_control(ui: &mut egui::Ui, active: usize, segments: &[&str]) -> Option<usize> {
    let dark = ui.visuals().dark_mode;
    let mut clicked = None;
    let track = if dark {
        Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        Color32::from_rgba_unmultiplied(120, 120, 128, 34)
    };
    let full_width = ui.available_width();
    egui::Frame::new()
        .fill(track)
        .corner_radius(8)
        .inner_margin(2.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                let seg_width = (full_width - 8.0) / segments.len() as f32;
                for (i, label) in segments.iter().enumerate() {
                    let is_active = i == active;
                    let (fill, text_color) = if is_active {
                        (
                            if dark {
                                Color32::from_rgb(66, 66, 70)
                            } else {
                                Color32::WHITE
                            },
                            if dark {
                                Color32::from_rgb(245, 245, 247)
                            } else {
                                Color32::from_rgb(20, 20, 22)
                            },
                        )
                    } else {
                        (Color32::TRANSPARENT, secondary_text(ui))
                    };
                    let text = egui::RichText::new(*label).color(text_color).size(12.0);
                    let btn = egui::Button::new(text)
                        .fill(fill)
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(6)
                        .min_size(egui::vec2(seg_width, 22.0));
                    if ui.add(btn).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
    clicked
}
