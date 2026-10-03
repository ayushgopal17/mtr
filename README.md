# mtr

A compact Rust terminal dashboard for CPU, GPU, RAM, swap, cache, and processes.
An htop-style interface: two columns of CPU meters, compact memory/GPU/cache
readings, a dense process table, and a function-key command bar.
Built with [Ratatui](https://ratatui.rs/) and [sysinfo](https://docs.rs/sysinfo/0.33.1/sysinfo/).

**Verification:** compiled and tested on an Apple M1 Mac running macOS 14.8.3
(Darwin 23.6), with Rust 1.99. The release build, seven unit tests, formatting, and
Clippy passed. Live CPU, GPU, and memory readings and terminal rendering were
checked. Linux and Intel Mac execution have not yet been tested locally. Restricted
OS queries show `N/A`; normal Terminal execution can expose more process data
than a sandboxed session.

## Run

Install a current stable [Rust toolchain](https://rustup.rs/), then:

```sh
cargo run --release
```

Install the command:

```sh
cargo install --path . --locked
mtr
mtr --interval 500
```

The name `mtr` is also used by a network diagnostic utility. If that is already
installed, run `./target/release/mtr` explicitly or use a shell alias such as
`alias pulse="$PWD/target/release/mtr"` from this project directory.

Cargo installs into `~/.cargo/bin` by default. Rustup configures this on your PATH;
open a fresh Terminal or run `source "$HOME/.cargo/env"` if needed. After
installation, `mtr` runs from any directory. Confirm with `type -a mtr`.

## Share with another Mac (no Rust required)

Build a release archive on your Mac:

```sh
bash scripts/package-macos.sh
```

On Apple Silicon this creates `dist/mtr-0.2.0-macos-arm64.tar.gz` and a SHA-256
checksum file. Share both with a friend using an M-series Mac running macOS 12
or newer. On an Intel Mac the same script makes an Intel archive.

To produce one binary for both Intel and Apple Silicon:

```sh
bash scripts/package-macos.sh --universal
```

The universal option downloads the additional Rust target through rustup, builds
both architectures, and combines them with Apple's `lipo`. It requires Apple's
Command Line Tools (`xcode-select --install` if missing). The resulting archive is
`dist/mtr-0.2.0-macos-universal.tar.gz`. Building both architectures does not replace
testing on both types of Mac.

Your friend extracts the archive, opens Terminal in the extracted folder, and runs:

```sh
bash install.sh
mtr
```

The installer copies the executable to `~/.local/bin`, without requiring Rust or
sudo. If that directory is not on PATH, it prints the exact line to add to
`~/.zshrc`; then open a new Terminal. It refuses to replace an existing `mtr`
unless called with `--force`. To uninstall, remove `~/.local/bin/mtr`.

Archives are locally ad-hoc signed, not Developer ID signed or notarized. macOS
may ask your friend to approve a downloaded executable; follow Apple's
[instructions for software from an unidentified developer](https://support.apple.com/102445)
only for a copy they trust. Do not disable Gatekeeper globally. Public releases
should use Developer ID signing and notarization for a smoother install.

Alternatively, share this entire source folder. A friend with Rust can run
`cargo install --path . --locked` to build for their own machine. A macOS binary
does not run on Linux; Linux needs its own build.

Use a UTF-8 terminal. Standard ANSI colors work in macOS Terminal; true color
is not required. Recommended size: 110 × 32 or larger; minimum: 60 × 16. No root
privileges are needed. On machines with many cores, `[` and `]` page through CPU
meters. JSON includes every core. Narrow terminals hide VIRT and ELAPSED columns.

## Controls

| Key | Action |
| --- | --- |
| `F1`, `?` | Help and metric definitions |
| `F2`, `p` | Toggle executable paths / process names |
| `F3`, `/` | Filter by name, path, username, or PID |
| `F4`, `Esc` | Clear filter |
| `F5`, `Space` | Pause/resume display |
| `F6`, `s`, `Tab` | Cycle sort column |
| `F7`, `c` | Sort CPU |
| `F8`, `m` | Sort resident memory |
| `F9`, `r` | Reverse sort |
| `F10`, `q`, `Ctrl+C` | Quit |
| `↑` / `↓`, `j` / `k` | Move selection |
| `PgUp` / `PgDn`, `Home` / `End` | Page / first / last |
| `[` / `]` | Previous / next CPU meter page |

On Mac, hold **Fn** if the function keys control brightness or volume. Letter
shortcuts work without Fn. The default view stays at the top of the ranking;
after you navigate, selection follows that process across refreshes. Sorting or
filtering resets selection to the first result.

The process table shows PID, USER, VIRT, RES, state, CPU%, MEM%, ELAPSED, and
Command. **ELAPSED is wall time since process start, not accumulated CPU time.**
VIRT includes reserved address space. `R` means runnable; it does not imply the
process is currently executing. CPU meter colors indicate total load, not a
user/kernel breakdown. F-key labels reflect the implemented actions; this is a
monitor, without process termination or priority-changing commands.

## Measurements

* **CPU:** system counter deltas via sysinfo, with a warm-up interval before the
  first reading. Aggregate and each logical core range from 0–100%. Process CPU
  uses one-core units and can exceed 100% for multithreaded workloads.
* **Memory:** OS-reported used, available, resident process memory, and swap.
  Resident memory is not unique ownership: shared pages can be counted in several
  processes. Some protected processes may expose limited information.
* **macOS cache:** native Mach VM statistics, `external_page_count × page size`
  (file-backed pages). Compressed memory is the physical compressor footprint.
  File-backed pages are not exactly the same as Activity Monitor's Cached Files.
* **Linux cache:** `/proc/meminfo`, `Cached + SReclaimable − Shmem`, saturated at zero.
  Cache overlaps existing memory accounting; do not add it to used RAM.
* **CPU caches:** hardware L1/L2/L3 capacities, read once. These are not cache
  occupancy, misses, or hit rates. macOS uses `sysctl`; Linux reports the cache
  hierarchy for CPU 0. Heterogeneous cores can have different cache sizes.
* **GPU:** driver-reported utilization and memory; not an estimate based on CPU
  activity. Missing readings are `N/A` / JSON `null`.

| Platform | GPU source | Notes |
| --- | --- | --- |
| macOS | Native IOKit / IORegistry | Apple GPU activity and shared memory where exposed; no sudo or polling subprocesses |
| Linux AMD | DRM sysfs | Busy percentage, VRAM, temperature when the driver exposes them |
| Linux NVIDIA | `nvidia-smi` | Requires NVIDIA's utility on PATH; 700 ms command timeout |
| Linux Intel | DRM sysfs | Device shown; utilization may be unavailable |

IORegistry performance fields are driver-dependent and may change across macOS
versions. Unified GPU memory is part of system RAM, not extra VRAM. GPU readings
use the driver's own sampling window and may differ from CPU's sampling window.

## Performance and diagnostics

The default interval is 1 second (configurable from 250 ms to 60 seconds). A
background collector publishes only the latest snapshot. The UI polls input at
50 ms and redraws only when data or input changes. History is bounded to 120
samples. Process refresh requests CPU and memory each interval; executable paths and
user IDs are fetched once per process. Hardware and user-name metadata are
collected at startup. NVIDIA collection starts one bounded subprocess per
sample; native macOS and Linux AMD paths do not spawn commands.

```sh
mtr --json                   # one real snapshot, after CPU warm-up
mtr --benchmark 10           # collection latency on your machine
mtr --interval 500 --benchmark 20
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The header reports collection wall time; it is not the monitor's CPU overhead.
Measure actual overhead using Activity Monitor or `top` on your machine. No
performance numbers are claimed before benchmarking. JSON is a diagnostic format
and its fields may change before version 1.0.

## Development

`src/metrics.rs` owns sampling and history; `src/platform/` contains platform
collectors; `src/app.rs` handles filtering and sorting; `src/ui.rs` draws the
dashboard. CI checks macOS and Linux builds, tests, and Clippy.

MIT licensed.
