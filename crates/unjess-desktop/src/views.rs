use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::icons::{
    dark_action_btn, paint_icon, pill_switch, settings_subtab_pill, white_cta_btn, Icon,
};
use crate::markdown::render_markdown;
use crate::theme;
use crate::{MainTab, SettingsSubTab, UnjessDesktopApp};
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Ui,
};
use unjess_core::{
    builtin_changelog, outline_file, scan_workspace, McpManager, MemoryStore, ScheduledTask,
    SchedulerManager, Settings, SkillsManager, SubagentDef, ToolEngine, WorkflowDef, Workspace,
    CURRENT_VERSION,
};

impl UnjessDesktopApp {
    /// Renders the `Agents` (Subagent Fleet & Custom Subagent Studio) main tab.
    pub fn render_agents_tab(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("agents_tab_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(920.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        // Header
                        ui.horizontal(|ui| {
                            let (ic_rect, _) =
                                ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                            paint_icon(ui.painter(), ic_rect, Icon::Cpu, theme::ACCENT_EMERALD);
                            ui.label(
                                RichText::new("Autonomous Subagent Fleet")
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if white_cta_btn(ui, Some(Icon::Plus), "New Subagent").clicked()
                                    {
                                        self.subagent_edit_id.clear();
                                        self.subagent_name_buf.clear();
                                        self.subagent_role_buf.clear();
                                        self.subagent_prompt_buf.clear();
                                        self.subagent_model_buf = "inherit".to_string();
                                        self.show_subagent_modal = true;
                                    }
                                },
                            );
                        });
                        ui.label(
                            RichText::new("Delegate specialized coding, reverse-engineering, and security auditing tasks to isolated worker agents.")
                                .size(12.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(16.0);

                        // Quick Dispatch Bar
                        egui::Frame::new()
                            .fill(theme::BG_CARD)
                            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("DISPATCH TASK TO SUBAGENT")
                                        .size(11.0)
                                        .strong()
                                        .color(theme::TEXT_SECONDARY),
                                );
                                ui.add_space(8.0);
                                let subagents = self.hm.subagents();
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("dispatch_sub_select")
                                        .selected_text(
                                            subagents
                                                .iter()
                                                .find(|s| s.id == self.selected_subagent_id)
                                                .map(|s| s.name.as_str())
                                                .unwrap_or("Select Subagent"),
                                        )
                                        .width(200.0)
                                        .show_ui(ui, |ui| {
                                            for s in &subagents {
                                                ui.selectable_value(
                                                    &mut self.selected_subagent_id,
                                                    s.id.clone(),
                                                    &s.name,
                                                );
                                            }
                                        });

                                    let resp = ui.add_sized(
                                        egui::vec2(ui.available_width() - 120.0, 30.0),
                                        TextEdit::singleline(&mut self.subagent_task_buf)
                                            .hint_text("Describe task for subagent worker..."),
                                    );
                                    if (resp.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                        || white_cta_btn(ui, Some(Icon::Play), "Dispatch").clicked()
                                    {
                                        let task = self.subagent_task_buf.trim().to_string();
                                        if !task.is_empty() {
                                            let sub_id = if self.selected_subagent_id.is_empty() {
                                                subagents
                                                    .first()
                                                    .map(|s| s.id.clone())
                                                    .unwrap_or_else(|| "sub-coder".to_string())
                                            } else {
                                                self.selected_subagent_id.clone()
                                            };
                                            self.hm.spawn_subagent_task_with_conv(&sub_id, &task, None);
                                            self.subagent_task_buf.clear();
                                            self.status_toast = Some((
                                                format!("Dispatched task to {sub_id}"),
                                                std::time::Instant::now(),
                                            ));
                                        }
                                    }
                                });
                            });

                        ui.add_space(16.0);

                        // Subagent Cards
                        let subagents = self.hm.subagents();
                        for sub in subagents {
                            self.render_subagent_card(ui, &sub);
                            ui.add_space(10.0);
                        }
                    });
                });
                ui.add_space(32.0);
            });
    }

    fn render_subagent_card(&mut self, ui: &mut Ui, sub: &SubagentDef) {
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let dot_col = match sub.status.as_str() {
                        "running" => theme::ACCENT_EMERALD,
                        "aborted" => theme::ACCENT_RED,
                        _ => theme::TEXT_MUTED,
                    };
                    let (dot_r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                    ui.painter().circle_filled(dot_r.center(), 4.0, dot_col);

                    ui.label(
                        RichText::new(&sub.name)
                            .size(14.5)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                    egui::Frame::new()
                        .fill(Color32::from_rgb(24, 24, 27))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&sub.model)
                                    .size(11.0)
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_SECONDARY),
                            );
                        });
                    egui::Frame::new()
                        .fill(Color32::from_rgb(20, 24, 32))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(sub.status.to_uppercase())
                                    .size(10.5)
                                    .family(FontFamily::Monospace)
                                    .color(dot_col),
                            );
                        });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if dark_action_btn(ui, Some(Icon::Trash2), "Delete", false).clicked() {
                            self.pending_delete = Some(crate::PendingDeleteAction::Subagent {
                                id: sub.id.clone(),
                                name: sub.name.clone(),
                            });
                        }
                        if dark_action_btn(ui, Some(Icon::Edit3), "Edit", false).clicked() {
                            self.subagent_edit_id = sub.id.clone();
                            self.subagent_name_buf = sub.name.clone();
                            self.subagent_role_buf = sub.role.clone();
                            self.subagent_prompt_buf = sub.system_prompt.clone();
                            self.subagent_model_buf = sub.model.clone();
                            self.show_subagent_modal = true;
                        }
                        if sub.status == "running" {
                            if dark_action_btn(ui, Some(Icon::Square), "Abort", true).clicked() {
                                self.hm.abort_subagent(&sub.id);
                            }
                        } else if dark_action_btn(ui, Some(Icon::Play), "Select", false).clicked() {
                            self.selected_subagent_id = sub.id.clone();
                        }
                    });
                });

                ui.add_space(4.0);
                ui.label(
                    RichText::new(&sub.role)
                        .size(12.5)
                        .color(theme::ACCENT_SKY),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(&sub.system_prompt)
                        .size(12.0)
                        .color(theme::TEXT_SECONDARY),
                );

                if !sub.last_output.is_empty() {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(theme::BG_ACTIVITY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Latest Output:")
                                    .size(11.0)
                                    .strong()
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.add_space(2.0);
                            let preview: String = sub.last_output.chars().take(600).collect();
                            ui.label(
                                RichText::new(preview)
                                    .size(11.5)
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_PRIMARY),
                            );
                        });
                }
            });
    }

    /// Renders the `Workflows` main tab.
    pub fn render_workflows_tab(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("workflows_tab_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(920.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.horizontal(|ui| {
                            let (ic_rect, _) =
                                ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                            paint_icon(
                                ui.painter(),
                                ic_rect,
                                Icon::GitMerge,
                                theme::ACCENT_PURPLE,
                            );
                            ui.label(
                                RichText::new("Multi-Step Engineering Workflows")
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if white_cta_btn(ui, Some(Icon::Plus), "New Workflow").clicked()
                                    {
                                        self.wf_edit_id.clear();
                                        self.wf_name_buf.clear();
                                        self.wf_desc_buf.clear();
                                        self.wf_steps_buf.clear();
                                        self.show_workflow_modal = true;
                                    }
                                },
                            );
                        });
                        ui.label(
                            RichText::new("Reusable autonomous playbooks for security audits, refactoring pipelines, and release verification.")
                                .size(12.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(16.0);

                        let workflows = self.hm.workflows();
                        for wf in workflows {
                            egui::Frame::new()
                                .fill(theme::BG_CARD)
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::same(16))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&wf.name)
                                                .size(14.5)
                                                .strong()
                                                .color(theme::TEXT_WHITE),
                                        );
                                        egui::Frame::new()
                                            .fill(Color32::from_rgb(24, 24, 27))
                                            .corner_radius(CornerRadius::same(4))
                                            .inner_margin(Margin::symmetric(6, 2))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(wf.category.to_uppercase())
                                                        .size(10.5)
                                                        .family(FontFamily::Monospace)
                                                        .color(theme::ACCENT_PURPLE),
                                                );
                                            });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if dark_action_btn(
                                                    ui,
                                                    Some(Icon::Trash2),
                                                    "Delete",
                                                    false,
                                                )
                                                .clicked()
                                                {
                                                    self.pending_delete =
                                                        Some(crate::PendingDeleteAction::Workflow {
                                                            id: wf.id.clone(),
                                                            name: wf.name.clone(),
                                                        });
                                                }
                                                if dark_action_btn(
                                                    ui,
                                                    Some(Icon::Edit3),
                                                    "Edit",
                                                    false,
                                                )
                                                .clicked()
                                                {
                                                    self.wf_edit_id = wf.id.clone();
                                                    self.wf_name_buf = wf.name.clone();
                                                    self.wf_desc_buf = wf.description.clone();
                                                    self.wf_steps_buf = wf.steps.join("\n");
                                                    self.show_workflow_modal = true;
                                                }
                                                if white_cta_btn(
                                                    ui,
                                                    Some(Icon::Play),
                                                    "Run Workflow",
                                                )
                                                .clicked()
                                                {
                                                    let prompt = format!(
                                                        "Execute Workflow '{}':\n{}",
                                                        wf.name,
                                                        wf.steps.join("\n")
                                                    );
                                                    self.hm.submit_prompt(prompt);
                                                    self.active_tab = MainTab::Chat;
                                                }
                                            },
                                        );
                                    });
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(&wf.description)
                                            .size(12.5)
                                            .color(theme::TEXT_SECONDARY),
                                    );
                                    if !wf.steps.is_empty() {
                                        ui.add_space(8.0);
                                        for (idx, step) in wf.steps.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(format!("{}.", idx + 1))
                                                        .size(11.5)
                                                        .family(FontFamily::Monospace)
                                                        .color(theme::TEXT_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new(step)
                                                        .size(12.0)
                                                        .color(theme::TEXT_PRIMARY),
                                                );
                                            });
                                        }
                                    }
                                });
                            ui.add_space(10.0);
                        }
                    });
                });
                ui.add_space(32.0);
            });
    }

    /// Renders the `Schedules` (Cron & Interval Background Tasks) main tab.
    pub fn render_schedules_tab(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("schedules_tab_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(920.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.horizontal(|ui| {
                            let (ic_rect, _) =
                                ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                            paint_icon(ui.painter(), ic_rect, Icon::Clock, theme::ACCENT_AMBER);
                            ui.label(
                                RichText::new("Scheduled Background Automations")
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if white_cta_btn(ui, Some(Icon::Plus), "New Schedule").clicked()
                                    {
                                        self.sched_name_buf.clear();
                                        self.sched_cron_buf = "every 1h".to_string();
                                        self.sched_prompt_buf.clear();
                                        self.sched_action_buf = "prompt".to_string();
                                        self.show_schedule_modal = true;
                                    }
                                },
                            );
                        });
                        ui.label(
                            RichText::new("Persistent cron & interval jobs executed automatically by the background ticker.")
                                .size(12.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(16.0);

                        if self.schedules_cache.is_empty() {
                            egui::Frame::new()
                                .fill(theme::BG_CARD)
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::same(32))
                                .show(ui, |ui| {
                                    ui.vertical_centered(|ui| {
                                        let (ic_r, _) = ui.allocate_exact_size(
                                            egui::vec2(28.0, 28.0),
                                            Sense::hover(),
                                        );
                                        paint_icon(
                                            ui.painter(),
                                            ic_r,
                                            Icon::Clock,
                                            theme::TEXT_MUTED,
                                        );
                                        ui.add_space(8.0);
                                        ui.label(
                                            RichText::new("No scheduled tasks configured yet.")
                                                .size(13.5)
                                                .strong()
                                                .color(theme::TEXT_PRIMARY),
                                        );
                                        ui.label(
                                            RichText::new("Click 'New Schedule' above to run recurring prompts or shell checks.")
                                                .size(12.0)
                                                .color(theme::TEXT_MUTED),
                                        );
                                    });
                                });
                        } else {
                            let tasks = self.schedules_cache.clone();
                            let mut changed = false;
                            for task in tasks {
                                egui::Frame::new()
                                    .fill(theme::BG_CARD)
                                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                    .corner_radius(CornerRadius::same(10))
                                    .inner_margin(Margin::same(16))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let mut enabled = task.enabled;
                                            if pill_switch(ui, &mut enabled).changed() {
                                                if let Some(slot) = self
                                                    .schedules_cache
                                                    .iter_mut()
                                                    .find(|s| s.id == task.id)
                                                {
                                                    slot.enabled = enabled;
                                                    changed = true;
                                                }
                                            }
                                            ui.label(
                                                RichText::new(&task.name)
                                                    .size(14.0)
                                                    .strong()
                                                    .color(theme::TEXT_WHITE),
                                            );
                                            egui::Frame::new()
                                                .fill(Color32::from_rgb(24, 24, 27))
                                                .corner_radius(CornerRadius::same(4))
                                                .inner_margin(Margin::symmetric(6, 2))
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        RichText::new(&task.cron_or_interval)
                                                            .size(11.0)
                                                            .family(FontFamily::Monospace)
                                                            .color(theme::ACCENT_AMBER),
                                                    );
                                                });

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if dark_action_btn(
                                                        ui,
                                                        Some(Icon::Trash2),
                                                        "Delete",
                                                        false,
                                                    )
                                                    .clicked()
                                                    {
                                                        self.pending_delete = Some(
                                                            crate::PendingDeleteAction::Schedule {
                                                                id: task.id.clone(),
                                                                name: task.name.clone(),
                                                            },
                                                        );
                                                    }
                                                    if dark_action_btn(
                                                        ui,
                                                        Some(Icon::Play),
                                                        "Run Now",
                                                        true,
                                                    )
                                                    .clicked()
                                                    {
                                                        let hm = std::sync::Arc::clone(&self.hm);
                                                        let tid = task.id.clone();
                                                        let _ = SchedulerManager::execute_task(
                                                            &hm, &tid,
                                                        );
                                                        self.schedules_cache =
                                                            SchedulerManager::load();
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(4.0);
                                        ui.label(
                                            RichText::new(&task.prompt)
                                                .size(12.0)
                                                .family(FontFamily::Monospace)
                                                .color(theme::TEXT_SECONDARY),
                                        );
                                        if let Some(ref last) = task.last_run {
                                            ui.add_space(4.0);
                                            ui.label(
                                                RichText::new(format!(
                                                    "Last run: {} ({})",
                                                    last,
                                                    task.last_status
                                                        .as_deref()
                                                        .unwrap_or("completed")
                                                ))
                                                .size(11.0)
                                                .color(theme::TEXT_MUTED),
                                            );
                                        }
                                    });
                                ui.add_space(10.0);
                            }
                            if changed {
                                let _ = SchedulerManager::save(&self.schedules_cache);
                            }
                        }
                    });
                });
                ui.add_space(32.0);
            });
    }

    /// Renders the `Files` (Workspace File Explorer, Editor & AST Code Intelligence) main tab.
    pub fn render_files_tab(&mut self, ui: &mut Ui) {
        ui.columns(2, |cols| {
            // Left column: File tree & search
            cols[0].vertical(|ui| {
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ic_r, _) =
                                ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                            paint_icon(ui.painter(), ic_r, Icon::Folder, theme::ACCENT_EMERALD);
                            ui.label(
                                RichText::new(format!(
                                    "Workspace Files ({})",
                                    self.workspace_files.len()
                                ))
                                .size(13.0)
                                .strong()
                                .color(theme::TEXT_WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if dark_action_btn(ui, Some(Icon::RefreshCw), "Refresh", false)
                                        .clicked()
                                    {
                                        self.refresh_workspace_cache();
                                    }
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.add_sized(
                            egui::vec2(ui.available_width(), 28.0),
                            TextEdit::singleline(&mut self.file_filter_buf)
                                .hint_text("Filter files by name or path..."),
                        );
                        ui.add_space(8.0);

                        let filter = self.file_filter_buf.trim().to_lowercase();
                        ScrollArea::vertical()
                            .id_salt("files_tree_scroll")
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - 16.0)
                            .show(ui, |ui| {
                                let files = self.workspace_files.clone();
                                for f in files {
                                    if !filter.is_empty() && !f.to_lowercase().contains(&filter) {
                                        continue;
                                    }
                                    let selected = self.selected_file_path == f;
                                    let resp = ui.selectable_label(
                                        selected,
                                        RichText::new(&f)
                                            .size(12.0)
                                            .family(FontFamily::Monospace)
                                            .color(if selected {
                                                theme::TEXT_WHITE
                                            } else {
                                                theme::TEXT_SECONDARY
                                            }),
                                    );
                                    if resp.clicked() {
                                        self.selected_file_path = f.clone();
                                        let full = self.hm.active_workspace_path().join(&f);
                                        self.selected_file_content =
                                            std::fs::read_to_string(&full).unwrap_or_default();
                                        self.ast_symbols = outline_file(&full);
                                    }
                                }
                            });
                    });
            });

            // Right column: File Editor & AST Outline
            cols[1].vertical(|ui| {
                egui::Frame::new()
                    .fill(theme::BG_CARD)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let title = if self.selected_file_path.is_empty() {
                                "Select a file to inspect or edit"
                            } else {
                                &self.selected_file_path
                            };
                            ui.label(
                                RichText::new(title)
                                    .size(13.0)
                                    .strong()
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_WHITE),
                            );
                            if !self.selected_file_path.is_empty() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if white_cta_btn(ui, Some(Icon::Check), "Save File")
                                            .clicked()
                                        {
                                            let full = self
                                                .hm
                                                .active_workspace_path()
                                                .join(&self.selected_file_path);
                                            if std::fs::write(&full, &self.selected_file_content)
                                                .is_ok()
                                            {
                                                self.status_toast = Some((
                                                    format!("Saved {}", self.selected_file_path),
                                                    std::time::Instant::now(),
                                                ));
                                            }
                                        }
                                    },
                                );
                            }
                        });
                        ui.add_space(8.0);

                        if !self.ast_symbols.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new("AST Symbols:")
                                        .size(11.0)
                                        .color(theme::TEXT_MUTED),
                                );
                                for sym in self.ast_symbols.iter().take(12) {
                                    egui::Frame::new()
                                        .fill(Color32::from_rgb(24, 24, 27))
                                        .corner_radius(CornerRadius::same(4))
                                        .inner_margin(Margin::symmetric(6, 2))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} {} (L{})",
                                                    sym.kind, sym.name, sym.line
                                                ))
                                                .size(10.5)
                                                .family(FontFamily::Monospace)
                                                .color(theme::ACCENT_CYAN),
                                            );
                                        });
                                }
                            });
                            ui.add_space(6.0);
                        }

                        ScrollArea::vertical()
                            .id_salt("file_editor_scroll")
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - 16.0)
                            .show(ui, |ui| {
                                ui.add_sized(
                                    ui.available_size(),
                                    TextEdit::multiline(&mut self.selected_file_content)
                                        .code_editor()
                                        .desired_width(f32::INFINITY),
                                );
                            });
                    });
            });
        });
    }

    /// Renders the `Browser` (Web Search & Security Duel Research Scanner) main tab.
    pub fn render_browser_tab(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("browser_tab_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(920.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.horizontal(|ui| {
                            let (ic_r, _) =
                                ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                            paint_icon(ui.painter(), ic_r, Icon::Globe, theme::ACCENT_SKY);
                            ui.label(
                                RichText::new("Web Research & Security Scanner")
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_WHITE),
                            );
                        });
                        ui.label(
                            RichText::new("Fetch live web documentation, inspect URLs, or run a Red Team vs. White Team Security Duel scan.")
                                .size(12.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(16.0);

                        egui::Frame::new()
                            .fill(theme::BG_CARD)
                            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let resp = ui.add_sized(
                                        egui::vec2((ui.available_width() - 255.0).max(200.0), 30.0),
                                        TextEdit::singleline(&mut self.browser_query_buf)
                                            .hint_text("Enter search query or https:// URL to fetch..."),
                                    );
                                    let is_url = self.browser_query_buf.trim().starts_with("http://")
                                        || self.browser_query_buf.trim().starts_with("https://");
                                    let btn_label = if is_url { "Fetch URL" } else { "Search Web" };
                                    if (resp.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                        || white_cta_btn(ui, Some(Icon::Search), btn_label)
                                            .clicked()
                                    {
                                        let q = self.browser_query_buf.trim().to_string();
                                        if !q.is_empty() {
                                            if let Ok(ws) =
                                                Workspace::new(self.hm.active_workspace_path())
                                            {
                                                let te = ToolEngine::new(ws);
                                                self.browser_result_buf = if is_url {
                                                    te.read_url(&q).unwrap_or_else(|e| e.to_string())
                                                } else {
                                                    te.web_search(&q).unwrap_or_else(|e| e.to_string())
                                                };
                                            }
                                        }
                                    }
                                    if dark_action_btn(
                                        ui,
                                        Some(Icon::Swords),
                                        "Security Scan",
                                        true,
                                    )
                                    .clicked()
                                    {
                                        let report =
                                            scan_workspace(&self.hm.active_workspace_path());
                                        let mut out = format!(
                                            "# Security Duel Report\nFindings: {} | Security Score: {}/100\n\n{}",
                                            report.red_team_findings.len(), report.score, report.summary
                                        );
                                        for f in &report.red_team_findings {
                                            out.push_str(&format!(
                                                "\n\n### [{}] {} ({}:{})\n- **Finding**: {}\n- **White Team Patch**: {}\n```\n{}\n```",
                                                f.severity.to_uppercase(),
                                                f.category,
                                                f.file,
                                                f.line,
                                                f.title,
                                                f.remediation,
                                                f.snippet
                                            ));
                                        }
                                        self.browser_result_buf = out;
                                    }
                                });
                            });

                        ui.add_space(16.0);
                        if !self.browser_result_buf.is_empty() {
                            egui::Frame::new()
                                .fill(theme::BG_CARD)
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::same(18))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("Research Output")
                                                .size(12.5)
                                                .strong()
                                                .color(theme::TEXT_SECONDARY),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if dark_action_btn(
                                                    ui,
                                                    Some(Icon::Send),
                                                    "Send Results to Chat",
                                                    true,
                                                )
                                                .clicked()
                                                {
                                                    let prompt = format!(
                                                        "Analyze these web research / scan results for `{}`:\n\n{}",
                                                        self.browser_query_buf.trim(),
                                                        self.browser_result_buf
                                                    );
                                                    self.hm.submit_prompt(prompt);
                                                    self.active_tab = MainTab::Chat;
                                                }
                                                if dark_action_btn(
                                                    ui,
                                                    Some(Icon::X),
                                                    "Clear",
                                                    false,
                                                )
                                                .clicked()
                                                {
                                                    self.browser_result_buf.clear();
                                                }
                                            },
                                        );
                                    });
                                    ui.add_space(8.0);
                                    render_markdown(ui, &self.browser_result_buf);
                                });
                        }
                    });
                });
                ui.add_space(32.0);
            });
    }

    /// Renders the `Settings & Control Center` matching `media_1794094817258_ea42a648.png` and `media_1794090367693_85aa24ac.jpg` 1:1.
    pub fn render_settings_tab(&mut self, ui: &mut Ui) {
        // Top Header Band (#0a0a0b background, 1px bottom border)
        egui::Frame::new()
            .fill(theme::BG_SIDEBAR)
            .inner_margin(Margin::symmetric(24, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.set_max_width(940.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let (ic_r, _) = ui.allocate_exact_size(
                                        egui::vec2(18.0, 18.0),
                                        Sense::hover(),
                                    );
                                    paint_icon(
                                        ui.painter(),
                                        ic_r,
                                        Icon::Sliders,
                                        theme::TEXT_SECONDARY,
                                    );
                                    ui.label(
                                        RichText::new("Settings & Control Center")
                                            .size(18.0)
                                            .strong()
                                            .color(theme::TEXT_WHITE),
                                    );
                                });
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new("Configure LLM providers, API keys, skills library, MCP servers, and agent behavior.")
                                        .size(12.0)
                                        .color(theme::TEXT_MUTED),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if white_cta_btn(ui, Some(Icon::Check), "Save All Settings")
                                        .clicked()
                                    {
                                        if !self.custom_model_override_buf.trim().is_empty() {
                                            self.settings_draft.model =
                                                self.custom_model_override_buf.trim().to_string();
                                        }
                                        self.hm.update_settings(self.settings_draft.clone());
                                        self.discovered_models =
                                            self.settings_draft.discover_models();
                                        self.status_toast = Some((
                                            "Saved all settings to ~/.unjess-rust".to_string(),
                                            std::time::Instant::now(),
                                        ));
                                    }
                                },
                            );
                        });

                        ui.add_space(14.0);

                        // 7 Sub-Tab Pills
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if settings_subtab_pill(
                                ui,
                                Icon::Cpu,
                                theme::ACCENT_EMERALD,
                                "Models & Providers",
                                None,
                                self.settings_subtab == SettingsSubTab::ModelsProviders,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::ModelsProviders;
                            }
                            if settings_subtab_pill(
                                ui,
                                Icon::Key,
                                theme::ACCENT_AMBER,
                                "API Keys (12)",
                                None,
                                self.settings_subtab == SettingsSubTab::ApiKeys,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::ApiKeys;
                            }
                            if settings_subtab_pill(
                                ui,
                                Icon::BookOpen,
                                theme::ACCENT_PURPLE,
                                "Skills Library",
                                Some(self.skills_cache.len()),
                                self.settings_subtab == SettingsSubTab::SkillsLibrary,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::SkillsLibrary;
                            }
                            if settings_subtab_pill(
                                ui,
                                Icon::Server,
                                theme::ACCENT_SKY,
                                "MCP Servers",
                                Some(self.mcp_cache.len()),
                                self.settings_subtab == SettingsSubTab::McpServers,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::McpServers;
                            }
                            if settings_subtab_pill(
                                ui,
                                Icon::Sliders,
                                theme::ACCENT_CYAN,
                                "Behavior & Context",
                                None,
                                self.settings_subtab == SettingsSubTab::BehaviorContext,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::BehaviorContext;
                            }
                            if settings_subtab_pill(
                                ui,
                                Icon::Palette,
                                Color32::from_rgb(244, 114, 182),
                                "Display, Budget & RAG",
                                None,
                                self.settings_subtab == SettingsSubTab::DisplayBudgetRag,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::DisplayBudgetRag;
                            }
                            if settings_subtab_pill(
                                ui,
                                Icon::RefreshCw,
                                theme::ACCENT_EMERALD,
                                "Updates & Changelog",
                                None,
                                self.settings_subtab == SettingsSubTab::UpdatesChangelog,
                            )
                            .clicked()
                            {
                                self.settings_subtab = SettingsSubTab::UpdatesChangelog;
                            }
                        });
                    });
                });
            });

        // 1px bottom border under Settings header band
        let (sep_r, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().line_segment(
            [sep_r.left_center(), sep_r.right_center()],
            Stroke::new(1.0, theme::BORDER_SUBTLE),
        );

        // Scrollable Settings Content Body
        ScrollArea::vertical()
            .id_salt("settings_body_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(940.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        match self.settings_subtab {
                            SettingsSubTab::ModelsProviders => {
                                self.render_settings_models_providers(ui)
                            }
                            SettingsSubTab::ApiKeys => self.render_settings_api_keys(ui),
                            SettingsSubTab::SkillsLibrary => {
                                self.render_settings_skills_library(ui)
                            }
                            SettingsSubTab::McpServers => self.render_settings_mcp_servers(ui),
                            SettingsSubTab::BehaviorContext => {
                                self.render_settings_behavior_context(ui)
                            }
                            SettingsSubTab::DisplayBudgetRag => {
                                self.render_settings_display_budget_rag(ui)
                            }
                            SettingsSubTab::UpdatesChangelog => {
                                self.render_settings_updates_changelog(ui)
                            }
                        }
                    });
                });
                ui.add_space(40.0);
            });
    }

    fn render_settings_models_providers(&mut self, ui: &mut Ui) {
        // Card 1: PRIMARY PROVIDER & MODEL EXECUTION (exact match to media_1794094817258_ea42a648.png)
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ic_r, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                    paint_icon(ui.painter(), ic_r, Icon::Cpu, theme::ACCENT_EMERALD);
                    ui.label(
                        RichText::new("PRIMARY PROVIDER & MODEL EXECUTION")
                            .size(11.5)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(12, 16, 15))
                            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (dr, _) = ui.allocate_exact_size(
                                        egui::vec2(6.0, 6.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().circle_filled(
                                        dr.center(),
                                        3.0,
                                        theme::ACCENT_EMERALD,
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "Connected ({} available models)",
                                            self.discovered_models.len()
                                        ))
                                        .size(11.0)
                                        .family(FontFamily::Monospace)
                                        .color(theme::TEXT_SECONDARY),
                                    );
                                });
                            });
                    });
                });

                ui.add_space(16.0);

                // 2-Column Row: Active LLM Provider | Active / Default Model
                let col_w = (ui.available_width() - 16.0) * 0.5;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(
                            RichText::new("Active LLM Provider")
                                .size(11.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        let providers = [
                            ("ollama", "Ollama (Local Server)"),
                            ("ollama_cloud", "Ollama Cloud"),
                            ("google", "Google Gemini"),
                            ("anthropic", "Anthropic Claude"),
                            ("openai", "OpenAI"),
                            ("groq", "Groq"),
                            ("mistral", "Mistral"),
                            ("xai", "xAI (Grok)"),
                            ("openrouter", "OpenRouter"),
                            ("cerebras", "Cerebras"),
                            ("kimi", "Kimi (Moonshot)"),
                            ("qwen", "Qwen (DashScope)"),
                            ("lmstudio", "LM Studio (Local)"),
                            ("llamacpp", "llama.cpp (Local)"),
                        ];
                        let current_label = providers
                            .iter()
                            .find(|(k, _)| *k == self.settings_draft.provider)
                            .map(|(_, l)| *l)
                            .unwrap_or("Ollama (Local Server)");
                        egui::ComboBox::from_id_salt("settings_active_provider")
                            .selected_text(
                                RichText::new(current_label)
                                    .size(12.5)
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_WHITE),
                            )
                            .width(col_w)
                            .show_ui(ui, |ui| {
                                for (k, label) in providers {
                                    ui.selectable_value(
                                        &mut self.settings_draft.provider,
                                        k.to_string(),
                                        label,
                                    );
                                }
                            });
                    });

                    ui.add_space(12.0);

                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(
                            RichText::new("Active / Default Model")
                                .size(11.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        egui::ComboBox::from_id_salt("settings_active_model")
                            .selected_text(
                                RichText::new(&self.settings_draft.model)
                                    .size(12.5)
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_WHITE),
                            )
                            .width(col_w)
                            .show_ui(ui, |ui| {
                                for m in &self.discovered_models {
                                    ui.selectable_value(
                                        &mut self.settings_draft.model,
                                        m.id.clone(),
                                        format!("{} ({})", m.id, m.provider),
                                    );
                                }
                            });
                    });
                });

                ui.add_space(14.0);

                // Subagent Worker Model Strategy
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Subagent Worker Model Strategy")
                            .size(11.5)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Controls how background subagents choose their LLM")
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                        );
                    });
                });
                ui.add_space(4.0);
                let sub_strategy_label = if self.settings_draft.ask_before_subagent_spawn {
                    "Ask Me Before Spawning (Pick model per subagent)".to_string()
                } else if self.settings_draft.subagent_model == "inherit"
                    || self.settings_draft.subagent_model.is_empty()
                {
                    "Inherit Active Main Model (Default)".to_string()
                } else {
                    self.settings_draft.subagent_model.clone()
                };
                egui::ComboBox::from_id_salt("settings_subagent_strategy")
                    .selected_text(
                        RichText::new(sub_strategy_label)
                            .size(12.5)
                            .family(FontFamily::Monospace)
                            .color(theme::TEXT_WHITE),
                    )
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                self.settings_draft.ask_before_subagent_spawn,
                                "Ask Me Before Spawning (Pick model per subagent)",
                            )
                            .clicked()
                        {
                            self.settings_draft.ask_before_subagent_spawn = true;
                        }
                        if ui
                            .selectable_label(
                                !self.settings_draft.ask_before_subagent_spawn
                                    && self.settings_draft.subagent_model == "inherit",
                                "Inherit Active Main Model (Default)",
                            )
                            .clicked()
                        {
                            self.settings_draft.ask_before_subagent_spawn = false;
                            self.settings_draft.subagent_model = "inherit".to_string();
                        }
                        for m in &self.discovered_models {
                            if ui
                                .selectable_label(
                                    !self.settings_draft.ask_before_subagent_spawn
                                        && self.settings_draft.subagent_model == m.id,
                                    format!("Always use {}", m.id),
                                )
                                .clicked()
                            {
                                self.settings_draft.ask_before_subagent_spawn = false;
                                self.settings_draft.subagent_model = m.id.clone();
                            }
                        }
                    });

                ui.add_space(14.0);

                // Custom Model Name Override
                ui.label(
                    RichText::new(
                        "Custom Model Name Override (Optional — type model identifier if not listed in dropdown above)",
                    )
                    .size(11.5)
                    .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(4.0);
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    TextEdit::singleline(&mut self.custom_model_override_buf)
                        .font(FontId::new(12.5, FontFamily::Monospace))
                        .hint_text("e.g. qwen2.5-coder:14b, deepseek-r1:14b, claude-3-5-sonnet-latest"),
                );

                ui.add_space(14.0);

                // Ollama Server URL (Local or LAN)
                ui.label(
                    RichText::new("Ollama Server URL (Local or LAN)")
                        .size(11.5)
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        egui::vec2(ui.available_width() - 165.0, 32.0),
                        TextEdit::singleline(&mut self.settings_draft.ollama_base_url)
                            .font(FontId::new(12.5, FontFamily::Monospace)),
                    );
                    if dark_action_btn(ui, Some(Icon::RefreshCw), "Test & Fetch Models", true)
                        .clicked()
                    {
                        let url = self.settings_draft.ollama_base_url.clone();
                        match Settings::probe_provider_endpoint("ollama", &url) {
                            Ok(local_models) => {
                                self.discovered_models = self.settings_draft.discover_models();
                                let preview = if local_models.is_empty() {
                                    "server online (0 local models pulled yet)".to_string()
                                } else {
                                    let sample: Vec<_> =
                                        local_models.iter().take(4).cloned().collect();
                                    format!(
                                        "{} local model(s) ready ({})",
                                        local_models.len(),
                                        sample.join(", ")
                                    )
                                };
                                let msg = format!("✓ Connected to {} — {}", url, preview);
                                self.ollama_probe_status = Some((true, msg.clone()));
                                self.status_toast = Some((msg, std::time::Instant::now()));
                            }
                            Err(err) => {
                                self.discovered_models = self.settings_draft.discover_models();
                                let msg = format!("✗ {}", err);
                                self.ollama_probe_status = Some((false, msg.clone()));
                                self.status_toast = Some((
                                    format!(
                                        "Ollama unreachable at {} ({} cloud models still available)",
                                        url,
                                        self.discovered_models.len()
                                    ),
                                    std::time::Instant::now(),
                                ));
                            }
                        }
                    }
                });
                if let Some((ok, status_txt)) = &self.ollama_probe_status {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(status_txt)
                            .size(11.5)
                            .family(FontFamily::Monospace)
                            .color(if *ok {
                                theme::ACCENT_EMERALD
                            } else {
                                theme::ACCENT_RED
                            }),
                    );
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Default: http://localhost:11434. For remote Ollama servers: http://10.6.0.x:11434",
                    )
                    .size(11.0)
                    .color(theme::TEXT_DIM),
                );
            });

        ui.add_space(18.0);

        // Card 2: ADDITIONAL ENDPOINT & PROVIDER BASE URLS
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ic_r, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                    paint_icon(ui.painter(), ic_r, Icon::Share2, theme::ACCENT_PURPLE);
                    ui.label(
                        RichText::new("ADDITIONAL ENDPOINT & PROVIDER BASE URLS")
                            .size(11.5)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                });

                ui.add_space(16.0);
                let col_w = (ui.available_width() - 16.0) * 0.5;

                // Row 1: Ollama Cloud Base URL | LM Studio Server URL + Fetch
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(
                            RichText::new("Ollama Cloud Base URL")
                                .size(11.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        ui.add_sized(
                            egui::vec2(col_w, 32.0),
                            TextEdit::singleline(&mut self.settings_draft.ollama_api_base_url)
                                .font(FontId::new(12.5, FontFamily::Monospace)),
                        );
                    });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(
                            RichText::new("LM Studio Server URL")
                                .size(11.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(col_w - 82.0, 32.0),
                                TextEdit::singleline(&mut self.settings_draft.lmstudio_base_url)
                                    .font(FontId::new(12.5, FontFamily::Monospace)),
                            );
                            if dark_action_btn(ui, Some(Icon::RefreshCw), "Fetch", true).clicked()
                            {
                                let url = self.settings_draft.lmstudio_base_url.clone();
                                match Settings::probe_provider_endpoint("LM Studio", &url) {
                                    Ok(models) => {
                                        self.discovered_models =
                                            self.settings_draft.discover_models();
                                        let msg = format!(
                                            "✓ Connected ({} LM Studio model(s) loaded)",
                                            models.len()
                                        );
                                        self.lmstudio_probe_status = Some((true, msg.clone()));
                                        self.status_toast =
                                            Some((msg, std::time::Instant::now()));
                                    }
                                    Err(err) => {
                                        self.lmstudio_probe_status =
                                            Some((false, format!("✗ {}", err)));
                                        self.status_toast = Some((
                                            format!("LM Studio Fetch: {}", err),
                                            std::time::Instant::now(),
                                        ));
                                    }
                                }
                            }
                        });
                        if let Some((ok, status_txt)) = &self.lmstudio_probe_status {
                            ui.add_space(3.0);
                            ui.label(
                                RichText::new(status_txt)
                                    .size(10.5)
                                    .family(FontFamily::Monospace)
                                    .color(if *ok {
                                        theme::ACCENT_EMERALD
                                    } else {
                                        theme::ACCENT_RED
                                    }),
                            );
                        }
                    });
                });

                ui.add_space(14.0);

                // Row 2: llama.cpp Base URL + Fetch | Kimi (Moonshot) Base URL
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(
                            RichText::new("llama.cpp Base URL")
                                .size(11.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(col_w - 82.0, 32.0),
                                TextEdit::singleline(&mut self.settings_draft.llamacpp_base_url)
                                    .font(FontId::new(12.5, FontFamily::Monospace)),
                            );
                            if dark_action_btn(ui, Some(Icon::RefreshCw), "Fetch", true).clicked()
                            {
                                let url = self.settings_draft.llamacpp_base_url.clone();
                                match Settings::probe_provider_endpoint("llama.cpp", &url) {
                                    Ok(models) => {
                                        self.discovered_models =
                                            self.settings_draft.discover_models();
                                        let msg = format!(
                                            "✓ Connected ({} llama.cpp model(s) loaded)",
                                            models.len()
                                        );
                                        self.llamacpp_probe_status = Some((true, msg.clone()));
                                        self.status_toast =
                                            Some((msg, std::time::Instant::now()));
                                    }
                                    Err(err) => {
                                        self.llamacpp_probe_status =
                                            Some((false, format!("✗ {}", err)));
                                        self.status_toast = Some((
                                            format!("llama.cpp Fetch: {}", err),
                                            std::time::Instant::now(),
                                        ));
                                    }
                                }
                            }
                        });
                        if let Some((ok, status_txt)) = &self.llamacpp_probe_status {
                            ui.add_space(3.0);
                            ui.label(
                                RichText::new(status_txt)
                                    .size(10.5)
                                    .family(FontFamily::Monospace)
                                    .color(if *ok {
                                        theme::ACCENT_EMERALD
                                    } else {
                                        theme::ACCENT_RED
                                    }),
                            );
                        }
                    });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(
                            RichText::new("Kimi (Moonshot) Base URL")
                                .size(11.5)
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        ui.add_sized(
                            egui::vec2(col_w, 32.0),
                            TextEdit::singleline(&mut self.settings_draft.kimi_base_url)
                                .font(FontId::new(12.5, FontFamily::Monospace)),
                        );
                    });
                });

                ui.add_space(14.0);

                // Row 3: Qwen (DashScope) Base URL
                ui.label(
                    RichText::new("Qwen (DashScope) Base URL")
                        .size(11.5)
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(4.0);
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    TextEdit::singleline(&mut self.settings_draft.qwen_base_url)
                        .font(FontId::new(12.5, FontFamily::Monospace)),
                );
            });
    }

    fn render_settings_api_keys(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ic_r, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                    paint_icon(ui.painter(), ic_r, Icon::Key, theme::ACCENT_AMBER);
                    ui.label(
                        RichText::new("CLOUD PROVIDER API KEYS (12 PROVIDERS)")
                            .size(11.5)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if dark_action_btn(
                            ui,
                            None,
                            if self.show_api_keys_plain {
                                "Hide Keys"
                            } else {
                                "Show Keys"
                            },
                            false,
                        )
                        .clicked()
                        {
                            self.show_api_keys_plain = !self.show_api_keys_plain;
                        }
                    });
                });
                ui.add_space(14.0);

                let key_defs = [
                    ("google", "Google Gemini API Key", "AIzaSy..."),
                    ("anthropic", "Anthropic Claude API Key", "sk-ant-..."),
                    ("openai", "OpenAI API Key", "sk-proj-..."),
                    ("ollama_cloud", "Ollama Cloud API Key", "ollama_..."),
                    ("groq", "Groq Cloud API Key", "gsk_..."),
                    ("openrouter", "OpenRouter API Key", "sk-or-v1-..."),
                    ("xai", "xAI (Grok) API Key", "xai-..."),
                    ("mistral", "Mistral AI API Key", "mistral-..."),
                    ("cerebras", "Cerebras API Key", "csk-..."),
                    ("kimi", "Kimi (Moonshot) API Key", "sk-..."),
                    ("qwen", "Qwen (DashScope) API Key", "sk-..."),
                ];

                let col_w = (ui.available_width() - 16.0) * 0.5;
                for pair in key_defs.chunks(2) {
                    ui.horizontal(|ui| {
                        for (k, label, hint) in pair {
                            ui.vertical(|ui| {
                                ui.set_width(col_w);
                                let entry = self
                                    .settings_draft
                                    .api_keys
                                    .entry((*k).to_string())
                                    .or_default();
                                let has_val = !entry.trim().is_empty();
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(*label)
                                            .size(11.5)
                                            .color(theme::TEXT_SECONDARY),
                                    );
                                    if has_val {
                                        ui.label(
                                            RichText::new("● Configured")
                                                .size(10.5)
                                                .color(theme::ACCENT_EMERALD),
                                        );
                                    }
                                });
                                ui.add_space(4.0);
                                ui.add_sized(
                                    egui::vec2(col_w, 30.0),
                                    TextEdit::singleline(entry)
                                        .password(!self.show_api_keys_plain)
                                        .font(FontId::new(12.0, FontFamily::Monospace))
                                        .hint_text(*hint),
                                );
                            });
                            ui.add_space(12.0);
                        }
                    });
                    ui.add_space(10.0);
                }

            });
    }

    fn render_settings_skills_library(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ic_r, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                    paint_icon(ui.painter(), ic_r, Icon::BookOpen, theme::ACCENT_PURPLE);
                    ui.label(
                        RichText::new("AGENT SKILLS (SKILL.MD)")
                            .size(11.5)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if dark_action_btn(ui, Some(Icon::Plus), "Create Skill", false).clicked() {
                            self.skill_edit_id.clear();
                            self.skill_name_buf.clear();
                            self.skill_desc_buf.clear();
                            self.skill_folder_buf = "General".to_string();
                            self.skill_triggers_buf.clear();
                            self.skill_body_buf.clear();
                            self.show_skill_modal = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Skills are loaded from ~/.unjess-rust/skills/<name>/SKILL.md and workspace .agents/skills/<name>/SKILL.md.")
                        .size(11.5)
                        .color(theme::TEXT_MUTED),
                );

                ui.add_space(14.0);

                // GitHub Skill Installer Box (exact match to media_1794090367693_85aa24ac.jpg)
                egui::Frame::new()
                    .fill(theme::BG_ACTIVITY)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("INSTALL SKILL FROM GITHUB REPOSITORY")
                                .size(10.5)
                                .strong()
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(ui.available_width() - 150.0, 30.0),
                                TextEdit::singleline(&mut self.github_skill_repo_buf)
                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                    .hint_text("owner/repo or https://github.com/owner/repo"),
                            );
                            if white_cta_btn(ui, None, "Install from GitHub").clicked() {
                                let repo = self.github_skill_repo_buf.trim().to_string();
                                if !repo.is_empty() {
                                    self.install_skill_preset(&repo, "");
                                }
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            ui.label(
                                RichText::new("Curated 1-Click Skill Presets:")
                                    .size(11.0)
                                    .color(theme::TEXT_DIM),
                            );
                            let presets = [
                                ("morluto/rea", "+ Reverse Engineer Anything (REA)"),
                                ("nextlevelbuilder/ui-ux-pro-max-skill", "+ UI/UX Pro Max"),
                                ("vxcontrol/pentagi", "+ PentAGI Pentesting"),
                                ("OWASP/secure-headers-project", "+ OWASP Secure Headers"),
                                ("github/awesome-copilot", "+ GDPR & Privacy Audit"),
                                ("sijunhe/apple-design-skill", "+ Apple HIG UI Design"),
                            ];
                            for (repo, label) in presets {
                                if dark_action_btn(ui, None, label, false).clicked() {
                                    self.install_skill_preset(
                                        repo,
                                        if repo == "github/awesome-copilot" {
                                            "gdpr-compliant"
                                        } else {
                                            ""
                                        },
                                    );
                                }
                            }
                        });
                    });

                ui.add_space(14.0);

                // Search bar + Scope Filter Pills (`All`, `Workspace`, `Global`, `Built-in`)
                ui.horizontal(|ui| {
                    let search_w = (ui.available_width() - 285.0).max(220.0);
                    ui.add_sized(
                        egui::vec2(search_w, 30.0),
                        TextEdit::singleline(&mut self.skill_search_buf)
                            .hint_text("Search skills by name, trigger, folder, or description..."),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (scope_key, scope_label) in [
                            ("builtin", "Built-in"),
                            ("global", "Global"),
                            ("workspace", "Workspace"),
                            ("all", "All"),
                        ] {
                            let active = self.skill_scope_filter == scope_key;
                            if dark_action_btn(ui, None, scope_label, active).clicked() {
                                self.skill_scope_filter = scope_key.to_string();
                            }
                        }
                    });
                });

                ui.add_space(16.0);

                let filter = self.skill_search_buf.trim().to_lowercase();
                let scope_filter = self.skill_scope_filter.as_str();
                let skills: Vec<_> = self
                    .skills_cache
                    .iter()
                    .filter(|s| {
                        let scope_ok = match scope_filter {
                            "workspace" => s.scope.eq_ignore_ascii_case("workspace"),
                            "global" => s.scope.eq_ignore_ascii_case("global"),
                            "builtin" => {
                                s.builtin
                                    || s.scope.eq_ignore_ascii_case("builtin")
                            }
                            _ => true,
                        };
                        let text_ok = filter.is_empty()
                            || s.name.to_lowercase().contains(&filter)
                            || s.description.to_lowercase().contains(&filter)
                            || s.folder.to_lowercase().contains(&filter)
                            || s.triggers
                                .iter()
                                .any(|t| t.to_lowercase().contains(&filter));
                        scope_ok && text_ok
                    })
                    .cloned()
                    .collect();

                // Group skills by folder (matching media_1794090367693_85aa24ac.jpg)
                let mut folder_groups: Vec<(String, Vec<crate::SkillInfo>)> = Vec::new();
                for skill in skills {
                    let folder_title = if skill.folder.trim().is_empty()
                        || skill.folder.eq_ignore_ascii_case("General")
                    {
                        "GENERAL / UNGROUPED".to_string()
                    } else {
                        skill.folder.to_uppercase()
                    };
                    if let Some(group) = folder_groups
                        .iter_mut()
                        .find(|(f, _)| *f == folder_title)
                    {
                        group.1.push(skill);
                    } else {
                        folder_groups.push((folder_title, vec![skill]));
                    }
                }

                let total_avail = ui.available_width();
                let col_w = ((total_avail - 14.0) * 0.5).max(260.0);

                for (folder_name, group_skills) in folder_groups {
                    // Folder Section Header (`[folder] GENERAL / UNGROUPED   43`)
                    ui.horizontal(|ui| {
                        let (ic_f, _) =
                            ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                        paint_icon(ui.painter(), ic_f, Icon::Folder, theme::ACCENT_AMBER);
                        ui.label(
                            RichText::new(&folder_name)
                                .size(11.0)
                                .strong()
                                .color(theme::TEXT_SECONDARY),
                        );
                        egui::Frame::new()
                            .fill(Color32::from_rgb(24, 24, 28))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(Margin::symmetric(6, 1))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(group_skills.len().to_string())
                                        .size(10.0)
                                        .family(FontFamily::Monospace)
                                        .color(theme::TEXT_MUTED),
                                );
                            });
                    });
                    ui.add_space(8.0);

                    for pair in group_skills.chunks(2) {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            for skill in pair {
                                // CRITICAL: Wrap inside `ui.vertical` so the card does NOT inherit `horizontal_top` layout!
                                ui.vertical(|ui| {
                                    ui.set_width(col_w);
                                    egui::Frame::new()
                                        .fill(theme::BG_ACTIVITY)
                                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                        .corner_radius(CornerRadius::same(8))
                                        .inner_margin(Margin::same(14))
                                        .show(ui, |ui| {
                                            ui.set_width(col_w - 28.0);
                                            ui.vertical(|ui| {
                                                // Top Row: Title + Scope Pill + Edit/Delete buttons
                                                ui.horizontal(|ui| {
                                                    let display_title =
                                                        if skill.name.chars().count() > 26 {
                                                            let s: String = skill
                                                                .name
                                                                .chars()
                                                                .take(24)
                                                                .collect();
                                                            format!("{s}...")
                                                        } else {
                                                            skill.name.clone()
                                                        };
                                                    ui.label(
                                                        RichText::new(display_title)
                                                            .size(13.0)
                                                            .strong()
                                                            .color(theme::TEXT_WHITE),
                                                    );
                                                    let badge_col = if skill
                                                        .scope
                                                        .eq_ignore_ascii_case("workspace")
                                                    {
                                                        theme::ACCENT_EMERALD
                                                    } else {
                                                        theme::ACCENT_PURPLE
                                                    };
                                                    egui::Frame::new()
                                                        .fill(Color32::from_rgb(22, 22, 26))
                                                        .corner_radius(CornerRadius::same(4))
                                                        .inner_margin(Margin::symmetric(5, 1))
                                                        .show(ui, |ui| {
                                                            ui.label(
                                                                RichText::new(
                                                                    skill.scope.to_uppercase(),
                                                                )
                                                                .size(9.5)
                                                                .family(FontFamily::Monospace)
                                                                .color(badge_col),
                                                            );
                                                        });

                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.spacing_mut().item_spacing.x = 4.0;
                                                            if dark_action_btn(
                                                                ui, None, "Delete", false,
                                                            )
                                                            .clicked()
                                                            {
                                                                self.pending_delete = Some(
                                                                    crate::PendingDeleteAction::Skill {
                                                                        id: skill.id.clone(),
                                                                        name: skill.name.clone(),
                                                                    },
                                                                );
                                                            }
                                                            if dark_action_btn(
                                                                ui, None, "Edit", false,
                                                            )
                                                            .clicked()
                                                            {
                                                                self.skill_edit_id =
                                                                    skill.id.clone();
                                                                self.skill_name_buf =
                                                                    skill.name.clone();
                                                                self.skill_desc_buf =
                                                                    skill.description.clone();
                                                                self.skill_folder_buf =
                                                                    skill.folder.clone();
                                                                self.skill_triggers_buf =
                                                                    skill.triggers.join(", ");
                                                                self.skill_body_buf =
                                                                    skill.instructions.clone();
                                                                self.show_skill_modal = true;
                                                            }
                                                        },
                                                    );
                                                });

                                                ui.add_space(6.0);

                                                // Wrapped multi-line description
                                                let desc_clean = skill
                                                    .description
                                                    .trim_matches('"')
                                                    .trim();
                                                let desc_short =
                                                    if desc_clean.chars().count() > 135 {
                                                        let s: String =
                                                            desc_clean.chars().take(132).collect();
                                                        format!("{s}...")
                                                    } else if desc_clean.is_empty() {
                                                        "No description provided.".to_string()
                                                    } else {
                                                        desc_clean.to_string()
                                                    };
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(desc_short)
                                                            .size(11.5)
                                                            .color(theme::TEXT_SECONDARY),
                                                    )
                                                    .wrap(),
                                                );

                                                ui.add_space(8.0);

                                                // Truncated path footer so it never overflows the card
                                                let path_short =
                                                    if skill.path.chars().count() > 54 {
                                                        let tail: String = skill
                                                            .path
                                                            .chars()
                                                            .rev()
                                                            .take(51)
                                                            .collect::<Vec<_>>()
                                                            .into_iter()
                                                            .rev()
                                                            .collect();
                                                        format!("...{tail}")
                                                    } else {
                                                        skill.path.clone()
                                                    };
                                                ui.label(
                                                    RichText::new(path_short)
                                                        .size(10.0)
                                                        .family(FontFamily::Monospace)
                                                        .color(theme::TEXT_DIM),
                                                );
                                            });
                                        });
                                });
                            }
                        });
                        ui.add_space(10.0);
                    }
                    ui.add_space(8.0);
                }
            });
    }

    fn install_skill_preset(&mut self, repo: &str, subpath: &str) {
        let ws = self.hm.active_workspace_path();
        if let Some((id, name, desc, folder, triggers, repo_url, body, script_opt)) =
            SkillsManager::curated_preset_bundle(repo, subpath)
        {
            if let Ok(saved) = SkillsManager::save_skill_full_with_id(
                Some(&id),
                &name,
                &desc,
                &body,
                &triggers,
                "global",
                &repo_url,
                &folder,
                Some(&ws),
            ) {
                if let Some((rel, content)) = script_opt {
                    let _ = SkillsManager::write_skill_file(&saved.id, &rel, &content, Some(&ws));
                }
                self.skills_cache = SkillsManager::list_skills_for_workspace(Some(&ws));
                self.status_toast = Some((
                    format!("Installed skill '{}'", saved.name),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn render_settings_mcp_servers(&mut self, ui: &mut Ui) {
        let active_probe = self
            .mcp_probing_target
            .lock()
            .ok()
            .and_then(|g| g.clone());
        if active_probe.is_none() {
            // Ensure cache is fresh after any background probe completes
            self.mcp_cache = McpManager::load();
        }

        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ic_r, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                    paint_icon(ui.painter(), ic_r, Icon::Server, theme::ACCENT_SKY);
                    ui.label(
                        RichText::new("MODEL CONTEXT PROTOCOL (MCP) SERVERS")
                            .size(11.5)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_label = if active_probe.as_deref() == Some("all") {
                            "Probing Enabled..."
                        } else {
                            "Probe All Enabled"
                        };
                        if dark_action_btn(ui, Some(Icon::RefreshCw), btn_label, true).clicked()
                            && active_probe.is_none()
                        {
                            if let Ok(mut g) = self.mcp_probing_target.lock() {
                                *g = Some("all".to_string());
                            }
                            self.status_toast = Some((
                                "Probing all enabled MCP servers...".to_string(),
                                std::time::Instant::now(),
                            ));
                            let target_slot = Arc::clone(&self.mcp_probing_target);
                            let summary_slot = Arc::clone(&self.mcp_last_summary);
                            std::thread::spawn(move || {
                                let results = McpManager::probe_all_enabled();
                                let ok_count = results.iter().filter(|r| r.ok).count();
                                let total_tools: usize =
                                    results.iter().map(|r| r.tools.len()).sum();
                                let all_ok = !results.is_empty() && ok_count == results.len();
                                let summary = if results.is_empty() {
                                    "No MCP servers are currently enabled.".to_string()
                                } else {
                                    format!(
                                        "Probed {} enabled server(s): {} Online · {} tools discovered",
                                        results.len(),
                                        ok_count,
                                        total_tools
                                    )
                                };
                                if let Ok(mut s) = summary_slot.lock() {
                                    *s = Some((all_ok, summary));
                                }
                                if let Ok(mut t) = target_slot.lock() {
                                    *t = None;
                                }
                            });
                        }
                    });
                });

                if let Some(ref target) = active_probe {
                    ui.add_space(10.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(24, 22, 14))
                        .stroke(Stroke::new(1.0, theme::ACCENT_AMBER))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "◌ Probing MCP target '{}' via JSON-RPC 2.0 handshake...",
                                    target
                                ))
                                .size(11.5)
                                .family(FontFamily::Monospace)
                                .color(theme::ACCENT_AMBER),
                            );
                        });
                } else if let Some((ok, summary_txt)) = self
                    .mcp_last_summary
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                {
                    ui.add_space(10.0);
                    let col = if ok {
                        theme::ACCENT_EMERALD
                    } else {
                        theme::ACCENT_AMBER
                    };
                    egui::Frame::new()
                        .fill(Color32::from_rgb(14, 22, 18))
                        .stroke(Stroke::new(1.0, col))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(summary_txt)
                                    .size(11.5)
                                    .family(FontFamily::Monospace)
                                    .color(col),
                            );
                        });
                }

                ui.add_space(14.0);
                let servers = self.mcp_cache.clone();
                let mut changed = false;
                for srv in servers {
                    let is_probing_this =
                        active_probe.as_deref() == Some(srv.name.as_str())
                            || (active_probe.as_deref() == Some("all") && srv.enabled);
                    egui::Frame::new()
                        .fill(theme::BG_ACTIVITY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(14))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut enabled = srv.enabled;
                                if pill_switch(ui, &mut enabled).changed() {
                                    if let Some(slot) =
                                        self.mcp_cache.iter_mut().find(|s| s.name == srv.name)
                                    {
                                        slot.enabled = enabled;
                                        changed = true;
                                    }
                                }
                                ui.label(
                                    RichText::new(&srv.name)
                                        .size(13.5)
                                        .strong()
                                        .color(theme::TEXT_WHITE),
                                );
                                if is_probing_this {
                                    ui.label(
                                        RichText::new("◌ Probing...")
                                            .size(11.0)
                                            .color(theme::ACCENT_AMBER),
                                    );
                                } else if let Some(ok) = srv.last_probe_ok {
                                    let badge_txt = if ok {
                                        format!("● Online ({} tools)", srv.cached_tools.len())
                                    } else {
                                        "● Error".to_string()
                                    };
                                    ui.label(
                                        RichText::new(badge_txt).size(11.0).color(if ok {
                                            theme::ACCENT_EMERALD
                                        } else {
                                            theme::ACCENT_RED
                                        }),
                                    );
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if dark_action_btn(ui, Some(Icon::Trash2), "Remove", false)
                                            .clicked()
                                        {
                                            self.pending_delete = Some(
                                                crate::PendingDeleteAction::McpServer {
                                                    name: srv.name.clone(),
                                                },
                                            );
                                        }
                                        let p_label = if is_probing_this {
                                            "Probing..."
                                        } else {
                                            "Probe"
                                        };
                                        if dark_action_btn(ui, Some(Icon::RefreshCw), p_label, true)
                                            .clicked()
                                            && active_probe.is_none()
                                        {
                                            let srv_clone = srv.clone();
                                            if let Ok(mut g) = self.mcp_probing_target.lock() {
                                                *g = Some(srv.name.clone());
                                            }
                                            self.status_toast = Some((
                                                format!("Probing MCP server '{}'...", srv.name),
                                                std::time::Instant::now(),
                                            ));
                                            let target_slot = Arc::clone(&self.mcp_probing_target);
                                            let summary_slot = Arc::clone(&self.mcp_last_summary);
                                            std::thread::spawn(move || {
                                                let r = McpManager::probe_server(&srv_clone);
                                                let summary = if r.ok {
                                                    format!(
                                                        "✓ '{}' Online ({} ms · {} tools discovered)",
                                                        r.name,
                                                        r.latency_ms,
                                                        r.tools.len()
                                                    )
                                                } else {
                                                    format!(
                                                        "✗ '{}' probe failed: {}",
                                                        r.name,
                                                        r.error.unwrap_or_else(|| {
                                                            "No response".to_string()
                                                        })
                                                    )
                                                };
                                                if let Ok(mut s) = summary_slot.lock() {
                                                    *s = Some((r.ok, summary));
                                                }
                                                if let Ok(mut t) = target_slot.lock() {
                                                    *t = None;
                                                }
                                            });
                                        }
                                    },
                                );
                            });
                            ui.add_space(4.0);
                            let endpoint = if !srv.url.is_empty() {
                                srv.url.clone()
                            } else {
                                format!("{} {}", srv.command, srv.args.join(" "))
                            };
                            ui.label(
                                RichText::new(endpoint)
                                    .size(11.5)
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_SECONDARY),
                            );
                            if !srv.cached_tools.is_empty() {
                                ui.add_space(4.0);
                                let tool_names: Vec<_> =
                                    srv.cached_tools.iter().map(|t| t.name.as_str()).collect();
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "Tools ({}): {}",
                                            tool_names.len(),
                                            tool_names.join(", ")
                                        ))
                                        .size(11.0)
                                        .color(theme::ACCENT_EMERALD),
                                    )
                                    .wrap(),
                                );
                            }
                            if srv.last_probe_ok == Some(false) {
                                if let Some(ref err_msg) = srv.last_error {
                                    if !err_msg.trim().is_empty() {
                                        ui.add_space(4.0);
                                        let short_err: String =
                                            err_msg.chars().take(180).collect();
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(format!("Error: {}", short_err))
                                                    .size(11.0)
                                                    .family(FontFamily::Monospace)
                                                    .color(theme::ACCENT_RED),
                                            )
                                            .wrap(),
                                        );
                                    }
                                }
                            }
                        });
                    ui.add_space(8.0);
                }
                if changed {
                    let _ = McpManager::save(&self.mcp_cache);
                }
            });
    }

    fn render_settings_behavior_context(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("AUTONOMOUS EXECUTION & CONTEXT PARAMETERS")
                        .size(11.5)
                        .strong()
                        .color(theme::TEXT_WHITE),
                );
                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Max Autonomous Iterations per Turn:")
                            .size(12.5)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.settings_draft.max_iterations)
                            .range(1..=2000),
                    );
                });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Subagent Max Turns:")
                            .size(12.5)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.settings_draft.subagent_max_turns)
                            .range(1..=2000),
                    );
                });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Shell Command Timeout (seconds):")
                            .size(12.5)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.settings_draft.command_timeout)
                            .range(5..=3600),
                    );
                });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    pill_switch(ui, &mut self.settings_draft.enable_compaction);
                    ui.label(
                        RichText::new("Enable Automatic Context Compaction on Long Sessions")
                            .size(12.5)
                            .color(theme::TEXT_PRIMARY),
                    );
                });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    pill_switch(ui, &mut self.settings_draft.confirm_commands);
                    ui.label(
                        RichText::new("Supervised Shell Mode (Require confirmation for destructive commands)")
                            .size(12.5)
                            .color(theme::TEXT_PRIMARY),
                    );
                });
            });
    }

    fn render_settings_display_budget_rag(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("SESSION BUDGET CIRCUIT-BREAKER & PERSISTENT MEMORY")
                        .size(11.5)
                        .strong()
                        .color(theme::TEXT_WHITE),
                );
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Max USD Cost per Session (0.0 = Unlimited):")
                            .size(12.5)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.settings_draft.max_cost_per_session)
                            .speed(0.05)
                            .range(0.0..=1000.0)
                            .prefix("$"),
                    );
                });

                ui.add_space(18.0);
                ui.label(
                    RichText::new("CROSS-SESSION PERSISTENT MEMORY (MEMORY.JSON)")
                        .size(11.5)
                        .strong()
                        .color(theme::TEXT_WHITE),
                );
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.add_sized(
                        egui::vec2(180.0, 28.0),
                        TextEdit::singleline(&mut self.mem_key_buf).hint_text("Memory key..."),
                    );
                    ui.add_sized(
                        egui::vec2(ui.available_width() - 110.0, 28.0),
                        TextEdit::singleline(&mut self.mem_val_buf).hint_text("Fact or rule to remember..."),
                    );
                    if white_cta_btn(ui, Some(Icon::Plus), "Save").clicked() {
                        let k = self.mem_key_buf.trim().to_string();
                        let v = self.mem_val_buf.trim().to_string();
                        if !k.is_empty() && !v.is_empty() {
                            let _ = MemoryStore::save_entry("general", &k, &v);
                            self.mem_key_buf.clear();
                            self.mem_val_buf.clear();
                        }
                    }
                });

                ui.add_space(10.0);
                let entries = MemoryStore::load();
                for entry in entries {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&entry.key)
                                .size(12.0)
                                .strong()
                                .family(FontFamily::Monospace)
                                .color(theme::ACCENT_EMERALD),
                        );
                        ui.label(
                            RichText::new(&entry.content)
                                .size(12.0)
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if dark_action_btn(ui, None, "Delete", false).clicked() {
                                let _ = MemoryStore::delete_entry(&entry.id);
                            }
                        });
                    });
                }
            });
    }

    fn render_settings_updates_changelog(&mut self, ui: &mut Ui) {
        let is_checking = self.update_checking.load(Ordering::Relaxed);
        if is_checking {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
        }

        egui::Frame::new()
            .fill(theme::BG_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let btn_reserve_width = 300.0;
                    let text_width = (ui.available_width() - btn_reserve_width).max(200.0);
                    ui.vertical(|ui| {
                        ui.set_max_width(text_width);
                        ui.label(
                            RichText::new(format!(
                                "Unjess v{} — 100% Native Rust GPU Edition",
                                CURRENT_VERSION
                            ))
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT_WHITE),
                        );
                        ui.label(
                            RichText::new(
                                "Compiled natively with eframe/egui (zero HTML, zero JS, zero WebView2). Connected to github.com/Bexx1107/Unjess-Rust.",
                            )
                            .size(12.0)
                            .color(theme::TEXT_SECONDARY),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_label = if is_checking {
                            "◌ Checking GitHub..."
                        } else {
                            "Check for Updates"
                        };
                        if white_cta_btn(ui, Some(Icon::RefreshCw), btn_label).clicked()
                            && !is_checking
                        {
                            let server_url = self.settings_draft.update_server_url.clone();
                            let checking_flag = Arc::clone(&self.update_checking);
                            let result_slot = Arc::clone(&self.update_check_result);
                            checking_flag.store(true, Ordering::Relaxed);
                            self.status_toast = Some((
                                "Checking github.com/Bexx1107/Unjess-Rust for updates...".to_string(),
                                std::time::Instant::now(),
                            ));
                            std::thread::spawn(move || {
                                let res =
                                    unjess_core::updater::check_for_updates_blocking(&server_url);
                                if let Ok(mut slot) = result_slot.lock() {
                                    *slot = Some(res);
                                }
                                checking_flag.store(false, Ordering::Relaxed);
                            });
                        }
                        if dark_action_btn(ui, Some(Icon::Globe), "GitHub Repo", false).clicked() {
                            self.browser_query_buf =
                                "https://github.com/Bexx1107/Unjess-Rust".to_string();
                            self.active_tab = MainTab::Browser;
                        }
                    });
                });

                if let Ok(mut slot) = self.update_download_progress.lock() {
                    if let Some(msg) = slot.take() {
                        self.status_toast = Some((msg, std::time::Instant::now()));
                    }
                }

                let check_res_opt = self
                    .update_check_result
                    .lock()
                    .ok()
                    .and_then(|g| g.clone());
                if let Some(check_res) = check_res_opt {
                    ui.add_space(14.0);
                    let (border_col, fill_col) = match &check_res {
                        Ok(st) if st.update_available => (
                            theme::ACCENT_CYAN,
                            Color32::from_rgba_unmultiplied(6, 182, 212, 18),
                        ),
                        Ok(_) => (
                            Color32::from_rgba_unmultiplied(16, 185, 129, 90),
                            Color32::from_rgba_unmultiplied(16, 185, 129, 14),
                        ),
                        Err(_) => (
                            Color32::from_rgba_unmultiplied(239, 68, 68, 90),
                            Color32::from_rgba_unmultiplied(239, 68, 68, 14),
                        ),
                    };
                    egui::Frame::new()
                        .fill(fill_col)
                        .stroke(Stroke::new(1.0, border_col))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            match check_res {
                                Ok(st) => {
                                    ui.horizontal(|ui| {
                                        if st.update_available {
                                            ui.label(
                                                RichText::new(format!(
                                                    "↑ Update Available: v{} (Installed: v{})",
                                                    st.latest_version, st.current_version
                                                ))
                                                .size(12.5)
                                                .strong()
                                                .color(theme::ACCENT_CYAN),
                                            );
                                        } else {
                                            ui.label(
                                                RichText::new(format!(
                                                    "✓ Up to Date — Unjess v{} matches latest on GitHub (Bexx1107/Unjess-Rust)",
                                                    st.current_version
                                                ))
                                                .size(12.5)
                                                .strong()
                                                .color(theme::ACCENT_EMERALD),
                                            );
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if st.update_available {
                                                    let is_dl = self.update_downloading.load(Ordering::Relaxed);
                                                    if is_dl {
                                                        let _ = dark_action_btn(
                                                            ui,
                                                            Some(Icon::Download),
                                                            "◌ Installing...",
                                                            false,
                                                        );
                                                    } else if white_cta_btn(
                                                        ui,
                                                        Some(Icon::Download),
                                                        "Install Update Now",
                                                    )
                                                    .clicked()
                                                    {
                                                        let dl_url = st.download_url.clone();
                                                        let sha = st.sha256.clone();
                                                        let flag = Arc::clone(&self.update_downloading);
                                                        let toast_slot = Arc::clone(&self.update_download_progress);
                                                        flag.store(true, Ordering::Relaxed);
                                                        self.status_toast = Some((
                                                            "Downloading & installing Unjess update from GitHub...".to_string(),
                                                            std::time::Instant::now(),
                                                        ));
                                                        std::thread::spawn(move || {
                                                            let res = unjess_core::updater::download_and_launch_installer_blocking(&dl_url, &sha);
                                                            flag.store(false, Ordering::Relaxed);
                                                            if let Ok(mut slot) = toast_slot.lock() {
                                                                match res {
                                                                    Ok(msg) => *slot = Some(msg),
                                                                    Err(e) => *slot = Some(format!("Update failed: {}", e)),
                                                                }
                                                            }
                                                        });
                                                    }

                                                    if dark_action_btn(
                                                        ui,
                                                        Some(Icon::Globe),
                                                        "Open in Browser",
                                                        false,
                                                    )
                                                    .clicked()
                                                    {
                                                        let target = if st.download_url.is_empty() {
                                                            "https://github.com/Bexx1107/Unjess-Rust/raw/main/dist/Unjess-Setup.exe"
                                                                .to_string()
                                                        } else {
                                                            st.download_url.clone()
                                                        };
                                                        ui.ctx().open_url(egui::OpenUrl::new_tab(&target));
                                                    }
                                                }
                                                if !st.sha256.is_empty() {
                                                    let short_sha = if st.sha256.len() > 16 {
                                                        format!("{}…", &st.sha256[..16])
                                                    } else {
                                                        st.sha256.clone()
                                                    };
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "SHA-256: {}",
                                                            short_sha
                                                        ))
                                                        .font(FontId::monospace(10.5))
                                                        .color(theme::TEXT_SECONDARY),
                                                    );
                                                }
                                            },
                                        );
                                    });
                                }
                                Err(err_msg) => {
                                    ui.label(
                                        RichText::new(format!("✗ Update check failed: {}", err_msg))
                                            .size(12.0)
                                            .color(theme::ACCENT_RED),
                                    );
                                }
                            }
                        });
                }

                ui.add_space(16.0);
                for entry in builtin_changelog() {
                    egui::Frame::new()
                        .fill(theme::BG_ACTIVITY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(14))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("v{} — {}", entry.version, entry.title))
                                        .size(13.5)
                                        .strong()
                                        .color(theme::TEXT_WHITE),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(&entry.date)
                                                .size(11.0)
                                                .family(FontFamily::Monospace)
                                                .color(theme::TEXT_MUTED),
                                        );
                                    },
                                );
                            });
                            ui.add_space(6.0);
                            for h in &entry.changes {
                                ui.label(
                                    RichText::new(format!("• {h}"))
                                        .size(12.0)
                                        .color(theme::TEXT_SECONDARY),
                                );
                            }
                        });
                    ui.add_space(10.0);
                }
            });
    }

    /// Renders floating modal dialogs (Subagent Editor, Workflow Editor, Schedule Editor, Skill Editor, Terminal Drawer, and Delete Confirmation).
    pub fn render_modals(&mut self, ctx: &egui::Context) {
        // 1. Subagent Modal
        if self.show_subagent_modal {
            egui::Window::new("Subagent Studio")
                .collapsible(false)
                .resizable(false)
                .fixed_size([480.0, 390.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.label("Subagent Name");
                    ui.text_edit_singleline(&mut self.subagent_name_buf);
                    ui.add_space(6.0);
                    ui.label("Role / Specialty");
                    ui.text_edit_singleline(&mut self.subagent_role_buf);
                    ui.add_space(6.0);
                    ui.label("Model ('inherit' or specific model ID)");
                    ui.text_edit_singleline(&mut self.subagent_model_buf);
                    ui.add_space(6.0);
                    ui.label("System Prompt");
                    ScrollArea::vertical()
                        .id_salt("subagent_modal_prompt_scroll")
                        .max_height(110.0)
                        .show(ui, |ui| {
                            ui.add_sized(
                                egui::vec2(440.0, 100.0),
                                TextEdit::multiline(&mut self.subagent_prompt_buf),
                            );
                        });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if white_cta_btn(ui, Some(Icon::Check), "Save Subagent").clicked() {
                            let id_opt = if self.subagent_edit_id.is_empty() {
                                None
                            } else {
                                Some(self.subagent_edit_id.as_str())
                            };
                            self.hm.upsert_subagent(
                                id_opt,
                                &self.subagent_name_buf,
                                &self.subagent_role_buf,
                                &self.subagent_prompt_buf,
                                &self.subagent_model_buf,
                                Vec::new(),
                            );
                            self.show_subagent_modal = false;
                        }
                        if dark_action_btn(ui, Some(Icon::X), "Cancel", false).clicked() {
                            self.show_subagent_modal = false;
                        }
                    });
                });
        }

        // 2. Workflow Modal
        if self.show_workflow_modal {
            egui::Window::new("Workflow Editor")
                .collapsible(false)
                .resizable(false)
                .fixed_size([480.0, 350.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.label("Workflow Title");
                    ui.text_edit_singleline(&mut self.wf_name_buf);
                    ui.add_space(6.0);
                    ui.label("Description");
                    ui.text_edit_singleline(&mut self.wf_desc_buf);
                    ui.add_space(6.0);
                    ui.label("Steps (one per line)");
                    ScrollArea::vertical()
                        .id_salt("workflow_modal_steps_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.add_sized(
                                egui::vec2(440.0, 110.0),
                                TextEdit::multiline(&mut self.wf_steps_buf),
                            );
                        });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if white_cta_btn(ui, Some(Icon::Check), "Save Workflow").clicked() {
                            let steps: Vec<String> = self
                                .wf_steps_buf
                                .lines()
                                .map(|l| l.trim().to_string())
                                .filter(|l| !l.is_empty())
                                .collect();
                            self.hm.save_workflow(WorkflowDef {
                                id: self.wf_edit_id.clone(),
                                name: self.wf_name_buf.clone(),
                                title: self.wf_name_buf.clone(),
                                description: self.wf_desc_buf.clone(),
                                category: "engineering".to_string(),
                                prompt: steps.join("\n"),
                                tools: Vec::new(),
                                steps,
                            });
                            self.show_workflow_modal = false;
                        }
                        if dark_action_btn(ui, Some(Icon::X), "Cancel", false).clicked() {
                            self.show_workflow_modal = false;
                        }
                    });
                });
        }

        // 3. Schedule Modal
        if self.show_schedule_modal {
            egui::Window::new("New Scheduled Task")
                .collapsible(false)
                .resizable(false)
                .fixed_size([460.0, 330.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_max_width(440.0);
                    ui.label("Schedule Name");
                    ui.text_edit_singleline(&mut self.sched_name_buf);
                    ui.add_space(6.0);
                    ui.label("Interval or Cron (e.g. 'every 30m', 'every 2h', '0 9 * * *')");
                    ui.text_edit_singleline(&mut self.sched_cron_buf);
                    ui.add_space(6.0);
                    ui.label("Autonomous Prompt or Command");
                    ScrollArea::vertical()
                        .id_salt("schedule_modal_prompt_scroll")
                        .max_height(100.0)
                        .show(ui, |ui| {
                            ui.add_sized(
                                egui::vec2(420.0, 90.0),
                                TextEdit::multiline(&mut self.sched_prompt_buf),
                            );
                        });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if white_cta_btn(ui, Some(Icon::Check), "Create Schedule").clicked() {
                            let mut task = ScheduledTask {
                                id: format!("sched-{}", self.schedules_cache.len() + 1),
                                name: self.sched_name_buf.clone(),
                                title: self.sched_name_buf.clone(),
                                cron_or_interval: self.sched_cron_buf.clone(),
                                cron_expression: self.sched_cron_buf.clone(),
                                action: self.sched_action_buf.clone(),
                                prompt: self.sched_prompt_buf.clone(),
                                workspace: self
                                    .hm
                                    .active_workspace_path()
                                    .to_string_lossy()
                                    .to_string(),
                                notify_on: "always".to_string(),
                                enabled: true,
                                last_run: None,
                                last_run_at: None,
                                next_run_at: None,
                                last_status: None,
                                last_output: None,
                            };
                            task.normalize();
                            self.schedules_cache.push(task);
                            let _ = SchedulerManager::save(&self.schedules_cache);
                            self.show_schedule_modal = false;
                        }
                        if dark_action_btn(ui, Some(Icon::X), "Cancel", false).clicked() {
                            self.show_schedule_modal = false;
                        }
                    });
                });
        }

        // 4. Skill Modal (bounded height with scrollable Markdown Instructions so it never escapes the screen!)
        if self.show_skill_modal {
            egui::Window::new("Skill Editor (SKILL.md)")
                .collapsible(false)
                .resizable(false)
                .fixed_size([560.0, 490.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_width(540.0);
                    ui.set_max_width(540.0);
                    ui.label("Skill Name");
                    ui.add_sized(
                        egui::vec2(536.0, 26.0),
                        TextEdit::singleline(&mut self.skill_name_buf),
                    );
                    ui.add_space(6.0);
                    ui.label("Description");
                    ui.add_sized(
                        egui::vec2(536.0, 26.0),
                        TextEdit::singleline(&mut self.skill_desc_buf),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(260.0);
                            ui.label("Category Folder");
                            ui.add_sized(
                                egui::vec2(260.0, 26.0),
                                TextEdit::singleline(&mut self.skill_folder_buf),
                            );
                        });
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.set_width(260.0);
                            ui.label("Triggers (comma-separated)");
                            ui.add_sized(
                                egui::vec2(260.0, 26.0),
                                TextEdit::singleline(&mut self.skill_triggers_buf),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    ui.label("Markdown Instructions (Scrollable)");
                    egui::Frame::new()
                        .fill(Color32::from_rgb(8, 8, 10))
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::same(6))
                        .show(ui, |ui| {
                            ui.set_width(524.0);
                            ui.set_max_width(524.0);
                            ScrollArea::vertical()
                                .id_salt("skill_modal_instructions_scroll")
                                .max_height(215.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.add_sized(
                                        egui::vec2(510.0, 210.0),
                                        TextEdit::multiline(&mut self.skill_body_buf)
                                            .code_editor()
                                            .frame(false)
                                            .desired_width(510.0),
                                    );
                                });
                        });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if white_cta_btn(ui, Some(Icon::Check), "Save Skill").clicked() {
                            let ws = self.hm.active_workspace_path();
                            let triggers: Vec<String> = self
                                .skill_triggers_buf
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            let id_opt = if self.skill_edit_id.is_empty() {
                                None
                            } else {
                                Some(self.skill_edit_id.as_str())
                            };
                            if SkillsManager::save_skill_full_with_id(
                                id_opt,
                                &self.skill_name_buf,
                                &self.skill_desc_buf,
                                &self.skill_body_buf,
                                &triggers,
                                "global",
                                "local",
                                &self.skill_folder_buf,
                                Some(&ws),
                            )
                            .is_ok()
                            {
                                self.skills_cache =
                                    SkillsManager::list_skills_for_workspace(Some(&ws));
                                self.show_skill_modal = false;
                            }
                        }
                        if dark_action_btn(ui, Some(Icon::X), "Cancel", false).clicked() {
                            self.show_skill_modal = false;
                        }
                    });
                });
        }

        // 5. Sandboxed Terminal Modal (fixed/bounded width so it never stretches across the screen)
        if self.show_terminal_drawer {
            egui::Window::new("Sandboxed Workspace Terminal")
                .collapsible(true)
                .resizable(false)
                .fixed_size([720.0, 360.0])
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    ui.set_max_width(700.0);
                    ui.horizontal(|ui| {
                        let resp = ui.add_sized(
                            egui::vec2(500.0, 28.0),
                            TextEdit::singleline(&mut self.terminal_cmd_buf)
                                .font(FontId::new(12.0, FontFamily::Monospace))
                                .hint_text("Enter PowerShell / cargo / git command..."),
                        );
                        if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            || white_cta_btn(ui, Some(Icon::Play), "Run").clicked()
                        {
                            let cmd = self.terminal_cmd_buf.trim().to_string();
                            if !cmd.is_empty() {
                                if let Ok(ws) = Workspace::new(self.hm.active_workspace_path()) {
                                    let te = ToolEngine::new(ws);
                                    let out = te
                                        .run_command(&cmd, 60)
                                        .unwrap_or_else(|e| e.to_string());
                                    self.terminal_output_buf =
                                        format!("$ {cmd}\n{out}\n\n{}", self.terminal_output_buf);
                                }
                                self.terminal_cmd_buf.clear();
                            }
                        }
                        if dark_action_btn(ui, Some(Icon::RotateCcw), "Clear", false).clicked() {
                            self.terminal_output_buf.clear();
                        }
                        if dark_action_btn(ui, Some(Icon::X), "Close", false).clicked() {
                            self.show_terminal_drawer = false;
                        }
                    });
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(8, 8, 10))
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(680.0);
                            ui.set_max_width(680.0);
                            ScrollArea::vertical()
                                .id_salt("terminal_drawer_scroll")
                                .max_height(265.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(664.0);
                                    ui.set_max_width(664.0);
                                    let out_text = if self.terminal_output_buf.trim().is_empty() {
                                        "Ready. Commands run inside the active workspace sandbox."
                                    } else {
                                        self.terminal_output_buf.as_str()
                                    };
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(out_text)
                                                .size(11.5)
                                                .family(FontFamily::Monospace)
                                                .color(if self.terminal_output_buf.trim().is_empty()
                                                {
                                                    theme::TEXT_DIM
                                                } else {
                                                    theme::TEXT_PRIMARY
                                                }),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                });
        }

        // 6. Confirmation Modal for Delete / Remove Actions
        if let Some(pending) = self.pending_delete.clone() {
            let (kind_label, item_title) = match &pending {
                crate::PendingDeleteAction::Chat { title, .. } => ("chat conversation", title.clone()),
                crate::PendingDeleteAction::Workspace { name, .. } => ("workspace folder", name.clone()),
                crate::PendingDeleteAction::Skill { name, .. } => ("skill", name.clone()),
                crate::PendingDeleteAction::Subagent { name, .. } => ("subagent", name.clone()),
                crate::PendingDeleteAction::Workflow { name, .. } => ("workflow", name.clone()),
                crate::PendingDeleteAction::Schedule { name, .. } => ("scheduled task", name.clone()),
                crate::PendingDeleteAction::McpServer { name } => ("MCP server", name.clone()),
            };

            egui::Window::new("Confirm Deletion")
                .collapsible(false)
                .resizable(false)
                .fixed_size([400.0, 155.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.set_max_width(380.0);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let (ic_r, _) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
                        paint_icon(ui.painter(), ic_r, Icon::Trash2, theme::ACCENT_RED);
                        ui.label(
                            RichText::new(format!("Delete {kind_label}?"))
                                .size(14.0)
                                .strong()
                                .color(theme::TEXT_WHITE),
                        );
                    });
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "Are you sure you want to remove '{item_title}'? This action cannot be undone."
                            ))
                            .size(12.5)
                            .color(theme::TEXT_SECONDARY),
                        )
                        .wrap(),
                    );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        // Red Confirm Delete button
                        let (del_rect, del_resp) =
                            ui.allocate_exact_size(egui::vec2(130.0, 30.0), Sense::click());
                        if del_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        ui.painter().rect_filled(
                            del_rect,
                            CornerRadius::same(6),
                            if del_resp.hovered() {
                                Color32::from_rgb(239, 68, 68)
                            } else {
                                Color32::from_rgb(200, 38, 38)
                            },
                        );
                        paint_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                egui::pos2(del_rect.left() + 16.0, del_rect.center().y),
                                egui::vec2(12.0, 12.0),
                            ),
                            Icon::Trash2,
                            Color32::WHITE,
                        );
                        ui.painter().text(
                            egui::pos2(del_rect.left() + 28.0, del_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Confirm Delete",
                            egui::FontId::new(12.0, FontFamily::Proportional),
                            Color32::WHITE,
                        );

                        if del_resp.clicked() {
                            match pending {
                                crate::PendingDeleteAction::Chat { id, .. } => {
                                    self.hm.delete_conversation(&id);
                                }
                                crate::PendingDeleteAction::Workspace { id, .. } => {
                                    self.hm.remove_workspace(&id);
                                    self.refresh_workspace_cache();
                                }
                                crate::PendingDeleteAction::Skill { id, .. } => {
                                    let ws = self.hm.active_workspace_path();
                                    let _ = SkillsManager::delete_skill_for_workspace(
                                        &id,
                                        Some(&ws),
                                    );
                                    self.skills_cache =
                                        SkillsManager::list_skills_for_workspace(Some(&ws));
                                }
                                crate::PendingDeleteAction::Subagent { id, .. } => {
                                    self.hm.delete_subagent(&id);
                                }
                                crate::PendingDeleteAction::Workflow { id, .. } => {
                                    self.hm.delete_workflow(&id);
                                }
                                crate::PendingDeleteAction::Schedule { id, .. } => {
                                    self.schedules_cache.retain(|s| s.id != id);
                                    let _ = SchedulerManager::save(&self.schedules_cache);
                                }
                                crate::PendingDeleteAction::McpServer { name } => {
                                    self.mcp_cache.retain(|s| s.name != name);
                                    let _ = McpManager::save(&self.mcp_cache);
                                }
                            }
                            self.pending_delete = None;
                        }

                        ui.add_space(6.0);
                        if dark_action_btn(ui, Some(Icon::X), "Cancel", false).clicked() {
                            self.pending_delete = None;
                        }
                    });
                });
        }
    }
}
