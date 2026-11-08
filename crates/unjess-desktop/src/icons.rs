//! Vector Icon Engine & Custom-Painted UI Controls (`icons.rs`)
//! Draws crisp Lucide-style vector icons via `egui::Painter` (zero emojis / zero mojibake)
//! and provides 1:1 workbench buttons, navigation pills, and badges.
#![allow(dead_code)]

use crate::theme::*;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    PanelLeft,
    PanelRight,
    Folder,
    FolderGit,
    FolderPlus,
    FolderOpen,
    GitBranch,
    Cpu,
    Search,
    Terminal,
    Sliders,
    MessageSquare,
    Bot,
    GitFork,
    Clock,
    Globe,
    Settings,
    Plus,
    X,
    Check,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    ChevronsUpDown,
    EditPencil,
    RotateCcw,
    Trash,
    Zap,
    Map,
    Shield,
    Swords,
    Sparkles,
    Paperclip,
    FileCode,
    FileDiff,
    Send,
    Server,
    Key,
    Eye,
    EyeOff,
    Download,
    Play,
    Refresh,
    RefreshCw,
    Palette,
    Lock,
    Square,
    Edit3,
    Trash2,
    GitMerge,
    BookOpen,
    Share2,
    FilePlus,
    FileText,
    Minimize2,
    Copy,
}

/// Paints a crisp 1.5px-stroke Lucide-style vector icon inside `rect`.
pub fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let r = s * 0.42;
    let stroke = Stroke::new(1.45, color);
    let thin = Stroke::new(1.25, color);

    let pt = |nx: f32, ny: f32| -> Pos2 { Pos2::new(c.x + nx * r, c.y + ny * r) };

    match icon {
        Icon::PanelLeft => {
            let box_r = Rect::from_center_size(c, Vec2::splat(r * 1.8));
            painter.rect_stroke(box_r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            let div_x = box_r.left() + box_r.width() * 0.34;
            painter.line_segment(
                [Pos2::new(div_x, box_r.top()), Pos2::new(div_x, box_r.bottom())],
                stroke,
            );
        }
        Icon::PanelRight => {
            let box_r = Rect::from_center_size(c, Vec2::splat(r * 1.8));
            painter.rect_stroke(box_r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            let div_x = box_r.left() + box_r.width() * 0.66;
            painter.line_segment(
                [Pos2::new(div_x, box_r.top()), Pos2::new(div_x, box_r.bottom())],
                stroke,
            );
        }
        Icon::Folder | Icon::FolderOpen => {
            let p0 = pt(-0.95, -0.65);
            let p1 = pt(-0.25, -0.65);
            let p2 = pt(-0.05, -0.40);
            let p3 = pt(0.95, -0.40);
            let p4 = pt(0.95, 0.70);
            let p5 = pt(-0.95, 0.70);
            painter.add(egui::Shape::closed_line(
                vec![p0, p1, p2, p3, p4, p5],
                stroke,
            ));
        }
        Icon::FolderGit => {
            paint_icon(painter, rect, Icon::Folder, color);
            painter.circle_filled(pt(0.0, 0.15), 1.5, color);
        }
        Icon::FolderPlus => {
            paint_icon(painter, rect, Icon::Folder, color);
            painter.line_segment([pt(0.0, -0.05), pt(0.0, 0.45)], thin);
            painter.line_segment([pt(-0.25, 0.20), pt(0.25, 0.20)], thin);
        }
        Icon::GitBranch => {
            painter.circle_stroke(pt(-0.45, -0.55), r * 0.22, stroke);
            painter.circle_stroke(pt(-0.45, 0.55), r * 0.22, stroke);
            painter.circle_stroke(pt(0.45, -0.25), r * 0.22, stroke);
            painter.line_segment([pt(-0.45, -0.33), pt(-0.45, 0.33)], stroke);
            painter.line_segment([pt(0.45, -0.03), pt(0.15, 0.22)], stroke);
            painter.line_segment([pt(0.15, 0.22), pt(-0.45, 0.22)], stroke);
        }
        Icon::Cpu => {
            let box_r = Rect::from_center_size(c, Vec2::splat(r * 1.35));
            painter.rect_stroke(box_r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            let inner_r = Rect::from_center_size(c, Vec2::splat(r * 0.55));
            painter.rect_stroke(inner_r, CornerRadius::same(1), thin, StrokeKind::Inside);
            for d in [-0.32, 0.32] {
                painter.line_segment([pt(d, -0.95), pt(d, -0.68)], thin);
                painter.line_segment([pt(d, 0.68), pt(d, 0.95)], thin);
                painter.line_segment([pt(-0.95, d), pt(-0.68, d)], thin);
                painter.line_segment([pt(0.68, d), pt(0.95, d)], thin);
            }
        }
        Icon::Search => {
            painter.circle_stroke(pt(-0.15, -0.15), r * 0.62, stroke);
            painter.line_segment([pt(0.30, 0.30), pt(0.88, 0.88)], stroke);
        }
        Icon::Terminal => {
            painter.line_segment([pt(-0.75, -0.45), pt(-0.15, 0.05)], stroke);
            painter.line_segment([pt(-0.15, 0.05), pt(-0.75, 0.55)], stroke);
            painter.line_segment([pt(0.05, 0.55), pt(0.75, 0.55)], stroke);
        }
        Icon::Sliders => {
            for (y, tx) in [(-0.55, -0.25), (0.0, 0.30), (0.55, -0.15)] {
                painter.line_segment([pt(-0.85, y), pt(0.85, y)], thin);
                painter.line_segment([pt(tx, y - 0.22), pt(tx, y + 0.22)], stroke);
            }
        }
        Icon::MessageSquare => {
            let pts = vec![
                pt(-0.85, -0.75),
                pt(0.85, -0.75),
                pt(0.85, 0.40),
                pt(-0.20, 0.40),
                pt(-0.85, 0.85),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Bot => {
            let head = Rect::from_min_max(pt(-0.80, -0.45), pt(0.80, 0.75));
            painter.rect_stroke(head, CornerRadius::same(3), stroke, StrokeKind::Inside);
            painter.line_segment([pt(0.0, -0.85), pt(0.0, -0.45)], stroke);
            painter.circle_filled(pt(0.0, -0.88), 1.4, color);
            painter.circle_filled(pt(-0.32, 0.12), 1.5, color);
            painter.circle_filled(pt(0.32, 0.12), 1.5, color);
        }
        Icon::GitFork | Icon::GitMerge => {
            painter.circle_stroke(pt(-0.48, -0.58), r * 0.22, stroke);
            painter.circle_stroke(pt(0.48, -0.58), r * 0.22, stroke);
            painter.circle_stroke(pt(0.0, 0.62), r * 0.22, stroke);
            painter.line_segment([pt(-0.48, -0.35), pt(0.0, 0.05)], stroke);
            painter.line_segment([pt(0.48, -0.35), pt(0.0, 0.05)], stroke);
            painter.line_segment([pt(0.0, 0.05), pt(0.0, 0.40)], stroke);
        }
        Icon::Clock => {
            painter.circle_stroke(c, r * 0.88, stroke);
            painter.line_segment([c, pt(0.0, -0.48)], stroke);
            painter.line_segment([c, pt(0.38, 0.22)], stroke);
        }
        Icon::Globe => {
            painter.circle_stroke(c, r * 0.88, stroke);
            painter.line_segment([pt(-0.86, 0.0), pt(0.86, 0.0)], thin);
            painter.line_segment([pt(0.0, -0.86), pt(0.0, 0.86)], thin);
        }
        Icon::Settings => {
            painter.circle_stroke(c, r * 0.36, stroke);
            painter.circle_stroke(c, r * 0.72, thin);
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                let p0 = Pos2::new(c.x + a.cos() * r * 0.68, c.y + a.sin() * r * 0.68);
                let p1 = Pos2::new(c.x + a.cos() * r * 0.94, c.y + a.sin() * r * 0.94);
                painter.line_segment([p0, p1], stroke);
            }
        }
        Icon::Plus => {
            painter.line_segment([pt(0.0, -0.75), pt(0.0, 0.75)], stroke);
            painter.line_segment([pt(-0.75, 0.0), pt(0.75, 0.0)], stroke);
        }
        Icon::X => {
            painter.line_segment([pt(-0.65, -0.65), pt(0.65, 0.65)], stroke);
            painter.line_segment([pt(0.65, -0.65), pt(-0.65, 0.65)], stroke);
        }
        Icon::Check => {
            painter.line_segment([pt(-0.72, 0.05), pt(-0.20, 0.58)], stroke);
            painter.line_segment([pt(-0.20, 0.58), pt(0.75, -0.52)], stroke);
        }
        Icon::ChevronDown => {
            painter.line_segment([pt(-0.58, -0.22), pt(0.0, 0.35)], stroke);
            painter.line_segment([pt(0.0, 0.35), pt(0.58, -0.22)], stroke);
        }
        Icon::ChevronUp => {
            painter.line_segment([pt(-0.58, 0.25), pt(0.0, -0.32)], stroke);
            painter.line_segment([pt(0.0, -0.32), pt(0.58, 0.25)], stroke);
        }
        Icon::ChevronRight => {
            painter.line_segment([pt(-0.25, -0.58), pt(0.32, 0.0)], stroke);
            painter.line_segment([pt(0.32, 0.0), pt(-0.25, 0.58)], stroke);
        }
        Icon::ChevronsUpDown => {
            painter.line_segment([pt(-0.45, -0.20), pt(0.0, -0.65)], thin);
            painter.line_segment([pt(0.0, -0.65), pt(0.45, -0.20)], thin);
            painter.line_segment([pt(-0.45, 0.20), pt(0.0, 0.65)], thin);
            painter.line_segment([pt(0.0, 0.65), pt(0.45, 0.20)], thin);
        }
        Icon::EditPencil | Icon::Edit3 => {
            painter.line_segment([pt(-0.65, 0.65), pt(0.55, -0.55)], stroke);
            painter.line_segment([pt(0.30, -0.70), pt(0.70, -0.30)], stroke);
            painter.line_segment([pt(-0.75, 0.75), pt(-0.35, 0.65)], thin);
        }
        Icon::RotateCcw => {
            painter.circle_stroke(c, r * 0.72, thin);
            painter.line_segment([pt(-0.72, -0.55), pt(-0.72, -0.05)], stroke);
            painter.line_segment([pt(-0.72, -0.05), pt(-0.22, -0.05)], stroke);
        }
        Icon::Trash | Icon::Trash2 => {
            painter.line_segment([pt(-0.75, -0.45), pt(0.75, -0.45)], stroke);
            painter.line_segment([pt(-0.30, -0.72), pt(0.30, -0.72)], thin);
            let body = Rect::from_min_max(pt(-0.55, -0.45), pt(0.55, 0.78));
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        Icon::Zap => {
            let pts = vec![
                pt(0.10, -0.88),
                pt(-0.65, 0.08),
                pt(-0.02, 0.08),
                pt(-0.12, 0.88),
                pt(0.65, -0.08),
                pt(0.02, -0.08),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Map => {
            let box_r = Rect::from_min_max(pt(-0.85, -0.65), pt(0.85, 0.65));
            painter.rect_stroke(box_r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment([pt(-0.28, -0.65), pt(-0.28, 0.65)], thin);
            painter.line_segment([pt(0.28, -0.65), pt(0.28, 0.65)], thin);
        }
        Icon::Shield => {
            let pts = vec![
                pt(0.0, -0.85),
                pt(0.75, -0.50),
                pt(0.68, 0.25),
                pt(0.0, 0.88),
                pt(-0.68, 0.25),
                pt(-0.75, -0.50),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Swords => {
            painter.line_segment([pt(-0.75, 0.75), pt(0.75, -0.75)], stroke);
            painter.line_segment([pt(-0.75, -0.75), pt(0.75, 0.75)], stroke);
            painter.line_segment([pt(-0.75, 0.35), pt(-0.35, 0.75)], thin);
            painter.line_segment([pt(0.35, 0.75), pt(0.75, 0.35)], thin);
        }
        Icon::Sparkles => {
            let pts = vec![
                pt(0.0, -0.88),
                pt(0.24, -0.24),
                pt(0.88, 0.0),
                pt(0.24, 0.24),
                pt(0.0, 0.88),
                pt(-0.24, 0.24),
                pt(-0.88, 0.0),
                pt(-0.24, -0.24),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Paperclip => {
            let r_box = Rect::from_min_max(pt(-0.45, -0.75), pt(0.45, 0.75));
            painter.rect_stroke(r_box, CornerRadius::same(4), stroke, StrokeKind::Inside);
            painter.line_segment([pt(0.0, -0.35), pt(0.0, 0.45)], thin);
        }
        Icon::FileCode => {
            let doc = Rect::from_min_max(pt(-0.65, -0.82), pt(0.65, 0.82));
            painter.rect_stroke(doc, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment([pt(-0.30, -0.10), pt(-0.48, 0.12)], thin);
            painter.line_segment([pt(-0.48, 0.12), pt(-0.30, 0.34)], thin);
            painter.line_segment([pt(0.30, -0.10), pt(0.48, 0.12)], thin);
            painter.line_segment([pt(0.48, 0.12), pt(0.30, 0.34)], thin);
        }
        Icon::FileDiff => {
            let doc = Rect::from_min_max(pt(-0.65, -0.82), pt(0.65, 0.82));
            painter.rect_stroke(doc, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment([pt(-0.28, -0.18), pt(0.28, -0.18)], thin);
            painter.line_segment([pt(0.0, -0.44), pt(0.0, 0.08)], thin);
            painter.line_segment([pt(-0.28, 0.38), pt(0.28, 0.38)], thin);
        }
        Icon::Send => {
            let pts = vec![
                pt(0.82, -0.82),
                pt(-0.85, -0.08),
                pt(-0.12, 0.18),
                pt(0.14, 0.85),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
            painter.line_segment([pt(-0.12, 0.18), pt(0.82, -0.82)], thin);
        }
        Icon::Server => {
            let top = Rect::from_min_max(pt(-0.82, -0.75), pt(0.82, -0.10));
            let bot = Rect::from_min_max(pt(-0.82, 0.10), pt(0.82, 0.75));
            painter.rect_stroke(top, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.rect_stroke(bot, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.circle_filled(pt(-0.48, -0.42), 1.4, color);
            painter.circle_filled(pt(-0.48, 0.42), 1.4, color);
        }
        Icon::Key => {
            painter.circle_stroke(pt(-0.38, 0.0), r * 0.42, stroke);
            painter.line_segment([pt(0.04, 0.0), pt(0.85, 0.0)], stroke);
            painter.line_segment([pt(0.55, 0.0), pt(0.55, 0.35)], stroke);
            painter.line_segment([pt(0.82, 0.0), pt(0.82, 0.35)], stroke);
        }
        Icon::Eye => {
            let box_r = Rect::from_min_max(pt(-0.85, -0.45), pt(0.85, 0.45));
            painter.rect_stroke(box_r, CornerRadius::same(6), stroke, StrokeKind::Inside);
            painter.circle_filled(c, r * 0.24, color);
        }
        Icon::EyeOff => {
            paint_icon(painter, rect, Icon::Eye, color);
            painter.line_segment([pt(-0.78, -0.68), pt(0.78, 0.68)], stroke);
        }
        Icon::Download => {
            painter.line_segment([pt(0.0, -0.78), pt(0.0, 0.35)], stroke);
            painter.line_segment([pt(-0.42, -0.05), pt(0.0, 0.38)], stroke);
            painter.line_segment([pt(0.42, -0.05), pt(0.0, 0.38)], stroke);
            painter.line_segment([pt(-0.78, 0.72), pt(0.78, 0.72)], stroke);
        }
        Icon::Play => {
            let pts = vec![pt(-0.55, -0.75), pt(0.75, 0.0), pt(-0.55, 0.75)];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Refresh | Icon::RefreshCw => {
            painter.circle_stroke(c, r * 0.72, stroke);
            painter.line_segment([pt(0.35, -0.72), pt(0.78, -0.72)], stroke);
            painter.line_segment([pt(0.78, -0.72), pt(0.78, -0.28)], stroke);
        }
        Icon::Palette => {
            painter.circle_stroke(c, r * 0.85, stroke);
            painter.circle_filled(pt(-0.32, -0.28), 1.4, color);
            painter.circle_filled(pt(0.28, -0.28), 1.4, color);
            painter.circle_filled(pt(-0.28, 0.28), 1.4, color);
        }
        Icon::Lock => {
            let body = Rect::from_min_max(pt(-0.70, -0.10), pt(0.70, 0.82));
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Inside);
            let shackle = Rect::from_min_max(pt(-0.42, -0.78), pt(0.42, -0.10));
            painter.rect_stroke(shackle, CornerRadius::same(4), thin, StrokeKind::Inside);
        }
        Icon::Square => {
            let sq = Rect::from_center_size(c, Vec2::splat(r * 1.15));
            painter.rect_filled(sq, CornerRadius::same(2), color);
        }
        Icon::BookOpen => {
            let left_p = Rect::from_min_max(pt(-0.88, -0.65), pt(0.0, 0.68));
            let right_p = Rect::from_min_max(pt(0.0, -0.65), pt(0.88, 0.68));
            painter.rect_stroke(left_p, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.rect_stroke(right_p, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        Icon::Share2 => {
            painter.circle_stroke(pt(-0.55, 0.0), r * 0.24, stroke);
            painter.circle_stroke(pt(0.55, -0.55), r * 0.24, stroke);
            painter.circle_stroke(pt(0.55, 0.55), r * 0.24, stroke);
            painter.line_segment([pt(-0.33, -0.12), pt(0.33, -0.43)], thin);
            painter.line_segment([pt(-0.33, 0.12), pt(0.33, 0.43)], thin);
        }
        Icon::FilePlus => {
            let box_r = Rect::from_min_max(pt(-0.70, -0.85), pt(0.70, 0.85));
            painter.rect_stroke(box_r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment([pt(0.0, -0.32), pt(0.0, 0.32)], thin);
            painter.line_segment([pt(-0.32, 0.0), pt(0.32, 0.0)], thin);
        }
        Icon::FileText => {
            let box_r = Rect::from_min_max(pt(-0.70, -0.85), pt(0.70, 0.85));
            painter.rect_stroke(box_r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment([pt(-0.38, -0.25), pt(0.38, -0.25)], thin);
            painter.line_segment([pt(-0.38, 0.10), pt(0.38, 0.10)], thin);
            painter.line_segment([pt(-0.38, 0.45), pt(0.15, 0.45)], thin);
        }
        Icon::Minimize2 => {
            painter.line_segment([pt(-0.75, 0.75), pt(-0.15, 0.15)], stroke);
            painter.line_segment([pt(-0.55, 0.15), pt(-0.15, 0.15)], stroke);
            painter.line_segment([pt(-0.15, 0.55), pt(-0.15, 0.15)], stroke);
            painter.line_segment([pt(0.75, -0.75), pt(0.15, -0.15)], stroke);
            painter.line_segment([pt(0.55, -0.15), pt(0.15, -0.15)], stroke);
            painter.line_segment([pt(0.15, -0.55), pt(0.15, -0.15)], stroke);
        }
        Icon::Copy => {
            let back = Rect::from_min_max(pt(-0.75, -0.75), pt(0.35, 0.35));
            let front = Rect::from_min_max(pt(-0.35, -0.35), pt(0.75, 0.75));
            painter.rect_stroke(back, CornerRadius::same(2), thin, StrokeKind::Inside);
            painter.rect_stroke(front, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
    }
}

/// Allocates a fixed-size icon widget inside an `egui::Ui`.
pub fn icon_widget(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_icon(ui.painter(), rect, icon, color);
    }
    resp
}

/// Square icon button (`26x26` default) with subtle hover/active border.
pub fn icon_btn(ui: &mut Ui, icon: Icon, active: bool, tooltip: &str) -> Response {
    icon_btn_sized(ui, icon, Vec2::splat(26.0), active, tooltip)
}

pub fn icon_btn_sized(
    ui: &mut Ui,
    icon: Icon,
    size: Vec2,
    bordered: bool,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if hovered {
            BG_CARD_HOVER
        } else if bordered {
            BG_CARD
        } else {
            Color32::TRANSPARENT
        };
        let border = if hovered {
            BORDER_HOVER
        } else if bordered {
            BORDER_SUBTLE
        } else {
            Color32::TRANSPARENT
        };
        if bg != Color32::TRANSPARENT || border != Color32::TRANSPARENT {
            ui.painter().rect(
                rect,
                CornerRadius::same(6),
                bg,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
        }
        let fg = if hovered || bordered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_400
        };
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(13.5));
        paint_icon(ui.painter(), icon_rect, icon, fg);
    }
    if !tooltip.is_empty() {
        resp.on_hover_text(tooltip)
    } else {
        resp
    }
}

/// Full-width Left Sidebar Navigation Row (`Chat`, `Agents 5`, `Workflows 3`, `Schedules 0`, `Files 190`, `Browser`, `Settings`).
pub fn sidebar_nav_item(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    badge: Option<&str>,
    selected: bool,
) -> Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if selected {
            BG_CARD_HOVER
        } else if hovered {
            Color32::from_rgb(15, 15, 18)
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if selected {
            Stroke::new(1.0, BORDER_CARD)
        } else {
            Stroke::NONE
        };
        ui.painter()
            .rect(rect, CornerRadius::same(6), bg, stroke, StrokeKind::Inside);

        let icon_color = if selected || hovered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_400
        };
        let text_color = if selected || hovered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_400
        };

        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 17.0, rect.center().y),
            Vec2::splat(14.0),
        );
        paint_icon(ui.painter(), icon_rect, icon, icon_color);

        ui.painter().text(
            Pos2::new(rect.left() + 34.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            text_color,
        );

        if let Some(b_txt) = badge {
            let galley = ui.painter().layout_no_wrap(
                b_txt.to_owned(),
                FontId::monospace(10.0),
                TEXT_SLATE_400,
            );
            let pill_w = (galley.size().x + 10.0).max(18.0);
            let pill_rect = Rect::from_center_size(
                Pos2::new(rect.right() - 8.0 - pill_w * 0.5, rect.center().y),
                Vec2::new(pill_w, 16.0),
            );
            ui.painter().rect_filled(
                pill_rect,
                CornerRadius::same(4),
                Color32::from_rgb(24, 27, 34),
            );
            ui.painter().galley(
                Pos2::new(
                    pill_rect.center().x - galley.size().x * 0.5,
                    pill_rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                TEXT_SLATE_400,
            );
        }
    }
    resp
}

/// Compact Header Action Button (`Subagents`, `Files`, `>_ Terminal`).
pub fn header_pill_btn(ui: &mut Ui, icon: Icon, label: &str, active: bool) -> Response {
    let font = FontId::proportional(11.5);
    let text_w = if label.is_empty() {
        0.0
    } else {
        ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT_SLATE_300).size().x)
    };
    let total_w = 26.0 + text_w + 6.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if active || hovered {
            BG_CARD_HOVER
        } else {
            Color32::TRANSPARENT
        };
        let border = if active {
            BORDER_CARD
        } else if hovered {
            BORDER_SUBTLE
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            bg,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        let fg = if active || hovered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_300
        };
        let ic_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 11.0, rect.center().y),
            Vec2::splat(12.5),
        );
        paint_icon(ui.painter(), ic_rect, icon, fg);
        if !label.is_empty() {
            ui.painter().text(
                Pos2::new(rect.left() + 21.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                font,
                fg,
            );
        }
    }
    resp
}

/// Crisp White Primary CTA Button (`Save All Settings`, `+ Create Custom Skill`, `+ New Subagent`).
pub fn white_cta_btn(ui: &mut Ui, icon: Option<Icon>, label: &str) -> Response {
    let font = FontId::proportional(12.0);
    let text_w = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT_DARK).size().x);
    let icon_pad = if icon.is_some() { 18.0 } else { 0.0 };
    let total_w = text_w + icon_pad + 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if resp.hovered() {
            Color32::WHITE
        } else {
            ACCENT_WHITE
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(7),
            bg,
            Stroke::new(1.0, Color32::from_rgb(228, 228, 231)),
            StrokeKind::Inside,
        );
        let mut x = rect.left() + 11.0;
        if let Some(ic) = icon {
            let ic_rect = Rect::from_center_size(
                Pos2::new(x + 6.0, rect.center().y),
                Vec2::splat(12.5),
            );
            paint_icon(ui.painter(), ic_rect, ic, TEXT_DARK);
            x += 17.0;
        }
        ui.painter().text(
            Pos2::new(x, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            TEXT_DARK,
        );
    }
    resp
}

/// Secondary Dark Outlined Action Button (`Test & Fetch Models`, `Fetch`, `Controls`, `New Chat`).
pub fn dark_action_btn(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    emerald_accent: bool,
) -> Response {
    let text_color = if emerald_accent {
        ACCENT_EMERALD
    } else {
        TEXT_SLATE_200
    };
    let font = FontId::monospace(11.5);
    let text_w = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), text_color).size().x);
    let icon_pad = if icon.is_some() { 17.0 } else { 0.0 };
    let total_w = text_w + icon_pad + 20.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if hovered {
            Color32::from_rgb(32, 38, 48)
        } else {
            BG_SLATE_800
        };
        let border = if hovered { BORDER_HOVER } else { BORDER_CARD };
        ui.painter().rect(
            rect,
            CornerRadius::same(7),
            bg,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        let fg = if hovered && !emerald_accent {
            TEXT_WHITE
        } else {
            text_color
        };
        let mut x = rect.left() + 10.0;
        if let Some(ic) = icon {
            let ic_rect = Rect::from_center_size(
                Pos2::new(x + 5.5, rect.center().y),
                Vec2::splat(12.0),
            );
            paint_icon(ui.painter(), ic_rect, ic, fg);
            x += 16.0;
        }
        ui.painter().text(
            Pos2::new(x, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            fg,
        );
    }
    resp
}

/// Bottom Composer Mode Segment Pill (`Auto | Plan | Supervised | Duel`).
/// Active pill renders solid white `#f4f4f5` with dark `#09090b` text, matching the screenshot!
pub fn mode_segment_pill(ui: &mut Ui, icon: Icon, label: &str, active: bool) -> Response {
    let font = FontId::monospace(11.0);
    let text_w = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT_WHITE).size().x);
    let total_w = text_w + 30.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 23.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if active {
            ACCENT_WHITE
        } else if hovered {
            BG_CARD_HOVER
        } else {
            Color32::TRANSPARENT
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), bg);
        let fg = if active {
            TEXT_DARK
        } else if hovered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_400
        };
        let ic_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 11.0, rect.center().y),
            Vec2::splat(11.5),
        );
        paint_icon(ui.painter(), ic_rect, icon, fg);
        ui.painter().text(
            Pos2::new(rect.left() + 20.5, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            fg,
        );
    }
    resp
}

/// Bottom Composer Right Modifier Pill (`>_ Caveman`, `Ponytail`, `Compact`).
pub fn modifier_pill_btn(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    active: bool,
) -> Response {
    let font = FontId::monospace(10.5);
    let text_w = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT_WHITE).size().x);
    let total_w = text_w + 28.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 23.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let bg = if active {
            Color32::from_rgb(28, 30, 38)
        } else if hovered {
            BG_CARD_HOVER
        } else {
            BG_ACTIVITY
        };
        let border = if active {
            BORDER_ACTIVE
        } else if hovered {
            BORDER_HOVER
        } else {
            BORDER_SUBTLE
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            bg,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        let fg = if active || hovered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_400
        };
        let ic_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 10.5, rect.center().y),
            Vec2::splat(11.0),
        );
        paint_icon(ui.painter(), ic_rect, icon, fg);
        ui.painter().text(
            Pos2::new(rect.left() + 19.5, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            fg,
        );
    }
    resp
}

/// Settings Sub-Tab Pill (`Models & Providers`, `API Keys (12)`, `Skills Library 47`, `MCP Servers 5`, etc.).
pub fn settings_subtab_pill(
    ui: &mut Ui,
    icon: Icon,
    icon_color: Color32,
    label: &str,
    badge_count: Option<usize>,
    active: bool,
) -> Response {
    let badge = badge_count.map(|n| n.to_string());
    let font = FontId::proportional(12.0);
    let text_w = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT_WHITE).size().x);
    let badge_w = badge
        .as_ref()
        .map(|b| (b.len() as f32) * 6.5 + 14.0)
        .unwrap_or(0.0);
    let total_w = 28.0 + text_w + if badge_w > 0.0 { badge_w + 6.0 } else { 4.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let bg = if active {
            BG_CARD
        } else if hovered {
            BG_CARD_HOVER
        } else {
            Color32::TRANSPARENT
        };
        let border = if active {
            BORDER_CARD
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(7),
            bg,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        let fg = if active || hovered {
            TEXT_WHITE
        } else {
            TEXT_SLATE_400
        };
        let ic_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 13.0, rect.center().y),
            Vec2::splat(13.0),
        );
        paint_icon(ui.painter(), ic_rect, icon, icon_color);
        ui.painter().text(
            Pos2::new(rect.left() + 24.5, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            fg,
        );
        if let Some(b_txt) = badge {
            let b_rect = Rect::from_min_max(
                Pos2::new(rect.right() - badge_w - 6.0, rect.center().y - 8.0),
                Pos2::new(rect.right() - 6.0, rect.center().y + 8.0),
            );
            ui.painter().rect(
                b_rect,
                CornerRadius::same(8),
                BG_SLATE_900,
                Stroke::new(1.0, BORDER_SUBTLE),
                StrokeKind::Inside,
            );
            ui.painter().text(
                b_rect.center(),
                Align2::CENTER_CENTER,
                b_txt,
                FontId::monospace(10.0),
                TEXT_SLATE_300,
            );
        }
    }
    resp
}

/// Modern Pill Toggle Switch (`28x15`).
pub fn pill_switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(28.0, 15.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let track_color = if *on {
            Color32::from_rgb(228, 228, 231)
        } else {
            Color32::from_rgb(51, 65, 85)
        };
        let thumb_color = if *on { TEXT_DARK } else { Color32::WHITE };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), track_color);
        let thumb_x = if *on {
            rect.right() - 7.5
        } else {
            rect.left() + 7.5
        };
        ui.painter()
            .circle_filled(Pos2::new(thumb_x, rect.center().y), 5.2, thumb_color);
    }
    resp
}
