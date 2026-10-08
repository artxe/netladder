use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Layout,
    Margin, Rect, RichText, Sense, Stroke, TextureHandle, ThemePreference, pos2, vec2,
};

use crate::engine::{self, EngineHandle, IDLE_TIMEOUT, ProcessTraffic, Shared, SharedState};

const DEFAULT_LIMIT_BITS_PER_SECOND: u64 = 10_000_000;
const MIN_LIMIT_MBPS: f64 = 0.1;
const MAX_LIMIT_MBPS: f64 = 100_000.0;
/// Rows keep their order for this long so a usage sort does not reshuffle the
/// table under the pointer every metrics tick.
const SORT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const REPAINT_INTERVAL: Duration = Duration::from_millis(250);

const ACCENT: Color32 = Color32::from_rgb(64, 156, 255);
const NEAR_LIMIT: Color32 = Color32::from_rgb(255, 160, 60);
const RUNNING_COLOR: Color32 = Color32::from_rgb(80, 210, 130);
const STARTING_COLOR: Color32 = Color32::from_rgb(240, 200, 60);
const STOPPED_COLOR: Color32 = Color32::from_rgb(240, 90, 90);

// Column geometry shared by the table header and every row so they line up.
const CHECKBOX_WIDTH: f32 = 24.0;
const LIMIT_INPUT_WIDTH: f32 = 104.0;
const ICON_SIZE: f32 = 32.0;
const NAME_WIDTH: f32 = 220.0;
const RATE_WIDTH: f32 = 110.0;
const USAGE_BAR_WIDTH: f32 = 96.0;
const ROW_HEIGHT: f32 = 36.0;
const ROW_MARGIN_X: i8 = 8;
const ROW_MARGIN_Y: i8 = 5;
const ROW_SPACING: f32 = 4.0;

pub struct NetLadderApp {
    shared: Shared,
    _engine: EngineHandle,
    process_icons: HashMap<String, Option<TextureHandle>>,
    process_sort: Option<ProcessSort>,
    row_order: Option<RowOrder>,
    remembered_limits: HashMap<String, u64>,
    filter: String,
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

/// The row order that was last computed, reused between sort refreshes.
struct RowOrder {
    positions: HashMap<String, usize>,
    sort: Option<ProcessSort>,
    computed_at: Instant,
}

/// Everything one frame needs from the engine, read under a single lock.
struct Snapshot {
    running: bool,
    error: Option<String>,
    detected_capacity: Option<u64>,
    rows: Vec<ProcessTraffic>,
    limits: HashMap<String, u64>,
}

impl NetLadderApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        install_korean_font(&context.egui_ctx);
        context.egui_ctx.set_theme(ThemePreference::Dark);
        let shared = std::sync::Arc::new(std::sync::Mutex::new(SharedState::default()));
        #[cfg(debug_assertions)]
        seed_preview_rows(&shared);
        let engine = engine::start(shared.clone());
        Self {
            shared,
            _engine: engine,
            process_icons: HashMap::new(),
            process_sort: None,
            row_order: None,
            remembered_limits: HashMap::new(),
            filter: String::new(),
        }
    }

    fn snapshot(&mut self) -> Snapshot {
        let (running, error, detected_capacity, mut rows, limits) = {
            let state = self.shared.lock().unwrap();
            let now = Instant::now();
            let rows: Vec<_> = state
                .order
                .iter()
                .filter_map(|name| state.traffic.get(name))
                .filter(|traffic| {
                    now.duration_since(traffic.last_seen) < IDLE_TIMEOUT
                        || state.limits_bits_per_second.contains_key(&traffic.name)
                })
                .cloned()
                .collect();
            (
                state.running,
                state.error.clone(),
                state.detected_capacity_bits_per_second,
                rows,
                state.limits_bits_per_second.clone(),
            )
        };
        self.arrange_rows(&mut rows, &limits);
        Snapshot {
            running,
            error,
            detected_capacity,
            rows,
            limits,
        }
    }

    fn arrange_rows(&mut self, rows: &mut [ProcessTraffic], limits: &HashMap<String, u64>) {
        let now = Instant::now();
        let reusable = self.row_order.as_ref().filter(|order| {
            order.sort == self.process_sort
                && now.duration_since(order.computed_at) < SORT_REFRESH_INTERVAL
                && rows
                    .iter()
                    .all(|row| order.positions.contains_key(&row.name))
        });
        if let Some(order) = reusable {
            rows.sort_by_key(|row| order.positions[&row.name]);
            return;
        }

        sort_process_rows(rows, limits, self.process_sort);
        self.row_order = Some(RowOrder {
            positions: rows
                .iter()
                .enumerate()
                .map(|(index, row)| (row.name.clone(), index))
                .collect(),
            sort: self.process_sort,
            computed_at: now,
        });
    }

    fn header(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.horizontal(|ui| {
            ui.heading("NetLadder");
            ui.add_space(4.0);
            draw_status_pill(ui, snapshot);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                egui::global_theme_preference_switch(ui);
                ui.add_space(4.0);
                let peak = snapshot
                    .detected_capacity
                    .map(|bits| format!("Peak {}", format_rate(bits as f64)))
                    .unwrap_or_else(|| "Waiting for traffic…".to_owned());
                ui.label(RichText::new(peak).color(ui.visuals().weak_text_color()))
                    .on_hover_text(
                        "Highest inbound rate seen since NetLadder started. Informational only.",
                    );
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let total: f64 = snapshot.rows.iter().map(|row| row.bits_per_second).sum();
            let limited = snapshot
                .rows
                .iter()
                .filter(|row| snapshot.limits.contains_key(&row.name))
                .count();
            ui.label(RichText::new(format!("Total {}", format_rate(total))).strong());
            ui.label(
                RichText::new(format!(
                    "·  {}  ·  {limited} limited",
                    plural(snapshot.rows.len(), "process", "processes")
                ))
                .weak(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.filter.is_empty()
                    && ui
                        .small_button("✕")
                        .on_hover_text("Clear the filter")
                        .clicked()
                {
                    self.filter.clear();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Filter by name or PID")
                        .desired_width(200.0),
                );
            });
        });
        if let Some(error) = &snapshot.error {
            ui.add_space(8.0);
            Frame::new()
                .fill(STOPPED_COLOR.gamma_multiply(0.15))
                .stroke(Stroke::new(1.0, STOPPED_COLOR.gamma_multiply(0.6)))
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.colored_label(STOPPED_COLOR, error);
                    ui.label(
                        RichText::new(
                            "NetLadder needs administrator rights for WinDivert. Restart it and \
                             accept the elevation prompt, and keep WinDivert.dll and \
                             WinDivert64.sys next to netladder.exe.",
                        )
                        .weak(),
                    );
                });
        }
    }

    fn process_list(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let rows: Vec<&ProcessTraffic> = snapshot
            .rows
            .iter()
            .filter(|traffic| matches_filter(traffic, &self.filter))
            .collect();
        if rows.is_empty() {
            draw_empty_state(ui, snapshot, &self.filter);
            return;
        }

        self.ensure_process_icons(ui.ctx(), &rows);
        let mut changes = Vec::new();
        for traffic in rows {
            let icon = self
                .process_icons
                .get(&traffic.name)
                .and_then(Option::as_ref)
                .map(TextureHandle::id);
            let limit = snapshot.limits.get(&traffic.name).copied();
            let initial_limit = self
                .remembered_limits
                .get(&traffic.name)
                .copied()
                .unwrap_or(DEFAULT_LIMIT_BITS_PER_SECOND);
            let change = ui
                .push_id(&traffic.name, |ui| {
                    draw_process_row(ui, traffic, icon, limit, initial_limit)
                })
                .inner;
            if let Some(limit) = change {
                changes.push((traffic.name.clone(), limit));
            }
            ui.add_space(ROW_SPACING);
        }
        self.apply_limit_changes(changes, &snapshot.limits);
    }

    fn apply_limit_changes(
        &mut self,
        changes: Vec<(String, Option<u64>)>,
        previous: &HashMap<String, u64>,
    ) {
        if changes.is_empty() {
            return;
        }
        let mut state = self.shared.lock().unwrap();
        for (name, limit) in changes {
            if limit.is_some() != previous.contains_key(&name) {
                // Grouping changed, so rebuild the order right away.
                self.row_order = None;
            }
            match limit {
                Some(bits_per_second) => {
                    state
                        .limits_bits_per_second
                        .insert(name.clone(), bits_per_second);
                    self.remembered_limits.insert(name, bits_per_second);
                }
                None => {
                    if let Some(bits_per_second) = previous.get(&name) {
                        self.remembered_limits
                            .insert(name.clone(), *bits_per_second);
                    }
                    state.limits_bits_per_second.remove(&name);
                }
            }
        }
    }

    fn ensure_process_icons(&mut self, context: &egui::Context, rows: &[&ProcessTraffic]) {
        for traffic in rows {
            if self.process_icons.contains_key(&traffic.name) {
                continue;
            }
            // Without a path there is nothing to extract yet; try again once
            // the engine has learned where the executable lives.
            let Some(path) = traffic.executable_path.as_deref() else {
                continue;
            };
            let icon = load_process_icon(path).map(|image| {
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
        let snapshot = self.snapshot();
        egui::Panel::top("netladder-header")
            .resizable(false)
            .frame(Frame::side_top_panel(root.style()).inner_margin(Margin::symmetric(14, 10)))
            .show(root, |ui| self.header(ui, &snapshot));
        egui::Panel::bottom("netladder-footer")
            .resizable(false)
            .frame(Frame::side_top_panel(root.style()).inner_margin(Margin::symmetric(14, 6)))
            .show(root, draw_footer);
        egui::CentralPanel::default()
            .frame(Frame::central_panel(root.style()).inner_margin(Margin {
                left: 14,
                right: 14,
                top: 8,
                bottom: 8,
            }))
            .show(root, |ui| {
                draw_process_header(ui, &mut self.process_sort);
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.process_list(ui, &snapshot));
            });
        root.ctx().request_repaint_after(REPAINT_INTERVAL);
    }
}

fn draw_status_pill(ui: &mut egui::Ui, snapshot: &Snapshot) {
    let (text, color) = if snapshot.running {
        ("Running", RUNNING_COLOR)
    } else if snapshot.error.is_some() {
        ("Stopped", STOPPED_COLOR)
    } else {
        ("Starting", STARTING_COLOR)
    };
    Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(9, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("● {text}")).color(color));
        });
}

fn draw_footer(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.small("Tick a process to limit it and set the value in Mbps. Unticked processes pass through untouched.");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.small(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).weak());
        });
    });
}

fn draw_empty_state(ui: &mut egui::Ui, snapshot: &Snapshot, filter: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(80.0);
        if snapshot.error.is_some() {
            ui.label(RichText::new("The packet engine is not running.").strong());
            ui.add_space(4.0);
            ui.small("Fix the error shown above and start NetLadder again.");
        } else if !filter.trim().is_empty() {
            ui.label(RichText::new(format!("No process matches \"{}\".", filter.trim())).strong());
            ui.add_space(4.0);
            ui.small("Try another name or PID, or clear the filter.");
        } else {
            ui.spinner();
            ui.add_space(12.0);
            ui.label("Waiting for a process to use the network…");
            ui.small("Start a browser or a download and it will appear automatically.");
        }
    });
}

fn draw_process_header(ui: &mut egui::Ui, sort: &mut Option<ProcessSort>) {
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        ui.add_space(f32::from(ROW_MARGIN_X));
        let limit_column_width = CHECKBOX_WIDTH + spacing + LIMIT_INPUT_WIDTH;
        ui.allocate_ui_with_layout(
            vec2(limit_column_width, 22.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(limit_column_width);
                ui.label(RichText::new("Download limit").strong());
            },
        );
        if draw_sort_header(
            ui,
            ICON_SIZE + spacing + NAME_WIDTH,
            "Process",
            SortColumn::Process,
            *sort,
        )
        .on_hover_text("Sort by process name")
        .clicked()
        {
            ProcessSort::select(sort, SortColumn::Process);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(f32::from(ROW_MARGIN_X) + ui.spacing().scroll.allocated_width());
            if draw_sort_header(ui, RATE_WIDTH, "Current usage", SortColumn::Usage, *sort)
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
    width: f32,
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
    let layout = if column == SortColumn::Usage {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    ui.allocate_ui_with_layout(vec2(width, 22.0), layout, |ui| {
        ui.set_min_width(width);
        let mut button = egui::Button::new(RichText::new(label).strong());
        if active_sort.is_some() {
            button = button.selected(true);
        } else {
            button = button.fill(Color32::TRANSPARENT);
        }
        ui.add(button)
    })
    .inner
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

fn matches_filter(traffic: &ProcessTraffic, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_lowercase();
    traffic.name.to_lowercase().contains(&needle)
        || traffic
            .pids
            .iter()
            .any(|pid| pid.to_string().contains(filter))
}

/// Draws one process row. Returns `Some(new_limit)` when the user changed the
/// limit, where `None` inside means the limit was disabled.
fn draw_process_row(
    ui: &mut egui::Ui,
    traffic: &ProcessTraffic,
    icon: Option<egui::TextureId>,
    limit: Option<u64>,
    initial_limit: u64,
) -> Option<Option<u64>> {
    let mut enabled = limit.is_some();
    let mut megabits = limit.unwrap_or(initial_limit) as f64 / 1_000_000.0;
    let mut changed = false;

    let palette = RowPalette::from_visuals(ui.visuals());
    let mut prepared = Frame::new()
        .inner_margin(Margin::symmetric(ROW_MARGIN_X, ROW_MARGIN_Y))
        .corner_radius(6)
        .fill(palette.fill)
        .stroke(Stroke::new(1.0, palette.stroke))
        .begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), ROW_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                let spacing = ui.spacing().item_spacing.x;
                let limit_column_width = CHECKBOX_WIDTH + spacing + LIMIT_INPUT_WIDTH;
                ui.allocate_ui_with_layout(
                    vec2(limit_column_width, 24.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        // Pin the column width so toggling the box never
                        // moves the icon and name next to it.
                        ui.set_min_width(limit_column_width);
                        let hint = if enabled {
                            "Remove the download limit"
                        } else {
                            "Cap the download speed of this process"
                        };
                        changed |= ui
                            .add_sized(
                                [CHECKBOX_WIDTH, 24.0],
                                egui::Checkbox::without_text(&mut enabled),
                            )
                            .on_hover_text(hint)
                            .changed();
                        if enabled {
                            changed |= ui
                                .add_sized(
                                    [LIMIT_INPUT_WIDTH, 24.0],
                                    egui::DragValue::new(&mut megabits)
                                        .range(MIN_LIMIT_MBPS..=MAX_LIMIT_MBPS)
                                        .speed(0.5)
                                        .suffix(" Mbps")
                                        .max_decimals(1)
                                        .update_while_editing(false),
                                )
                                .on_hover_text("Click to type a value or drag to adjust")
                                .changed();
                        } else if ui
                            .add(egui::Label::new("Limit").sense(Sense::click()))
                            .on_hover_text("Cap the download speed of this process")
                            .clicked()
                        {
                            enabled = true;
                            changed = true;
                        }
                    },
                );
                ui.add_space(4.0);
                draw_process_icon(ui, icon, &traffic.name);
                draw_process_identity(ui, traffic);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    draw_process_rate(ui, traffic, limit);
                    if let Some(limit) = limit
                        && usage_bar_fits(ui.available_width())
                    {
                        ui.add_space(6.0);
                        draw_usage_bar(ui, traffic, limit);
                    }
                });
            },
        );
    }
    let widget_rect = prepared.frame.widget_rect(prepared.content_ui.min_rect());
    if ui.rect_contains_pointer(widget_rect) {
        prepared.frame.fill = palette.hover_fill;
    }
    let response = prepared.end(ui);
    if limit.is_some() {
        let rect = response.rect;
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(rect.left() + 1.0, rect.top() + 6.0),
                pos2(rect.left() + 4.0, rect.bottom() - 6.0),
            ),
            2,
            ACCENT,
        );
    }

    changed.then(|| {
        enabled
            .then(|| (megabits.clamp(MIN_LIMIT_MBPS, MAX_LIMIT_MBPS) * 1_000_000.0).round() as u64)
    })
}

struct RowPalette {
    fill: Color32,
    hover_fill: Color32,
    stroke: Color32,
}

impl RowPalette {
    fn from_visuals(visuals: &egui::Visuals) -> Self {
        if visuals.dark_mode {
            Self {
                fill: Color32::from_gray(34),
                hover_fill: Color32::from_gray(42),
                stroke: Color32::from_gray(82),
            }
        } else {
            Self {
                fill: Color32::WHITE,
                hover_fill: Color32::from_gray(245),
                stroke: Color32::from_gray(200),
            }
        }
    }
}

fn draw_process_icon(ui: &mut egui::Ui, icon: Option<egui::TextureId>, name: &str) {
    match icon {
        Some(texture) => {
            ui.add(
                egui::Image::new((texture, vec2(ICON_SIZE, ICON_SIZE)))
                    .fit_to_exact_size(vec2(ICON_SIZE, ICON_SIZE))
                    .corner_radius(4),
            );
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(vec2(ICON_SIZE, ICON_SIZE), Sense::hover());
            let tile = rect.shrink(4.0);
            let visuals = ui.visuals();
            ui.painter()
                .rect_filled(tile, 5, visuals.widgets.inactive.bg_fill);
            let initial = name
                .chars()
                .find(|character| character.is_alphanumeric())
                .map(|character| character.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned());
            ui.painter().text(
                tile.center(),
                Align2::CENTER_CENTER,
                initial,
                FontId::proportional(15.0),
                visuals.weak_text_color(),
            );
        }
    }
}

fn draw_process_identity(ui: &mut egui::Ui, traffic: &ProcessTraffic) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.set_width(NAME_WIDTH);
        let name = ui.add(egui::Label::new(RichText::new(&traffic.name).strong()).truncate());
        if let Some(path) = &traffic.executable_path {
            name.on_hover_text(path);
        }
        let details = format!(
            "{}  ·  {}",
            format_pids(&traffic.pids),
            format_bytes(traffic.total_bytes)
        );
        let details = ui.add(egui::Label::new(RichText::new(details).small().weak()).truncate());
        if traffic.pids.len() > SHOWN_PIDS {
            details.on_hover_text(format!(
                "PID {}",
                traffic
                    .pids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    });
}

fn draw_process_rate(ui: &mut egui::Ui, traffic: &ProcessTraffic, limit: Option<u64>) {
    let near_limit = limit.is_some_and(|limit| usage_fraction(traffic, limit) >= 0.9);
    let mut text = RichText::new(format_rate(traffic.bits_per_second))
        .monospace()
        .size(15.0);
    if near_limit {
        text = text.color(NEAR_LIMIT);
    }
    ui.allocate_ui_with_layout(
        vec2(RATE_WIDTH, 28.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.set_min_width(RATE_WIDTH);
            ui.label(text);
        },
    );
}

fn draw_usage_bar(ui: &mut egui::Ui, traffic: &ProcessTraffic, limit: u64) {
    let (rect, response) = ui.allocate_exact_size(vec2(USAGE_BAR_WIDTH, 8.0), Sense::hover());
    let fraction = usage_fraction(traffic, limit);
    let painter = ui.painter();
    painter.rect_filled(rect, 4, ui.visuals().widgets.inactive.bg_fill);
    if fraction > 0.0 {
        let width = (rect.width() * fraction).max(rect.height());
        let fill = Rect::from_min_size(rect.min, vec2(width, rect.height()));
        let color = if fraction >= 0.9 { NEAR_LIMIT } else { ACCENT };
        painter.rect_filled(fill, 4, color);
    }

    let mut hover = format!(
        "{} of {} ({:.0}%)",
        format_rate(traffic.bits_per_second),
        format_rate(limit as f64),
        fraction * 100.0
    );
    if traffic.dropped_bytes > 0 {
        hover.push_str(&format!(
            "\nDropped {} while the queue was full.",
            format_bytes(traffic.dropped_bytes)
        ));
    }
    response.on_hover_text(hover);
}

/// The bar is skipped rather than drawn over the process name when the row
/// is too narrow to hold it next to the rate.
fn usage_bar_fits(available_width: f32) -> bool {
    available_width >= USAGE_BAR_WIDTH + 6.0
}

fn usage_fraction(traffic: &ProcessTraffic, limit: u64) -> f32 {
    (traffic.bits_per_second / limit.max(1) as f64).clamp(0.0, 1.0) as f32
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

const SHOWN_PIDS: usize = 3;

fn format_pids(pids: &[u32]) -> String {
    if pids.is_empty() {
        return "PID unknown".to_owned();
    }
    let shown = pids
        .iter()
        .take(SHOWN_PIDS)
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if pids.len() > SHOWN_PIDS {
        format!("PID {shown} +{}", pids.len() - SHOWN_PIDS)
    } else {
        format!("PID {shown}")
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
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
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Debug builds show sample rows when `NETLADDER_PREVIEW` is set, so the
/// layout can be checked without administrator rights or traffic. Release
/// builds never enter preview mode.
pub fn preview_requested() -> bool {
    cfg!(debug_assertions) && std::env::var_os("NETLADDER_PREVIEW").is_some()
}

#[cfg(debug_assertions)]
fn seed_preview_rows(shared: &Shared) {
    if !preview_requested() {
        return;
    }
    let now = Instant::now();
    let samples: [(&str, Option<&str>, f64, u64, u64, &[u32]); 5] = [
        (
            "chrome.exe",
            Some(r"C:\Windows\explorer.exe"),
            48_200_000.0,
            2_147_483_648,
            0,
            &[4120, 4188, 5010, 5222],
        ),
        (
            "steam.exe",
            Some(r"C:\Windows\System32\notepad.exe"),
            9_800_000.0,
            15_032_385_536,
            3_145_728,
            &[7788],
        ),
        (
            "Discord.exe",
            None,
            320_000.0,
            104_857_600,
            0,
            &[9001, 9002],
        ),
        (
            "한글 프로그램.exe",
            None,
            2_500_000.0,
            52_428_800,
            0,
            &[1234],
        ),
        ("Unknown process", None, 12_000.0, 40_960, 0, &[]),
    ];
    let mut state = shared.lock().unwrap();
    for (name, path, bits_per_second, total_bytes, dropped_bytes, pids) in samples {
        state.order.push(name.to_owned());
        state.traffic.insert(
            name.to_owned(),
            ProcessTraffic {
                name: name.to_owned(),
                executable_path: path.map(str::to_owned),
                pids: pids.to_vec(),
                bits_per_second,
                total_bytes,
                dropped_bytes,
                last_seen: now,
            },
        );
    }
    state
        .limits_bits_per_second
        .insert("steam.exe".to_owned(), 10_000_000);
    state
        .limits_bits_per_second
        .insert("Discord.exe".to_owned(), 5_000_000);
}

/// Adds Malgun Gothic so Korean process names render. It leads the
/// proportional family and only backs up the monospace family, so monospace
/// text keeps its fixed-width font.
fn install_korean_font(context: &egui::Context) {
    let Ok(bytes) = fs::read(r"C:\Windows\Fonts\malgun.ttf") else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("malgun".into(), FontData::from_owned(bytes).into());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "malgun".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("malgun".into());
    context.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Instant};

    use super::{
        ProcessSort, SortColumn, SortDirection, USAGE_BAR_WIDTH, format_bytes, format_pids,
        format_rate, matches_filter, plural, sort_process_rows, usage_bar_fits,
    };
    use crate::engine::ProcessTraffic;

    fn traffic(name: &str, bits_per_second: f64) -> ProcessTraffic {
        ProcessTraffic {
            name: name.to_owned(),
            executable_path: None,
            pids: Vec::new(),
            bits_per_second,
            total_bytes: 0,
            dropped_bytes: 0,
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
        assert_eq!(format_rate(0.0), "0 bps");
    }

    #[test]
    fn formats_byte_totals() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2_048), "2 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(3_221_225_472), "3.0 GiB");
    }

    #[test]
    fn formats_pid_lists_compactly() {
        assert_eq!(format_pids(&[]), "PID unknown");
        assert_eq!(format_pids(&[42]), "PID 42");
        assert_eq!(format_pids(&[1, 2, 3, 4, 5]), "PID 1, 2, 3 +2");
        assert_eq!(plural(1, "process", "processes"), "1 process");
        assert_eq!(plural(3, "process", "processes"), "3 processes");
    }

    #[test]
    fn usage_bar_only_when_it_fits() {
        assert!(usage_bar_fits(USAGE_BAR_WIDTH + 6.0));
        assert!(usage_bar_fits(400.0));
        assert!(!usage_bar_fits(USAGE_BAR_WIDTH));
        assert!(!usage_bar_fits(0.0));
    }

    #[test]
    fn filters_by_name_or_pid() {
        let mut row = traffic("Chrome.exe", 0.0);
        row.pids = vec![1234, 5678];
        assert!(matches_filter(&row, ""));
        assert!(matches_filter(&row, "  "));
        assert!(matches_filter(&row, "chrome"));
        assert!(matches_filter(&row, "5678"));
        assert!(matches_filter(&row, " 12 "));
        assert!(!matches_filter(&row, "firefox"));
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
