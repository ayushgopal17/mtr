mod app;
mod metrics;
mod platform;
mod ui;

use app::{App, Sort};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
};
use std::{
    env,
    error::Error,
    io::{self, IsTerminal},
    thread,
    time::Duration,
};

const HELP: &str = "mtr — CPU · GPU · memory · temperature\n\nUsage: mtr [OPTIONS]\n\n  -i, --interval MS   Sampling interval, 250–60000 ms (default: 1000)\n      --json          Print one live JSON sample and exit\n      --benchmark N   Collect N samples and report collection cost (1–1000)\n  -h, --help          Show this help\n  -V, --version       Print version\n\nKeys: q quit · / search · c CPU · m memory · s sort · r reverse\n      ↑/↓ scroll · space pause · ? help\n\nGPU support: Apple IORegistry, Linux AMD DRM and NVIDIA nvidia-smi.\nUnavailable metrics are reported as N/A (JSON null), never fabricated.\n";

#[derive(Debug, PartialEq)]
struct Options {
    interval: u64,
    json: bool,
    benchmark: Option<usize>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut args = args;
    let mut options = Options {
        interval: 1000,
        json: false,
        benchmark: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-i" | "--interval" => {
                options.interval = args
                    .next()
                    .ok_or("--interval needs milliseconds")?
                    .parse()
                    .map_err(|_| "Invalid interval")?;
                if !(250..=60000).contains(&options.interval) {
                    return Err("Interval must be between 250 and 60000 ms".into());
                }
            }
            "--json" => options.json = true,
            "--benchmark" => {
                let count = args
                    .next()
                    .ok_or("--benchmark needs a sample count")?
                    .parse()
                    .map_err(|_| "Invalid sample count")?;
                if !(1..=1000).contains(&count) {
                    return Err("Sample count must be between 1 and 1000".into());
                }
                options.benchmark = Some(count);
            }
            _ => return Err(format!("Unknown option: {arg}. Use --help.")),
        }
    }
    if options.json && options.benchmark.is_some() {
        return Err("Choose either --json or --benchmark".into());
    }
    Ok(options)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("mtr: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mtr {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = parse_args(args.into_iter())?;
    let interval = Duration::from_millis(options.interval);
    if options.json || options.benchmark.is_some() {
        let mut collector = metrics::Collector::new();
        thread::sleep(interval.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
        if options.json {
            println!("{}", serde_json::to_string_pretty(&collector.sample())?);
        } else {
            let count = options.benchmark.unwrap_or(1);
            let mut durations = Vec::with_capacity(count);
            for i in 0..count {
                let start = std::time::Instant::now();
                durations.push(collector.sample().collection_ms);
                if i + 1 < count {
                    thread::sleep(interval.saturating_sub(start.elapsed()));
                }
            }
            durations.sort_by(f64::total_cmp);
            let mean = durations.iter().sum::<f64>() / count as f64;
            let p95 = durations[(count * 95).div_ceil(100).saturating_sub(1)];
            println!("{count} live samples · {}ms interval\nCollection wall time: mean {mean:.2}ms · p95 {p95:.2}ms · max {:.2}ms\nThis measures collection latency, not process CPU utilization.", options.interval, durations[count - 1]);
        }
        return Ok(());
    }
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err("Interactive mode needs a terminal. Use --json for a snapshot.".into());
    }
    let worker = metrics::Worker::start(interval);
    let mut terminal = ratatui::init();
    // Extend Ratatui's panic cleanup to restore normal terminal mouse behavior.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        previous_hook(info);
    }));
    // Capture wheel/trackpad events in the alternate screen, so they scroll
    // processes instead of moving the terminal into its shell scrollback.
    let result = execute!(io::stdout(), EnableMouseCapture)
        .and_then(|()| terminal.clear())
        .and_then(|()| dashboard(&mut terminal, &worker, options.interval));
    let cleanup = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result?;
    cleanup?;
    Ok(())
}

fn dashboard(
    terminal: &mut ratatui::DefaultTerminal,
    worker: &metrics::Worker,
    interval_ms: u64,
) -> io::Result<()> {
    let mut app = App::new(interval_ms);
    let mut dirty = true;
    let mut was_stale = false;
    loop {
        if !app.paused {
            let next = worker.latest.lock().ok().and_then(|mut slot| slot.take());
            if let Some(snapshot) = next {
                app.update(snapshot);
                dirty = true;
            }
        }
        let stale = app
            .snapshot
            .collected_at
            .is_some_and(|t| t.elapsed().as_millis() > u128::from(interval_ms) * 3);
        if stale != was_stale {
            dirty = true;
            was_stale = stale;
        }
        if dirty {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            dirty = false;
        }
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        match event::read()? {
            Event::Resize(_, _) => dirty = true,
            Event::Mouse(mouse) if !app.help && !app.searching => {
                let delta = match mouse.kind {
                    MouseEventKind::ScrollUp => -3,
                    MouseEventKind::ScrollDown => 3,
                    _ => 0,
                };
                if delta != 0 {
                    app.scroll(delta);
                    dirty = true;
                }
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                dirty = true;
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                if app.help {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?') => app.help = false,
                        KeyCode::F(10) | KeyCode::Char('q') => break,
                        _ => {}
                    }
                    continue;
                }
                if app.searching {
                    match key.code {
                        KeyCode::Esc => {
                            app.query.clear();
                            app.searching = false;
                        }
                        KeyCode::Enter => app.searching = false,
                        KeyCode::Backspace => {
                            app.query.pop();
                        }
                        KeyCode::Char(c) if !c.is_control() && app.query.len() < 256 => {
                            app.query.push(c)
                        }
                        _ => {}
                    }
                    app.refresh_view();
                    continue;
                }
                match key.code {
                    KeyCode::F(10) | KeyCode::Char('q') => break,
                    KeyCode::F(1) | KeyCode::Char('?') => app.help = true,
                    KeyCode::F(3) | KeyCode::Char('/') => app.searching = true,
                    KeyCode::F(2) | KeyCode::Char('p') => app.full_paths = !app.full_paths,
                    KeyCode::Char('[') => app.core_page = app.core_page.saturating_sub(1),
                    KeyCode::Char(']') => app.core_page = app.core_page.saturating_add(1),
                    KeyCode::F(4) | KeyCode::Esc => {
                        app.query.clear();
                        app.refresh_view();
                    }
                    KeyCode::F(5) | KeyCode::Char(' ') => app.paused = !app.paused,
                    KeyCode::F(7) | KeyCode::Char('c') => {
                        app.sort = Sort::Cpu;
                        app.refresh_view();
                    }
                    KeyCode::F(8) | KeyCode::Char('m') => {
                        app.sort = Sort::Memory;
                        app.refresh_view();
                    }
                    KeyCode::F(6) | KeyCode::Char('s') | KeyCode::Tab => {
                        app.sort = app.sort.next();
                        app.refresh_view();
                    }
                    KeyCode::F(9) | KeyCode::Char('r') => {
                        app.reversed = !app.reversed;
                        app.refresh_view();
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.scroll(1),
                    KeyCode::Up | KeyCode::Char('k') => app.scroll(-1),
                    KeyCode::PageDown => app.scroll(app.process_rows as isize),
                    KeyCode::PageUp => app.scroll(-(app.process_rows as isize)),
                    KeyCode::Home => app.scroll(isize::MIN),
                    KeyCode::End => app.scroll(isize::MAX),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }
    #[test]
    fn validates_cli_before_starting_monitor() {
        assert_eq!(parse(&[]).unwrap().interval, 1000);
        assert!(parse(&["--interval", "0"]).is_err());
        assert!(parse(&["--interval"]).is_err());
        assert!(parse(&["--benchmark", "0"]).is_err());
        assert!(parse(&["--json", "--benchmark", "2"]).is_err());
        assert_eq!(
            parse(&["--interval", "250", "--json"]).unwrap().interval,
            250
        );
    }
}
