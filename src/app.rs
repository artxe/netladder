use std::{cmp::Ordering, collections::HashMap, fs, time::Duration};

use eframe::egui::{
    self, Align, Color32, FontData, FontDefinitions, FontFamily, Frame, Layout, RichText,
    TextureHandle,
};

use crate::engine::{self, EngineHandle, ProcessTraffic, Shared, SharedState};

pub struct NetLadderApp {
    shared: Shared,
    _engine: EngineHandle,
    process_icons: HashMap<String, Option<TextureHandle>>,
    process_sort: Option<ProcessSort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Process,
    Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessSort {
    column: SortColumn,
    direction: SortDirection,
}

impl ProcessSort {
    fn select(current: &mut Option<Self>, column: SortColumn) {
        *current = Some(match *current {
            Some(sort) if sort.column == column => Self {
                column,
                direction: match sort.direction {
                    SortDirection::Ascending => SortDirection::Descending,
                    SortDirection::Descending => SortDirection::Ascending,
                },
            },
            _ => Self {
                column,
                direction: match column {
                    SortColumn::Process => SortDirection::Ascending,
                    SortColumn::Usage => SortDirection::Descending,
                },
            },
        });
    }
}

impl NetLadderApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        install_korean_font(&context.egui_ctx);
        context.egui_ctx.set_visuals(egui::Visuals::dark());
        let shared = std::sync::Arc::new(std::sync::Mutex::new(SharedState::default()));
        let engine = engine::start(shared.clone());
        Self {
            shared,
            _engine: engine,
            process_icons: HashMap::new(),
            process_sort: None,
        }
    }

    fn header(&self, ui: &mut egui::Ui) {
        let (running, error, detected_capacity) = {
            let state = self.shared.lock().unwrap();
            (
                state.running,
                state.error.clone(),
                state.detected_capacity_bits_per_second,
            )
        };
        ui.horizontal(|ui| {
            ui.heading("NetLadder");
            ui.label(
                RichText::new(if running {
                    "● Running"
                } else {
                    "● Starting"
                })
                .color(if running {
                    Color32::from_rgb(80, 210, 130)
                } else {
                    Color32::YELLOW
                }),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let capacity = detected_capacity
                    .map(|bits| format!("Observed: {:.1} Mbps", bits as f64 / 1_000_000.0))
                    .unwrap_or_else(|| "Waiting for traffic…".to_owned());
                ui.label(RichText::new(capacity).color(Color32::LIGHT_BLUE));
            });
        });
        ui.label("Set an independent download speed limit for each process.");
        if let Some(error) = error {
            ui.add_space(6.0);
            Frame::new()
                .fill(Color32::from_rgb(80, 30, 30))
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, error);
                    ui.label("Check administrator access and the WinDivert packaged files.");
                });
        }
    }

    fn process_list(&mut self, ui: &mut egui::Ui) {
        let (rows, limits) = self.visible_rows();
        if rows.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(90.0);
                ui.spinner();
                ui.add_space(12.0);
                ui.label("Waiting for a process to use the network…");
                ui.small("Start a browser or download and it will appear automatically.");
            });
            return;
        }
        self.ensure_process_icons(ui.ctx(), &rows);
        let mut changes = Vec::new();
        for traffic in &rows {
            let icon = self
                .process_icons
                .get(&traffic.name)
                .and_then(Option::as_ref)
                .map(TextureHandle::id);
            let change = ui
                .push_id(&traffic.name, |ui| {
                    draw_process_row(ui, traffic, icon, limits.get(&traffic.name).copied())
                })
                .inner;
            if let Some(limit) = change {
                changes.push((traffic.name.clone(), limit));
            }
            ui.add_space(3.0);
        }
        if !changes.is_empty() {
            let mut state = self.shared.lock().unwrap();
            for (name, limit) in changes {
                if let Some(bits_per_second) = limit {
                    state.limits_bits_per_second.insert(name, bits_per_second);
                } else {
                    state.limits_bits_per_second.remove(&name);
                }
            }
        }
    }

    fn visible_rows(&self) -> (Vec<ProcessTraffic>, HashMap<String, u64>) {
        let (mut rows, limits) = {
            let state = self.shared.lock().unwrap();
            let now = std::time::Instant::now();
            let rows: Vec<_> = state
                .order
                .iter()
                .filter_map(|name| state.traffic.get(name))
                .filter(|traffic| now.duration_since(traffic.last_seen) < Duration::from_secs(30))
                .cloned()
                .collect();
            (rows, state.limits_bits_per_second.clone())
        };
        sort_process_rows(&mut rows, &limits, self.process_sort);
        (rows, limits)
    }

    fn ensure_process_icons(&mut self, context: &egui::Context, rows: &[ProcessTraffic]) {
        for traffic in rows {
            if self.process_icons.contains_key(&traffic.name) {
                continue;
            }
            let icon = traffic
                .executable_path
                .as_deref()
                .and_then(load_process_icon)
                .map(|image| {
                    context.load_texture(
                        format!("process-icon:{}", traffic.name),
                        image,
                        egui::TextureOptions::LINEAR,
                    )
                });
            self.process_icons.insert(traffic.name.clone(), icon);
        }
    }
}

impl eframe::App for NetLadderApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Frame::central_panel(root.style()).show(root, |ui| {
            self.header(ui);
            ui.add_space(18.0);
            draw_process_header(ui, &mut self.process_sort);
            ui.separator();
            let list_height = (ui.available_height() - 34.0).max(120.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(list_height)
                .show(ui, |ui| self.process_list(ui));
            ui.add_space(10.0);
            ui.separator();
            ui.small("Enable a limit and set its Mbps value. Disabled processes are unrestricted.");
        });
        root.ctx().request_repaint_after(Duration::from_millis(250));
    }
}

fn draw_process_header(ui: &mut egui::Ui, sort: &mut Option<ProcessSort>) {
    ui.horizontal(|ui| {
        ui.add_sized([178.0, 20.0], egui::Label::new("Download limit"));
        if draw_sort_header(ui, [260.0, 20.0], "Process", SortColumn::Process, *sort)
            .on_hover_text("Sort by process name")
            .clicked()
        {
            ProcessSort::select(sort, SortColumn::Process);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if draw_sort_header(ui, [120.0, 20.0], "Current usage", SortColumn::Usage, *sort)
                .on_hover_text("Sort by current usage")
                .clicked()
            {
                ProcessSort::select(sort, SortColumn::Usage);
            }
        });
    });
}

fn draw_sort_header(
    ui: &mut egui::Ui,
    size: [f32; 2],
    label: &str,
    column: SortColumn,
    sort: Option<ProcessSort>,
) -> egui::Response {
    let active_sort = sort.filter(|sort| sort.column == column);
    let label = match active_sort.map(|sort| sort.direction) {
        Some(SortDirection::Ascending) => format!("{label} ▲"),
        Some(SortDirection::Descending) => format!("{label} ▼"),
        None => label.to_owned(),
    };
    ui.add_sized(
        size,
        egui::Button::new(label)
            .selected(active_sort.is_some())
            .frame(active_sort.is_some()),
    )
}

fn sort_process_rows(
    rows: &mut [ProcessTraffic],
    limits: &HashMap<String, u64>,
    sort: Option<ProcessSort>,
) {
    let Some(sort) = sort else {
        return;
    };

    rows.sort_by(|left, right| {
        match (
            limits.contains_key(&left.name),
            limits.contains_key(&right.name),
        ) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => compare_process_rows(left, right, sort),
        }
    });
}

fn compare_process_rows(
    left: &ProcessTraffic,
    right: &ProcessTraffic,
    sort: ProcessSort,
) -> Ordering {
    match sort.column {
        SortColumn::Process => {
            apply_sort_direction(compare_process_names(left, right), sort.direction)
        }
        SortColumn::Usage => apply_sort_direction(
            left.bits_per_second.total_cmp(&right.bits_per_second),
            sort.direction,
        )
        .then_with(|| compare_process_names(left, right)),
    }
}

fn compare_process_names(left: &ProcessTraffic, right: &ProcessTraffic) -> Ordering {
    left.name
        .to_lowercase()
        .cmp(&right.name.to_lowercase())
        .then_with(|| left.name.cmp(&right.name))
}

fn apply_sort_direction(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn draw_process_row(
    ui: &mut egui::Ui,
    traffic: &ProcessTraffic,
    icon: Option<egui::TextureId>,
    limit: Option<u64>,
) -> Option<Option<u64>> {
    const ROW_CONTENT_HEIGHT: f32 = 34.0;

    let mut enabled = limit.is_some();
    let mut megabits = limit.unwrap_or(10_000_000) as f64 / 1_000_000.0;
    let mut changed = false;
    process_row_frame(ui).show(ui, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ROW_CONTENT_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                changed |= ui
                    .add_sized([62.0, 24.0], egui::Checkbox::new(&mut enabled, "Limit"))
                    .changed();
                changed |= ui
                    .add_enabled(
                        enabled,
                        egui::DragValue::new(&mut megabits)
                            .range(0.1..=100_000.0)
                            .speed(0.5)
                            .suffix(" Mbps")
                            .max_decimals(1),
                    )
                    .on_hover_text("Click to type or drag to adjust")
                    .changed();
                ui.add_space(8.0);
                draw_process_body(ui, traffic, icon);
            },
        );
    });
    changed.then(|| enabled.then(|| (megabits.clamp(0.1, 100_000.0) * 1_000_000.0).round() as u64))
}

fn process_row_frame(ui: &egui::Ui) -> Frame {
    Frame::new()
        .inner_margin(egui::Margin::symmetric(6, 5))
        .corner_radius(5)
        .fill(ui.visuals().widgets.inactive.bg_fill)
        .stroke(egui::Stroke::new(1.0, Color32::from_gray(82)))
}

fn draw_process_body(ui: &mut egui::Ui, traffic: &ProcessTraffic, icon: Option<egui::TextureId>) {
    if let Some(texture) = icon {
        ui.add(
            egui::Image::new((texture, egui::vec2(32.0, 32.0)))
                .fit_to_exact_size(egui::vec2(32.0, 32.0)),
        );
    } else {
        ui.add_sized([32.0, 32.0], egui::Label::new("◻"));
    }
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_width(220.0);
        ui.add(egui::Label::new(RichText::new(&traffic.name).strong()).truncate());
        let pids = traffic
            .pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "PID {pids}  ·  Total {}",
                    format_bytes(traffic.total_bytes)
                ))
                .small(),
            )
            .truncate(),
        );
    });
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 34.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.add_sized(
                [120.0, 28.0],
                egui::Label::new(
                    RichText::new(format_rate(traffic.bits_per_second))
                        .monospace()
                        .size(15.0),
                ),
            );
        },
    );
}

#[cfg(windows)]
fn load_process_icon(path: &str) -> Option<egui::ColorImage> {
    let image = windows_icons::get_icon_by_path(path).ok()?;
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        image.as_raw(),
    ))
}

#[cfg(not(windows))]
fn load_process_icon(_path: &str) -> Option<egui::ColorImage> {
    None
}

fn format_rate(bits_per_second: f64) -> String {
    if bits_per_second >= 1_000_000.0 {
        format!("{:.1} Mbps", bits_per_second / 1_000_000.0)
    } else if bits_per_second >= 1_000.0 {
        format!("{:.0} Kbps", bits_per_second / 1_000.0)
    } else {
        format!("{bits_per_second:.0} bps")
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    }
}

fn install_korean_font(context: &egui::Context) {
    let Ok(bytes) = fs::read(r"C:\Windows\Fonts\malgun.ttf") else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("malgun".into(), FontData::from_owned(bytes).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "malgun".into());
    }
    context.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Instant};

    use super::{ProcessSort, SortColumn, SortDirection, format_rate, sort_process_rows};
    use crate::engine::ProcessTraffic;

    fn traffic(name: &str, bits_per_second: f64) -> ProcessTraffic {
        ProcessTraffic {
            name: name.to_owned(),
            executable_path: None,
            pids: Vec::new(),
            bits_per_second,
            total_bytes: 0,
            last_seen: Instant::now(),
        }
    }

    fn names(rows: &[ProcessTraffic]) -> Vec<&str> {
        rows.iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn formats_network_rates() {
        assert_eq!(format_rate(25_400_000.0), "25.4 Mbps");
        assert_eq!(format_rate(850_000.0), "850 Kbps");
    }

    #[test]
    fn selecting_headers_uses_natural_defaults_and_toggles_direction() {
        let mut sort = None;

        ProcessSort::select(&mut sort, SortColumn::Process);
        assert_eq!(
            sort,
            Some(ProcessSort {
                column: SortColumn::Process,
                direction: SortDirection::Ascending,
            })
        );

        ProcessSort::select(&mut sort, SortColumn::Process);
        assert_eq!(
            sort,
            Some(ProcessSort {
                column: SortColumn::Process,
                direction: SortDirection::Descending,
            })
        );

        ProcessSort::select(&mut sort, SortColumn::Usage);
        assert_eq!(
            sort,
            Some(ProcessSort {
                column: SortColumn::Usage,
                direction: SortDirection::Descending,
            })
        );
    }

    #[test]
    fn sorts_names_inside_separate_limited_and_unlimited_groups() {
        let mut rows = vec![
            traffic("Zulu.exe", 1.0),
            traffic("bravo.exe", 2.0),
            traffic("Echo.exe", 3.0),
            traffic("alpha.exe", 4.0),
        ];
        let limits = HashMap::from([("Zulu.exe".to_owned(), 1), ("Echo.exe".to_owned(), 1)]);

        sort_process_rows(
            &mut rows,
            &limits,
            Some(ProcessSort {
                column: SortColumn::Process,
                direction: SortDirection::Ascending,
            }),
        );
        assert_eq!(
            names(&rows),
            ["Echo.exe", "Zulu.exe", "alpha.exe", "bravo.exe"]
        );

        sort_process_rows(
            &mut rows,
            &limits,
            Some(ProcessSort {
                column: SortColumn::Process,
                direction: SortDirection::Descending,
            }),
        );
        assert_eq!(
            names(&rows),
            ["Zulu.exe", "Echo.exe", "bravo.exe", "alpha.exe"]
        );
    }

    #[test]
    fn sorts_usage_inside_separate_limited_and_unlimited_groups() {
        let mut rows = vec![
            traffic("limited-slow.exe", 10.0),
            traffic("unlimited-fast.exe", 400.0),
            traffic("limited-fast.exe", 200.0),
            traffic("unlimited-slow.exe", 20.0),
        ];
        let limits = HashMap::from([
            ("limited-slow.exe".to_owned(), 1),
            ("limited-fast.exe".to_owned(), 1),
        ]);

        sort_process_rows(
            &mut rows,
            &limits,
            Some(ProcessSort {
                column: SortColumn::Usage,
                direction: SortDirection::Descending,
            }),
        );
        assert_eq!(
            names(&rows),
            [
                "limited-fast.exe",
                "limited-slow.exe",
                "unlimited-fast.exe",
                "unlimited-slow.exe",
            ]
        );

        sort_process_rows(
            &mut rows,
            &limits,
            Some(ProcessSort {
                column: SortColumn::Usage,
                direction: SortDirection::Ascending,
            }),
        );
        assert_eq!(
            names(&rows),
            [
                "limited-slow.exe",
                "limited-fast.exe",
                "unlimited-slow.exe",
                "unlimited-fast.exe",
            ]
        );
    }

    #[test]
    #[cfg(windows)]
    fn executable_contains_extractable_icon() {
        let executable = std::env::current_exe().unwrap();
        let icon = windows_icons::get_icon_by_path(executable).unwrap();
        assert!(icon.width() > 0);
        assert!(icon.height() > 0);
    }
}
