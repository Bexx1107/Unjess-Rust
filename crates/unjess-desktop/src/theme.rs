//! Unjess Design System Theme (`theme.rs`)
//! Exact color palette, font loader, and `egui::Visuals` matching the Unjess dark obsidian workbench.
#![allow(dead_code)]

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Visuals};
use std::fs;
use std::sync::Arc;

// Exact palette from the Unjess workbench design system
pub const BG_MAIN: Color32 = Color32::from_rgb(5, 5, 5); // #050505
pub const BG_SIDEBAR: Color32 = Color32::from_rgb(10, 10, 11); // #0a0a0b
pub const BG_HEADER: Color32 = Color32::from_rgb(10, 10, 11); // #0a0a0b
pub const BG_CARD: Color32 = Color32::from_rgb(17, 17, 19); // #111113
pub const BG_CARD_HOVER: Color32 = Color32::from_rgb(24, 24, 27); // #18181b
pub const BG_AGENT_CARD: Color32 = Color32::from_rgb(12, 12, 15); // #0c0c0f
pub const BG_USER_BUBBLE: Color32 = Color32::from_rgb(20, 20, 23); // #141417
pub const BG_ACTIVITY: Color32 = Color32::from_rgb(9, 9, 11); // #09090b
pub const BG_INPUT: Color32 = Color32::from_rgb(13, 17, 23); // #0d1117
pub const BG_SLATE_800: Color32 = Color32::from_rgb(30, 34, 42); // #1e222a
pub const BG_SLATE_900: Color32 = Color32::from_rgb(15, 18, 24); // #0f1218

pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(24, 24, 27); // #18181b
pub const BORDER_CARD: Color32 = Color32::from_rgb(31, 31, 35); // #1f1f23
pub const BORDER_HOVER: Color32 = Color32::from_rgb(48, 48, 54); // #303036
pub const BORDER_ACTIVE: Color32 = Color32::from_rgb(72, 72, 82); // #484852

pub const TEXT_WHITE: Color32 = Color32::from_rgb(244, 244, 245); // #f4f4f5
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(228, 228, 231); // #e4e4e7
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(161, 161, 170); // #a1a1aa
pub const TEXT_SLATE_200: Color32 = Color32::from_rgb(228, 228, 231); // #e4e4e7
pub const TEXT_SLATE_300: Color32 = Color32::from_rgb(212, 212, 216); // #d4d4d8
pub const TEXT_SLATE_400: Color32 = Color32::from_rgb(161, 161, 170); // #a1a1aa
pub const TEXT_MUTED: Color32 = Color32::from_rgb(113, 113, 122); // #71717a
pub const TEXT_DIM: Color32 = Color32::from_rgb(82, 82, 91); // #52525b
pub const TEXT_DARK: Color32 = Color32::from_rgb(9, 9, 11); // #09090b
pub const TEXT_DARK_ON_WHITE: Color32 = Color32::from_rgb(9, 9, 11); // #09090b

pub const ACCENT_WHITE: Color32 = Color32::from_rgb(244, 244, 245); // #f4f4f5
pub const ACCENT_EMERALD: Color32 = Color32::from_rgb(52, 211, 153); // #34d399
pub const ACCENT_EMERALD_BG: Color32 = Color32::from_rgb(6, 44, 34); // #062c22
pub const ACCENT_SKY: Color32 = Color32::from_rgb(56, 189, 248); // #38bdf8
pub const ACCENT_CYAN: Color32 = Color32::from_rgb(125, 211, 252); // #7dd3fc
pub const ACCENT_SKY_BG: Color32 = Color32::from_rgb(8, 47, 73); // #082f49
pub const ACCENT_RED: Color32 = Color32::from_rgb(248, 113, 113); // #f87171
pub const ACCENT_RED_BG: Color32 = Color32::from_rgb(69, 10, 10); // #450a0a
pub const ACCENT_AMBER: Color32 = Color32::from_rgb(251, 191, 36); // #fbbf24
pub const ACCENT_PURPLE: Color32 = Color32::from_rgb(192, 132, 252); // #c084fc
pub const ACCENT_INDIGO: Color32 = Color32::from_rgb(129, 140, 248); // #818cf8

/// Loads crisp system UI & Monospace fonts (`Segoe UI` + `Cascadia Mono` / `Consolas`) with safe fallbacks.
pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // Proportional UI font (Segoe UI on Windows)
    let prop_candidates = [
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
    ];
    for path in prop_candidates {
        if let Ok(bytes) = fs::read(path) {
            fonts.font_data.insert(
                "unjess_sans".to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                fam.insert(0, "unjess_sans".to_owned());
            }
            break;
        }
    }

    // Monospace code font (Cascadia Mono or Consolas on Windows)
    let mono_candidates = [
        r"C:\Windows\Fonts\CascadiaMono.ttf",
        r"C:\Windows\Fonts\consola.ttf",
    ];
    for path in mono_candidates {
        if let Ok(bytes) = fs::read(path) {
            fonts.font_data.insert(
                "unjess_mono".to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
                fam.insert(0, "unjess_mono".to_owned());
            }
            break;
        }
    }

    ctx.set_fonts(fonts);
}

/// Applies the dark obsidian Unjess visual style to `egui::Context`.
pub fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG_MAIN;
    visuals.window_fill = BG_CARD;
    visuals.faint_bg_color = BG_SIDEBAR;
    visuals.extreme_bg_color = BG_INPUT;
    visuals.code_bg_color = Color32::from_rgb(24, 24, 30);
    visuals.window_stroke = Stroke::new(1.0, BORDER_CARD);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.override_text_color = None;

    // Widget states
    visuals.widgets.noninteractive.bg_fill = BG_CARD;
    visuals.widgets.noninteractive.weak_bg_fill = BG_CARD;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SLATE_200);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(7);

    visuals.widgets.inactive.bg_fill = BG_CARD;
    visuals.widgets.inactive.weak_bg_fill = BG_CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_CARD);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SLATE_300);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(7);

    visuals.widgets.hovered.bg_fill = BG_CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_HOVER);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_WHITE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(7);

    visuals.widgets.active.bg_fill = Color32::from_rgb(32, 32, 36);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(32, 32, 36);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, BORDER_ACTIVE);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT_WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(7);

    visuals.widgets.open.bg_fill = BG_CARD_HOVER;
    visuals.widgets.open.weak_bg_fill = BG_CARD_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, BORDER_HOVER);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT_WHITE);
    visuals.widgets.open.corner_radius = CornerRadius::same(7);

    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(56, 189, 248, 45);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_CYAN);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.scroll.bar_width = 5.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style.spacing.scroll.bar_outer_margin = 2.0;
    ctx.set_style(style);
}

pub fn install_fonts(ctx: &egui::Context) {
    configure_fonts(ctx);
}

pub fn apply_obsidian_theme(ctx: &egui::Context) {
    configure_visuals(ctx);
}

