# Accuracy check

Checked on the local Apple M1 Mac on 2026-10-09. The final release binary was rebuilt.

## Current behavior

- macOS CPU meters use checked native per-core Mach tick deltas. Failed reads, empty intervals, counter resets, and warm-up samples are unavailable, not fabricated zeroes. Valid idle intervals remain 0%.
- Process CPU uses checked task-counter differences divided by elapsed Mach ticks, with PID identity validation.
- macOS process MEM, MEM%, and memory sorting use physical footprint from `proc_pid_rusage`, the metric used by Activity Monitor. JSON keeps resident memory separately. Failed footprint queries do not fall back to resident memory.
- System used RAM is total physical RAM minus free pages and cached files. Cached files include file-backed and purgeable pages. Speculative pages are already part of free pages. This counts memory outside the named App/Wired/Compressed page categories; JSON exposes that remainder as `other_bytes` when nonnegative.
- RAM statistics accept older macOS structure revisions only when every field used is present. Failed or inconsistent queries remain null/N/A. Load averages use checked `getloadavg`.
- Charts show the latest available history; a failed CPU or RAM reading clears its graph rather than extending a stale history.

## Verification

- Formatting, Clippy with warnings denied, all 16 unit tests, release build, and whitespace checks passed.
- Activity Monitor was opened and its actual Memory view inspected. No Activity Monitor settings were changed.
- WallpaperImageExtension (PID 627): mtr footprint 130.8 MiB; Activity Monitor 130.8 MB. The same mtr sample had only 5.7 MiB resident RAM, demonstrating why resident memory was not an appropriate comparison.
- Chrome (PID 30734): mtr footprint 413.7 MiB; Activity Monitor showed 412.6 MB in the nearby observation. These were independently timed samples.
- Final used RAM: mtr 7.021 GiB; Activity Monitor 7.03 GB shortly afterward. This is a sampled comparison, not a guarantee of exact synchronization. The final sandbox sample correctly reported inaccessible swap as null.
- An unrestricted sample exposed 52 real temperature sensors and swap usage of 568.1 MiB, matching Activity Monitor's 568.1 MB in that observation.
- Earlier controlled process-counter checks measured a busy worker at 90–98% of one core, a sleeping process at 0%, and resident memory matching `ps`. GPU driver fields, load, uptime, total RAM, and swap were also checked in the earlier pass.

No calibration offset or invented value is applied to match Activity Monitor. Its polling window differs from mtr's. Temperature accuracy remains dependent on hardware calibration; Activity Monitor provides no Celsius sensor reference. GPU utilization is the driver-reported metric. Linux and Intel Mac runtime behavior have not been verified locally.

## References

- [Apple: physical footprint, Activity Monitor, and proc_pid_rusage](https://developer.apple.com/videos/play/wwdc2022/10106/)
- [Apple VM statistics fields](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/vm_statistics.h)
- [Activity Monitor update frequency](https://support.apple.com/en-ie/guide/activity-monitor/actmntr2224/mac)
