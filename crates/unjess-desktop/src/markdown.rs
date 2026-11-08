use crate::icons::{paint_icon, Icon};
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, RichText, Sense, Stroke, StrokeKind, Ui};

/// Renders clean, rich Markdown content directly into an `egui::Ui` matching the
/// dark obsidian typography (`**bold**` bright white text, `` `code` `` pills, fenced code blocks).
pub fn render_markdown(ui: &mut Ui, text: &str) {
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut block_idx = 0usize;

    for raw_line in text.lines() {
        let trimmed = raw_line.trim_end();
        if let Some(fence_rest) = trimmed.strip_prefix("```") {
            if in_code_block {
                render_code_block(ui, &code_lang, &code_buf, block_idx);
                block_idx += 1;
                code_lang.clear();
                code_buf.clear();
                in_code_block = false;
            } else {
                in_code_block = true;
                code_lang = fence_rest.trim().to_string();
            }
            continue;
        }

        if in_code_block {
            if !code_buf.is_empty() {
                code_buf.push('\n');
            }
            code_buf.push_str(raw_line);
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }

        // Horizontal rule
        if trimmed == "---" || trimmed == "***" {
            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter()
                .line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0, theme::BORDER_SUBTLE));
            ui.add_space(4.0);
            continue;
        }

        // Headings
        if let Some(h3) = trimmed.strip_prefix("### ") {
            ui.add_space(4.0);
            render_inline_spans(ui, h3, 14.5, true, theme::TEXT_PRIMARY);
            ui.add_space(2.0);
            continue;
        }
        if let Some(h2) = trimmed.strip_prefix("## ") {
            ui.add_space(6.0);
            render_inline_spans(ui, h2, 16.0, true, theme::TEXT_WHITE);
            ui.add_space(3.0);
            continue;
        }
        if let Some(h1) = trimmed.strip_prefix("# ") {
            ui.add_space(8.0);
            render_inline_spans(ui, h1, 18.0, true, theme::TEXT_WHITE);
            ui.add_space(4.0);
            continue;
        }

        // Bullet list items
        if let Some(bullet) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
        {
            ui.horizontal_top(|ui| {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("•")
                        .size(13.5)
                        .color(theme::TEXT_MUTED),
                );
                ui.vertical(|ui| {
                    render_inline_spans(ui, bullet, 13.5, false, Color32::from_rgb(212, 212, 216));
                });
            });
            ui.add_space(2.0);
            continue;
        }

        // Numbered list items (e.g., "1. ")
        if let Some((num_part, rest)) = parse_numbered_item(trimmed) {
            ui.horizontal_top(|ui| {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("{num_part}."))
                        .size(13.0)
                        .color(theme::TEXT_MUTED)
                        .family(FontFamily::Monospace),
                );
                ui.vertical(|ui| {
                    render_inline_spans(ui, rest, 13.5, false, Color32::from_rgb(212, 212, 216));
                });
            });
            ui.add_space(2.0);
            continue;
        }

        // Standard paragraph line
        render_inline_spans(ui, trimmed, 13.5, false, Color32::from_rgb(212, 212, 216));
        ui.add_space(2.0);
    }

    if in_code_block && !code_buf.is_empty() {
        render_code_block(ui, &code_lang, &code_buf, block_idx);
    }
}

fn parse_numbered_item(line: &str) -> Option<(&str, &str)> {
    let dot_pos = line.find(". ")?;
    if dot_pos == 0 || dot_pos > 3 {
        return None;
    }
    let num_str = &line[..dot_pos];
    if num_str.chars().all(|c| c.is_ascii_digit()) {
        Some((num_str, &line[dot_pos + 2..]))
    } else {
        None
    }
}

fn render_code_block(ui: &mut Ui, lang: &str, code: &str, _idx: usize) {
    ui.add_space(4.0);
    let max_w = ui.available_width();
    egui::Frame::new()
        .fill(Color32::from_rgb(9, 10, 14))
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(0))
        .show(ui, |ui| {
            ui.set_width(max_w);
            ui.set_max_width(max_w);
            // Header bar
            egui::Frame::new()
                .fill(Color32::from_rgb(14, 15, 20))
                .corner_radius(CornerRadius {
                    nw: 8,
                    ne: 8,
                    sw: 0,
                    se: 0,
                })
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(max_w - 24.0);
                    ui.set_max_width(max_w - 24.0);
                    ui.horizontal(|ui| {
                        let label_txt = if lang.is_empty() { "code" } else { lang };
                        ui.label(
                            RichText::new(label_txt)
                                .size(11.0)
                                .family(FontFamily::Monospace)
                                .color(theme::TEXT_MUTED),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(54.0, 20.0), Sense::click());
                            let hovered = resp.hovered();
                            if hovered {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            ui.painter().rect(
                                rect,
                                CornerRadius::same(4),
                                if hovered {
                                    theme::BG_CARD_HOVER
                                } else {
                                    Color32::from_rgb(20, 20, 25)
                                },
                                Stroke::new(1.0, theme::BORDER_SUBTLE),
                                StrokeKind::Inside,
                            );
                            let ic_rect = egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 12.0, rect.center().y),
                                egui::vec2(11.0, 11.0),
                            );
                            paint_icon(ui.painter(), ic_rect, Icon::Copy, theme::TEXT_SECONDARY);
                            ui.painter().text(
                                egui::pos2(rect.left() + 22.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                "Copy",
                                FontId::new(10.5, FontFamily::Proportional),
                                theme::TEXT_SECONDARY,
                            );
                            if resp.clicked() {
                                ui.ctx().copy_text(code.to_string());
                            }
                        });
                    });
                });

            // Code body
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(max_w - 24.0);
                    ui.set_max_width(max_w - 24.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(code)
                                .size(12.0)
                                .family(FontFamily::Monospace)
                                .color(Color32::from_rgb(228, 228, 231)),
                        )
                        .wrap(),
                    );
                });
        });
    ui.add_space(4.0);
}

/// Parses inline `**bold**` and `` `code` `` spans into a wrapped `LayoutJob`.
fn render_inline_spans(ui: &mut Ui, text: &str, font_size: f32, force_bold: bool, base_color: Color32) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut normal_buf = String::new();

    let flush_normal = |job: &mut egui::text::LayoutJob, buf: &mut String| {
        if !buf.is_empty() {
            job.append(
                buf,
                0.0,
                egui::TextFormat {
                    font_id: FontId::new(font_size, FontFamily::Proportional),
                    color: if force_bold { theme::TEXT_WHITE } else { base_color },
                    line_height: Some(font_size * 1.48),
                    ..Default::default()
                },
            );
            buf.clear();
        }
    };

    while i < len {
        // Check for `inline code`
        if chars[i] == '`' {
            let mut end = i + 1;
            while end < len && chars[end] != '`' {
                end += 1;
            }
            if end < len {
                flush_normal(&mut job, &mut normal_buf);
                let code_str: String = chars[i + 1..end].iter().collect();
                job.append(
                    &format!(" {code_str} "),
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::new(font_size - 1.5, FontFamily::Monospace),
                        color: Color32::from_rgb(228, 228, 231),
                        background: Color32::from_rgb(24, 24, 29),
                        line_height: Some(font_size * 1.48),
                        ..Default::default()
                    },
                );
                i = end + 1;
                continue;
            }
        }

        // Check for **bold**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            let mut end = i + 2;
            while end + 1 < len && !(chars[end] == '*' && chars[end + 1] == '*') {
                end += 1;
            }
            if end + 1 < len {
                flush_normal(&mut job, &mut normal_buf);
                let bold_str: String = chars[i + 2..end].iter().collect();
                job.append(
                    &bold_str,
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::new(font_size + 0.3, FontFamily::Proportional),
                        color: Color32::WHITE,
                        line_height: Some(font_size * 1.48),
                        ..Default::default()
                    },
                );
                i = end + 2;
                continue;
            }
        }

        normal_buf.push(chars[i]);
        i += 1;
    }

    flush_normal(&mut job, &mut normal_buf);
    ui.label(job);
}
