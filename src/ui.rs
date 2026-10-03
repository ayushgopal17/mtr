use crate::{
    app::{App, Sort},
    metrics::{ratio, Process},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

// ANSI colors also work in macOS Terminal, which does not require true color.
const TEXT: Color = Color::Gray;
const MUTED: Color = Color::DarkGray;
const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const YELLOW: Color = Color::Yellow;
const RED: Color = Color::Red;

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
        Span::styled("[", style(TEXT)),
        Span::styled("|".repeat(filled), style(color)),
        Span::raw(" ".repeat(bars.saturating_sub(filled))),
        Span::styled(value, style(CYAN).bold()),
        Span::styled("]", style(TEXT)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::default().style(style(TEXT).bg(Color::Reset)), area);
    if area.width < 60 || area.height < 16 {
        text(f, area, "mtr\nResize to at least 60 × 16.\nq to quit", CYAN);
        return;
    }
    let core_rows = app
        .snapshot
        .cores
        .len()
        .div_ceil(2)
        .max(1)
        .min(usize::from(area.height.saturating_sub(12) / 2).max(1));
    let top_height = core_rows as u16 + 5;
    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(top_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
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
    text(
        f,
        sections[0],
        format!(
            " mtr  {}  ·  {}  ·  {}  ·  {}ms",
            app.snapshot.host, app.snapshot.cpu_name, state, app.interval_ms
        ),
        MUTED,
    );
    summary(f, sections[1], app, core_rows);
    let tab = Line::from(vec![
        Span::styled(" Main ", style(Color::Black).bg(GREEN).bold()),
        Span::styled(
            format!(
                "  {} {}  |  {}/{} processes{}",
                app.sort.label(),
                if app.reversed { "↑" } else { "↓" },
                app.visible.len(),
                app.snapshot.processes.len(),
                if app.query.is_empty() {
                    String::new()
                } else {
                    format!("  /{}", app.query)
                }
            ),
            style(CYAN),
        ),
    ]);
    f.render_widget(Paragraph::new(tab), sections[3]);
    app.process_rows = usize::from(sections[4].height.saturating_sub(1));
    processes(f, sections[4], app);
    footer(f, sections[5], app);
    if app.help {
        help(f);
    }
}

fn summary(f: &mut Frame, area: Rect, app: &mut App, core_rows: usize) {
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(2)
        .split(area);
    let s = &app.snapshot;
    let per_page = core_rows * 2;
    let pages = s.cores.len().div_ceil(per_page).max(1);
    app.core_page = app.core_page.min(pages - 1);
    for column in 0..2 {
        for r in 0..core_rows {
            let index = app.core_page * per_page + column * core_rows + r;
            if let Some(usage) = s.cores.get(index) {
                meter(
                    f,
                    row(cols[column], r as u16),
                    &index.to_string(),
                    Some(f64::from(*usage) / 100.0),
                    format!("{usage:5.1}%"),
                    heat(f64::from(*usage)),
                );
            }
        }
    }
    let base = core_rows as u16;
    let ready = s.sequence > 0;
    meter(
        f,
        row(cols[0], base),
        "Mem",
        ready.then(|| ratio(s.memory_used, s.memory_total)),
        if ready {
            format!("{}/{}", compact(s.memory_used), compact(s.memory_total))
        } else {
            "sampling".into()
        },
        GREEN,
    );
    meter(
        f,
        row(cols[0], base + 1),
        "Swp",
        ready.then(|| ratio(s.swap_used, s.swap_total)),
        if ready {
            format!("{}/{}", compact(s.swap_used), compact(s.swap_total))
        } else {
            "sampling".into()
        },
        RED,
    );
    let gpu = s.gpus.first();
    meter(
        f,
        row(cols[0], base + 2),
        "GPU",
        gpu.and_then(|g| g.utilization).map(|v| v / 100.0),
        gpu.and_then(|g| g.utilization)
            .map(|v| format!("{v:.1}%"))
            .unwrap_or_else(|| "N/A".into()),
        CYAN,
    );
    let gpu_info = gpu
        .map(|g| {
            format!(
                "     {}  {}{}{}",
                g.name,
                g.memory_used.map(compact).unwrap_or_else(|| "N/A".into()),
                if g.source.contains("shared") {
                    " shared"
                } else {
                    " VRAM"
                },
                if s.gpus.len() > 1 {
                    format!("  +{} GPUs (--json)", s.gpus.len() - 1)
                } else {
                    String::new()
                }
            )
        })
        .unwrap_or_else(|| "     GPU telemetry unavailable".into());
    text(f, row(cols[0], base + 3), gpu_info, MUTED);
    text(
        f,
        row(cols[0], base + 4),
        if pages > 1 {
            format!(
                "     CPU page {}/{}  [ / ] change page",
                app.core_page + 1,
                pages
            )
        } else {
            format!(
                "     CPU {:.1}%  ·  {:.1}ms collect",
                s.cpu_usage, s.collection_ms
            )
        },
        MUTED,
    );
    let running = s
        .processes
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Run" | "Running" | "Runnable"))
        .count();
    text(
        f,
        row(cols[1], base),
        format!("Tasks: {}   {} runnable", s.processes.len(), running),
        CYAN,
    );
    text(
        f,
        row(cols[1], base + 1),
        format!(
            "Load average: {:.2} {:.2} {:.2}",
            s.load[0], s.load[1], s.load[2]
        ),
        CYAN,
    );
    text(
        f,
        row(cols[1], base + 2),
        format!(
            "Uptime: {}",
            s.uptime.map(elapsed).unwrap_or_else(|| "N/A".into())
        ),
        CYAN,
    );
    text(
        f,
        row(cols[1], base + 3),
        format!(
            "Cache: {}  Compressed: {}",
            s.memory_detail
                .cache_bytes
                .map(compact)
                .unwrap_or_else(|| "N/A".into()),
            s.memory_detail
                .compressed_bytes
                .map(compact)
                .unwrap_or_else(|| "N/A".into())
        ),
        YELLOW,
    );
    text(
        f,
        row(cols[1], base + 4),
        if s.cpu_caches.is_empty() {
            "CPU cache sizes: N/A".into()
        } else {
            s.cpu_caches.clone()
        },
        MUTED,
    );
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
        Cell::from(name).style(style(Color::Black).bg(if sorted { CYAN } else { GREEN }))
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
        header_cell("     RES", false),
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
                cell(memory(p.memory), CYAN),
                cell(state(p).into(), if state(p) == "R" { GREEN } else { MUTED }),
                cell(
                    p.cpu
                        .map(|v| format!("{v:>6.1}"))
                        .unwrap_or_else(|| "   N/A".into()),
                    TEXT,
                ),
                cell(
                    p.memory
                        .map(|v| format!("{:>6.1}", ratio(v, app.snapshot.memory_total) * 100.0))
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
            Row::new(cells)
        })
        .collect();
    let table = Table::new(rows, widths)
        .column_spacing(1)
        .header(Row::new(headers).style(style(Color::Black).bg(GREEN)))
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
        spans.push(Span::styled(
            format!("{name:<6}"),
            style(Color::Black).bg(CYAN),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn help(f: &mut Frame) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(78);
    let height = area.height.saturating_sub(2).min(22);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    f.render_widget(Clear, popup);
    let text = "KEYBOARD\nF1 / ?       Help             F2 / p      Full paths / names\nF3 / /       Search           F4 / Esc    Clear filter\nF5 / Space   Pause display    F6 / s      Cycle sort column\nF7 / c       Sort CPU         F8 / m      Sort memory\nF9 / r       Reverse sort     F10 / q     Quit\n↑↓ / j k     Move selection   PgUp/PgDn   Scroll one page\nHome / End   First / last     [ / ]       CPU meter pages\nCtrl+C       Quit (also while searching)\nOn Mac, hold Fn if the function keys control brightness or volume.\n\nREADINGS\nCPU meters show total busy time, 0–100%; color indicates load.\nProcess CPU can exceed 100% across cores. RES is resident RAM;\nVIRT includes reserved address space. ELAPSED is wall time, not CPU\ntime. R means runnable, not necessarily executing on a core.\nGPU is driver-reported; Apple GPU memory is shared with RAM.\nCache is file-backed / reclaimable memory, not additional RAM.\nN/A means unavailable. Shared process pages may be counted twice.\n\nPress F1, ? or Esc to close";
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
    fn populated_layout_keeps_table_and_shortcuts_visible() {
        let mut app = App::new(1000);
        app.update(Snapshot {
            sequence: 1,
            cores: vec![25.0; 8],
            cpu_name: "Apple M1".into(),
            memory_total: 8 * 1024 * 1024 * 1024,
            memory_used: 6 * 1024 * 1024 * 1024,
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
        assert!(screen.contains("/usr/local/bin/mtr") && screen.contains("Tasks: 1"));
        assert!(lines[35].contains("F10Quit"));
        assert_eq!(buffer[(0, 12)].bg, GREEN);
        assert_eq!(buffer[(0, 13)].bg, CYAN);
        assert!(app.process_rows >= 20);
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
            app.snapshot.cores = vec![100.0; 256];
            app.core_page = usize::MAX;
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            app.help = true;
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
}
