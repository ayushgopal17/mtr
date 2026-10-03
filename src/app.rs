use crate::metrics::{Process, Snapshot};
use ratatui::widgets::TableState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Cpu,
    Memory,
    Pid,
    Name,
}
impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "MEM",
            Self::Pid => "PID",
            Self::Name => "NAME",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Cpu => Self::Memory,
            Self::Memory => Self::Pid,
            Self::Pid => Self::Name,
            Self::Name => Self::Cpu,
        }
    }
}

pub struct App {
    pub snapshot: Snapshot,
    pub sort: Sort,
    pub reversed: bool,
    pub paused: bool,
    pub help: bool,
    pub searching: bool,
    pub query: String,
    pub table: TableState,
    pub visible: Vec<usize>,
    pub interval_ms: u64,
    pub full_paths: bool,
    pub core_page: usize,
    pub process_rows: usize,
    follow_selection: bool,
}

impl App {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            snapshot: Snapshot::default(),
            sort: Sort::Cpu,
            reversed: false,
            paused: false,
            help: false,
            searching: false,
            query: String::new(),
            table: TableState::default(),
            visible: Vec::new(),
            interval_ms,
            full_paths: true,
            core_page: 0,
            process_rows: 10,
            follow_selection: false,
        }
    }

    pub fn refresh_view(&mut self) {
        self.follow_selection = false;
        self.rebuild(None);
    }

    pub fn update(&mut self, snapshot: Snapshot) {
        let selected_pid = self
            .table
            .selected()
            .and_then(|i| self.visible.get(i))
            .and_then(|i| self.snapshot.processes.get(*i))
            .map(|p| p.pid);
        self.snapshot = snapshot;
        self.rebuild(selected_pid.filter(|_| self.follow_selection));
    }

    fn rebuild(&mut self, selected_pid: Option<u32>) {
        let query = self.query.to_lowercase();
        self.visible = self
            .snapshot
            .processes
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                query.is_empty()
                    || p.name.to_lowercase().contains(&query)
                    || p.executable.to_lowercase().contains(&query)
                    || p.user.to_lowercase().contains(&query)
                    || p.pid.to_string().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();
        self.visible.sort_unstable_by(|a, b| {
            let (a, b) = (&self.snapshot.processes[*a], &self.snapshot.processes[*b]);
            let ordering = match self.sort {
                Sort::Cpu => b.cpu.unwrap_or(-1.0).total_cmp(&a.cpu.unwrap_or(-1.0)),
                Sort::Memory => b.memory.cmp(&a.memory),
                Sort::Pid => a.pid.cmp(&b.pid),
                Sort::Name => a.name.cmp(&b.name),
            }
            .then_with(|| a.pid.cmp(&b.pid));
            if self.reversed {
                ordering.reverse()
            } else {
                ordering
            }
        });
        let position = selected_pid.and_then(|pid| {
            self.visible
                .iter()
                .position(|i| self.snapshot.processes[*i].pid == pid)
        });
        self.table.select(if self.visible.is_empty() {
            None
        } else {
            Some(position.unwrap_or(0))
        });
    }

    pub fn scroll(&mut self, delta: isize) {
        self.follow_selection = true;
        if self.visible.is_empty() {
            self.table.select(None);
            return;
        }
        let position = self
            .table
            .selected()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(self.visible.len() - 1);
        self.table.select(Some(position));
    }

    pub fn processes(&self) -> impl Iterator<Item = &Process> {
        self.visible.iter().map(|i| &self.snapshot.processes[*i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn process(pid: u32, cpu: f32, name: &str) -> Process {
        Process {
            pid,
            cpu: Some(cpu),
            name: name.into(),
            memory: Some(0),
            status: "Run".into(),
            ..Default::default()
        }
    }
    #[test]
    fn default_view_stays_at_top_when_cpu_ranking_changes() {
        let mut app = App::new(1000);
        app.update(Snapshot {
            processes: vec![process(1, 50.0, "Alpha"), process(2, 5.0, "Beta")],
            ..Default::default()
        });
        app.update(Snapshot {
            processes: vec![process(1, 5.0, "Alpha"), process(2, 80.0, "Beta")],
            ..Default::default()
        });
        assert_eq!(app.table.selected(), Some(0));
        assert_eq!(app.processes().next().unwrap().pid, 2);
    }
    #[test]
    fn selection_tracks_pid_across_samples_and_search() {
        let mut app = App::new(1000);
        app.update(Snapshot {
            processes: vec![process(1, 5.0, "Alpha"), process(2, 80.0, "Beta")],
            ..Default::default()
        });
        assert_eq!(app.processes().next().unwrap().pid, 2);
        app.scroll(0);
        app.update(Snapshot {
            processes: vec![process(2, 1.0, "Beta"), process(1, 90.0, "Alpha")],
            ..Default::default()
        });
        assert_eq!(app.table.selected(), Some(1));
        app.query = "ALP".into();
        app.refresh_view();
        assert_eq!(app.visible.len(), 1);
        assert_eq!(app.processes().next().unwrap().pid, 1);
        app.query = "missing".into();
        app.refresh_view();
        app.scroll(100);
        assert_eq!(app.table.selected(), None);
    }
}
