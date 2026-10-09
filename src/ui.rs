use crate::{
    app::{App, Sort},
    metrics::{memory_ratio, Process},
};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, Wrap},
};

// ANSI colors also work in macOS Terminal, which does not require true color.
const TEXT: Color = Color::Gray;
const MUTED: Color = Color::DarkGray;
const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const YELLOW: Color = Color::Yellow;
const RED: Color = Color::Red;
const VIOLET: Color = Color::LightMagenta;
const SURFACE: Color = Color::Black;

fn style(color: Color) -> Style {
    Style::default().fg(color)
}
fn heat(value: f64) -> Color {
    if value >= 90.0 {
        RED
    } else if value >= 70.0 {
        YELLOW
    } else {
        GREEN
    }
}
fn compact(value: u64) -> String {
    let mut n = value as f64;
    let units = ["B", "K", "M", "G", "T"];
    let mut unit = 0;
    while n >= 1024.0 && unit < units.len() - 1 {
        n /= 1024.0;
        unit += 1;
    }
    if unit < 2 {
        format!("{n:.0}{}", units[unit])
    } else {
        format!("{n:.2}{}", units[unit])
    }
}
fn optional_bytes(value: Option<u64>) -> String {
    value.map(compact).unwrap_or_else(|| "N/A".into())
}
fn elapsed(seconds: u64) -> String {
    if seconds >= 86400 {
        format!("{}d{:02}h", seconds / 86400, seconds / 3600 % 24)
    } else {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}
fn row(area: Rect, index: u16) -> Rect {
    Rect::new(
        area.x,
        area.y + index.min(area.height),
        area.width,
        u16::from(index < area.height),
    )
}
fn text(f: &mut Frame, area: Rect, value: impl Into<String>, color: Color) {
    f.render_widget(Paragraph::new(value.into()).style(style(color)), area);
}
fn meter(
    f: &mut Frame,
    area: Rect,
    label: &str,
    fraction: Option<f64>,
    value: String,
    color: Color,
) {
    if area.height == 0 || area.width < 10 {
        return;
    }
    let width = usize::from(area.width.saturating_sub(6));
    let value: String = value.chars().take(width).collect();
    let bars = width.saturating_sub(value.chars().count());
    let filled = (fraction.unwrap_or(0.0).clamp(0.0, 1.0) * bars as f64).round() as usize;
    let line = Line::from(vec![
        Span::styled(format!("{label:>4}"), style(CYAN).bold()),
        Span::raw(" "),
        Span::styled("━".repeat(filled), style(color)),
        Span::styled("─".repeat(bars.saturating_sub(filled)), style(MUTED)),
        Span::styled(value, style(CYAN).bold()),
        Span::raw(" "),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn panel(f: &mut Frame, area: Rect, title: &str, color: Color) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style(color))
        .title(Line::from(format!(" {title} ")).style(style(color).bold()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::default().style(style(TEXT).bg(SURFACE)), area);
    if area.width < 60 || area.height < 16 {
        text(f, area, "mtr\nResize to at least 60 × 16.\nq to quit", CYAN);
        return;
    }
    let detail_height = area.height.saturating_sub(14).clamp(3, 8);
    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Length(detail_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);
    let state = if app.paused {
        "PAUSED"
    } else if app.snapshot.sequence == 0 {
        "SAMPLING"
    } else if app
        .snapshot
        .collected_at
        .is_some_and(|t| t.elapsed().as_millis() > u128::from(app.interval_ms) * 3)
    {
        "STALE"
    } else {
        "LIVE"
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" mtr ", style(SURFACE).bg(CYAN).bold()),
            Span::styled(
                format!("  ● {state}  "),
                style(if state == "LIVE" { GREEN } else { YELLOW }),
            ),
            Span::styled(
                format!(
                    "{} · {} · {}ms",
                    app.snapshot.host, app.snapshot.cpu_name, app.interval_ms
                ),
                style(TEXT),
            ),
        ])),
        sections[0],
    );
    let s = &app.snapshot;
    text(
        f,
        sections[3],
        if cfg!(target_os = "macos") {
            format!(
                " App {} · Wired {} · Compressed {} · Cached {} · Up {}",
                optional_bytes(s.memory_detail.app_bytes),
                optional_bytes(s.memory_detail.wired_bytes),
                optional_bytes(s.memory_detail.compressed_bytes),
                optional_bytes(s.memory_detail.cached_files_bytes),
                s.uptime.map(elapsed).unwrap_or_else(|| "N/A".into())
            )
        } else {
            format!(
                " {} {} · Compressed {} · Up {} · {:.1}ms",
                if s.memory_detail.label.is_empty() {
                    "Cache"
                } else {
                    &s.memory_detail.label
                },
                s.memory_detail
                    .cache_bytes
                    .map(compact)
                    .unwrap_or_else(|| "N/A".into()),
                s.memory_detail
                    .compressed_bytes
                    .map(compact)
                    .unwrap_or_else(|| "N/A".into()),
                s.uptime.map(elapsed).unwrap_or_else(|| "N/A".into()),
                s.collection_ms
            )
        },
        MUTED,
    );
    overview(f, sections[1], app);
    details(f, sections[2], app);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" PROCESSES ", style(SURFACE).bg(VIOLET).bold()),
            Span::styled(
                format!(
                    "  {} {}  ·  {}/{} tasks  {}",
                    app.sort.label(),
                    if app.reversed { "↑" } else { "↓" },
                    app.visible.len(),
                    app.snapshot.processes.len(),
                    if app.query.is_empty() {
                        String::new()
                    } else {
                        format!("/{}", app.query)
                    }
                ),
                style(VIOLET),
            ),
        ])),
        sections[4],
    );
    app.process_rows = usize::from(sections[5].height.saturating_sub(1));
    processes(f, sections[5], app);
    footer(f, sections[6], app);
    if app.help {
        help(f);
    }
}

fn recent_history(history: &std::collections::VecDeque<u64>, width: u16) -> Vec<u64> {
    history
        .iter()
        .skip(history.len().saturating_sub(usize::from(width)))
        .copied()
        .collect()
}

fn overview(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(33),
        Constraint::Percentage(33),
    ])
    .split(area);
    let s = &app.snapshot;
    let cpu = panel(f, cols[0], "CPU", CYAN);
    text(
        f,
        row(cpu, 0),
        if s.sequence == 0 {
            " Sampling…".into()
        } else {
            format!(
                " {}  ·  {} cores",
                s.cpu_usage
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "N/A".into()),
                s.cores.len()
            )
        },
        CYAN,
    );
    let history = recent_history(&s.cpu_history, cpu.width);
    f.render_widget(
        Sparkline::default()
            .data(&history)
            .max(100)
            .style(style(CYAN)),
        row(cpu, 1),
    );
    text(
        f,
        row(cpu, 2),
        if s.sequence == 0 {
            " Load sampling…".into()
        } else {
            s.load
                .map(|v| format!(" Load {:.2}  {:.2}  {:.2}", v[0], v[1], v[2]))
                .unwrap_or_else(|| " Load N/A".into())
        },
        MUTED,
    );
    let ram = panel(f, cols[1], "MEMORY", VIOLET);
    text(
        f,
        row(ram, 0),
        if s.sequence == 0 {
            " Sampling…".into()
        } else {
            format!(
                " {} / {}",
                optional_bytes(s.memory_used),
                optional_bytes(s.memory_total)
            )
        },
        VIOLET,
    );
    let history = recent_history(&s.memory_history, ram.width);
    f.render_widget(
        Sparkline::default()
            .data(&history)
            .max(100)
            .style(style(VIOLET)),
        row(ram, 1),
    );
    text(
        f,
        row(ram, 2),
        format!(
            " Swap {} / {}",
            optional_bytes(s.swap_used),
            optional_bytes(s.swap_total)
        ),
        MUTED,
    );
    let gpu = panel(f, cols[2], "GPU", GREEN);
    let g = s.gpus.first();
    meter(
        f,
        row(gpu, 0),
        "GPU",
        g.and_then(|g| g.utilization).map(|v| v / 100.0),
        g.and_then(|g| g.utilization)
            .map(|v| format!("{v:.1}%"))
            .unwrap_or_else(|| "N/A".into()),
        GREEN,
    );
    text(
        f,
        row(gpu, 1),
        g.map(|g| {
            format!(
                " {}{}",
                g.name,
                if s.gpus.len() > 1 {
                    format!(" +{} GPUs", s.gpus.len() - 1)
                } else {
                    String::new()
                }
            )
        })
        .unwrap_or_else(|| " No GPU telemetry".into()),
        TEXT,
    );
    text(
        f,
        row(gpu, 2),
        g.map(|g| {
            format!(
                " {} {}",
                g.memory_used.map(compact).unwrap_or_else(|| "N/A".into()),
                if g.source.contains("shared") {
                    "shared"
                } else {
                    "VRAM"
                }
            )
        })
        .unwrap_or_default(),
        MUTED,
    );
}

fn details(f: &mut Frame, area: Rect, app: &mut App) {
    let cols =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area);
    let core_rows = usize::from(area.height.saturating_sub(2)).max(1);
    let per_page = core_rows * 2;
    let pages = app.snapshot.cores.len().div_ceil(per_page).max(1);
    app.core_page = app.core_page.min(pages - 1);
    let title = if pages > 1 {
        format!("CPU CORES · {}/{} [ / ]", app.core_page + 1, pages)
    } else {
        "CPU CORES".into()
    };
    let inner = panel(f, cols[0], &title, CYAN);
    let cores =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inner);
    for (column, col) in cores.iter().enumerate() {
        for r in 0..core_rows {
            let index = app.core_page * per_page + column * core_rows + r;
            if let Some(usage) = app.snapshot.cores.get(index) {
                meter(
                    f,
                    row(*col, r as u16),
                    &index.to_string(),
                    usage.map(|v| f64::from(v) / 100.0),
                    usage
                        .map(|v| format!("{v:.0}%"))
                        .unwrap_or_else(|| "N/A".into()),
                    usage.map(|v| heat(f64::from(v))).unwrap_or(MUTED),
                );
            }
        }
    }
    let inner = panel(f, cols[1], "TEMPERATURE · °C", YELLOW);
    let readings: Vec<_> = app
        .snapshot
        .temperatures
        .iter()
        .filter(|t| t.celsius.is_some())
        .collect();
    if readings.is_empty() {
        text(
            f,
            row(inner, 0),
            if app.snapshot.sequence == 0 {
                " Sampling sensors…"
            } else {
                " N/A · no sensor readings"
            },
            MUTED,
        );
        text(f, row(inner, 1), " macOS / hardware dependent", MUTED);
    } else {
        // Hottest first; reserve a row for the count when readings overflow.
        let count = usize::from(inner.height);
        let shown = if readings.len() > count && count > 1 {
            count - 1
        } else {
            count
        };
        for (index, reading) in readings.iter().take(shown).enumerate() {
            let value = reading.celsius.unwrap();
            let label: String = reading
                .label
                .chars()
                .take(usize::from(inner.width.saturating_sub(10)))
                .collect();
            let line = Line::from(vec![
                Span::styled(
                    format!(" {value:4.1}°C "),
                    style(heat(f64::from(value))).bold(),
                ),
                Span::styled(label, style(TEXT)),
            ]);
            f.render_widget(Paragraph::new(line), row(inner, index as u16));
        }
        if shown < readings.len() && count > 1 {
            text(
                f,
                row(inner, shown as u16),
                format!(" +{} sensors · --json for all", readings.len() - shown),
                MUTED,
            );
        }
    }
}

fn state(p: &Process) -> &'static str {
    match p.status.as_str() {
        "Run" | "Running" | "Runnable" => "R",
        "Sleep" | "Sleeping" => "S",
        "Idle" => "I",
        "Zombie" => "Z",
        "Stopped" => "T",
        "Dead" => "X",
        _ => "?",
    }
}

fn processes(f: &mut Frame, area: Rect, app: &mut App) {
    let wide = area.width >= 100;
    let header_cell = |name: &'static str, sorted: bool| {
        Cell::from(name).style(style(if sorted { CYAN } else { MUTED }).bg(SURFACE).bold())
    };
    let mut headers = vec![
        header_cell("    PID", app.sort == Sort::Pid),
        header_cell("USER", false),
    ];
    let mut widths = vec![Constraint::Length(7), Constraint::Length(10)];
    if wide {
        headers.push(header_cell("    VIRT", false));
        widths.push(Constraint::Length(8));
    }
    headers.extend([
        header_cell(
            if cfg!(target_os = "macos") {
                "     MEM"
            } else {
                "     RES"
            },
            app.sort == Sort::Memory,
        ),
        header_cell("S", false),
        header_cell(" CPU%", app.sort == Sort::Cpu),
        header_cell(" MEM%", app.sort == Sort::Memory),
    ]);
    widths.extend([
        Constraint::Length(8),
        Constraint::Length(1),
        Constraint::Length(6),
        Constraint::Length(6),
    ]);
    if wide {
        headers.push(header_cell(" ELAPSED", false));
        widths.push(Constraint::Length(9));
    }
    headers.push(header_cell("Command", app.sort == Sort::Name));
    widths.push(Constraint::Min(8));
    let selected = app.table.selected();
    let rows: Vec<Row> = app
        .processes()
        .enumerate()
        .map(|(index, p)| {
            let selected = selected == Some(index);
            let cell = |value: String, color: Color| {
                Cell::from(value).style(style(if selected { Color::Black } else { color }))
            };
            let memory =
                |v: Option<u64>| format!("{:>8}", v.map(compact).unwrap_or_else(|| "N/A".into()));
            let mut cells = vec![
                cell(format!("{:>7}", p.pid), TEXT),
                cell(p.user.clone(), TEXT),
            ];
            if wide {
                cells.push(cell(memory(p.virtual_memory), GREEN));
            }
            cells.extend([
                cell(memory(p.displayed_memory()), CYAN),
                cell(state(p).into(), if state(p) == "R" { GREEN } else { MUTED }),
                cell(
                    p.cpu
                        .map(|v| format!("{v:>6.1}"))
                        .unwrap_or_else(|| "   N/A".into()),
                    TEXT,
                ),
                cell(
                    memory_ratio(p.displayed_memory(), app.snapshot.memory_total)
                        .map(|v| format!("{:>6.1}", v * 100.0))
                        .unwrap_or_else(|| "   N/A".into()),
                    TEXT,
                ),
            ]);
            if wide {
                cells.push(cell(
                    format!(
                        "{:>9}",
                        p.elapsed.map(elapsed).unwrap_or_else(|| "N/A".into())
                    ),
                    TEXT,
                ));
            }
            cells.push(cell(
                if app.full_paths && !p.executable.is_empty() {
                    p.executable.clone()
                } else {
                    p.name.clone()
                },
                TEXT,
            ));
            Row::new(cells).style(style(TEXT))
        })
        .collect();
    let table = Table::new(rows, widths)
        .column_spacing(1)
        .header(Row::new(headers).style(style(MUTED).bg(SURFACE)))
        .row_highlight_style(style(Color::Black).bg(CYAN));
    f.render_stateful_widget(table, area, &mut app.table);
    if app.visible.is_empty() {
        text(
            f,
            row(area, 1),
            if app.snapshot.sequence == 0 {
                " Taking the first CPU sample…"
            } else {
                " No matching processes. F4 clears the filter."
            },
            MUTED,
        );
    }
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    if app.searching {
        text(
            f,
            area,
            format!("Search: {}▏  Enter apply · Esc clear", app.query),
            CYAN,
        );
        return;
    }
    let actions = [
        ("F1", "Help"),
        ("F2", "Paths"),
        ("F3", "Search"),
        ("F4", "Clear"),
        ("F5", if app.paused { "Resume" } else { "Pause" }),
        ("F6", "Sort"),
        ("F7", "CPU"),
        ("F8", "Mem"),
        ("F9", "Reverse"),
        ("F10", "Quit"),
    ];
    let mut spans = Vec::new();
    for (key, name) in actions {
        if area.width < 85 && matches!(key, "F2" | "F4" | "F7" | "F8" | "F9") {
            continue;
        }
        spans.push(Span::styled(key, style(TEXT)));
        spans.push(Span::styled(format!("{name:<6}"), style(CYAN).bg(SURFACE)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn help(f: &mut Frame) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(78);
    let height = area.height.saturating_sub(2).min(25);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    f.render_widget(Clear, popup);
    let text = "KEYBOARD\nF1 / ?       Help             F2 / p      Full paths / names\nF3 / /       Search           F4 / Esc    Clear filter\nF5 / Space   Pause display    F6 / s      Cycle sort column\nF7 / c       Sort CPU         F8 / m      Sort memory\nF9 / r       Reverse sort     F10 / q     Quit\n↑↓ / j k     Move selection   PgUp/PgDn   Scroll one page\nHome / End   First / last     [ / ]       CPU meter pages\nCtrl+C       Quit (also while searching)\nOn Mac, hold Fn if the function keys control brightness or volume.\n\nREADINGS\nCPU meters show total busy time, 0–100%; color indicates load.\nProcess CPU can exceed 100% across cores. MEM is macOS physical footprint; RES elsewhere is resident RAM;\nVIRT includes reserved address space. ELAPSED is wall time, not CPU\ntime. R means runnable, not necessarily executing on a core.\nGPU is driver-reported; Apple GPU memory is shared with RAM.\nTemperatures: hottest sensors first, in °C; all sensors in --json.\nColors: <70 green, 70–89 yellow, ≥90 red (visual guides only).\nMemory units: K/M/G are KiB/MiB/GiB (powers of 1024).\nFile-backed / cache overlaps RAM; do not add it to used RAM.\nN/A means unavailable. Shared process pages may be counted twice.\n\nPress F1, ? or Esc to close";
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" mtr · field guide ")
                    .border_style(style(CYAN)),
            )
            .style(style(TEXT).bg(Color::Black)),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Snapshot;
    use ratatui::{backend::TestBackend, Terminal};
    #[test]
    fn graphs_show_the_newest_samples_after_history_fills() {
        let history = (0..120).collect();
        assert_eq!(recent_history(&history, 3), vec![117, 118, 119]);
        assert_eq!(recent_history(&history, 0), Vec::<u64>::new());
        assert_eq!(recent_history(&history, 200).len(), 120);
    }
    #[test]
    fn populated_layout_keeps_table_and_shortcuts_visible() {
        let mut app = App::new(1000);
        app.update(Snapshot {
            sequence: 1,
            cores: vec![Some(25.0); 8],
            cpu_name: "Apple M1".into(),
            memory_total: Some(8 * 1024 * 1024 * 1024),
            memory_used: Some(6 * 1024 * 1024 * 1024),
            processes: vec![Process {
                pid: 42,
                name: "mtr".into(),
                user: "ayush".into(),
                cpu: Some(2.5),
                memory: Some(8192),
                executable: "/usr/local/bin/mtr".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = (0..36)
            .map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let screen = lines.join("\n");
        assert!(
            screen.contains("Command") && screen.contains("VIRT") && screen.contains("ELAPSED")
        );
        assert!(screen.contains("/usr/local/bin/mtr") && screen.contains("1/1 tasks"));
        assert!(lines[35].contains("F10Quit"));
        assert!(screen.contains("TEMPERATURE") && screen.contains("no sensor readings"));
        assert!(app.process_rows >= 18);
        // The populated sensor path must render a genuine Celsius reading.
        app.snapshot.temperatures = vec![crate::metrics::Temperature {
            label: "CPU performance core".into(),
            celsius: Some(72.5),
        }];
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("72.5°C") && screen.contains("CPU performance core"));
        if let Ok(path) = std::env::var("MTR_TEST_SCREEN") {
            let cells: Vec<_> = buffer.content.iter().map(|c| serde_json::json!({"text":c.symbol(),"fg":format!("{:?}",c.fg),"bg":format!("{:?}",c.bg)})).collect();
            std::fs::write(path, serde_json::to_string(&cells).unwrap()).unwrap();
        }
    }
    #[test]
    fn renders_small_terminals_and_many_cores() {
        for (w, h) in [(120, 40), (80, 24), (60, 16), (20, 8), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut app = App::new(1000);
            app.snapshot.cores = vec![Some(100.0); 256];
            app.core_page = usize::MAX;
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            app.help = true;
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
}
