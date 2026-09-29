# qhbench

Black-box benchmark harness (performance-plan.md section 4, item 0.5). A separate Swift package,
Apple frameworks only, not linked into the app. It drives QueryHive or TablePro the same way
(CGEvent input, Accessibility to find regions, ScreenCaptureKit to see pixels) and prints one JSON
sample per line, the input contract of `deploy/dev/bench_app.py --source qhbench`.

    swift build -c release --package-path tools/bench/qhbench
    Q=tools/bench/qhbench/.build/release/qhbench
    B="/usr/bin/python3 deploy/dev/bench_app.py --source qhbench"

## Safety rules the tool enforces

- **`--drive` is required** by every subcommand that activates an app or sends input (`ttfr`, `type`,
  `scroll`, `cancel`, `launch`, and `memory` unless `--no-run`). Without it the tool prints one
  `[belum diukur]` line saying it refused, exits 0, and touches nothing.
- **One target only.** It refuses when more than one copy of the profile's bundle id is running, unless
  `--pid N` (must belong to that bundle id) or `--path /path/App.app` picks one. There is no fallback by
  app name. The chosen pid, bundle path, window, grid and editor rectangles go to stderr and into every
  sample's notes before anything is sent.
- **Focus guard.** Before every posted event the tool checks the target is still the frontmost app and
  aborts the run if not. Do not touch keyboard or mouse during a run.
- **Scenario names are validated first**, before any app is looked at.
- Permissions are only read (`--check-permissions`), never requested. Without Screen Recording or
  Accessibility, every subcommand prints `"status":"tidak diukur (izin OS)"` and exits 0.
  `QHBENCH_FORCE_NO_PERMS=1` simulates that.

| Subcommand | Needs |
|---|---|
| ttfr, type, scroll | Screen Recording + Accessibility + `--drive` |
| launch | Screen Recording + `--drive` |
| cancel | Accessibility + `--drive` |
| memory | Accessibility + `--drive`; nothing with `--no-run` |

`scroll` also needs Developer Tools access for `xctrace record --attach`. If macOS has not been
allowed for Terminal under Privacy & Security > Developer Tools, or the target is not debuggable, the
trace fails and `hitch_ms_per_s` falls back to ScreenCaptureKit frame intervals (the note says so).

## Setup

- Set the keymap to the `queryhive` scheme in QueryHive. Cmd+R is Run and Cmd+. is Stop only under that
  scheme; under the DBeaver scheme Run is Cmd+Return and Stop is unbound (`Models/Shortcuts.swift`).
  `profiles/queryhive.json` is marked `"verified": false` until a live run confirms the regions.
- `profiles/tablepro.json` is UNVERIFIED (inferred from TablePro's source; Cmd+Return, Cmd+.,
  bundle id `com.TablePro`). Check shortcuts in its Settings.
- Open the app on the right connection, one query tab, the window on the display being measured.
- Corpus for the typing scenarios: `/usr/bin/python3 deploy/dev/make_sql_corpus.py` writes
  `target/bench-corpus/lines-10k.sql` (type-10k) and `target/bench-corpus/chars-2m.sql` (type-2m).
  Open the matching file in the editor tab before each `type` run.
- `cancel` credentials (throwaway fixtures from `deploy/dev/up.sh`):

      export QHBENCH_PG_PASSWORD="$(/usr/bin/sed -n 's/^DB_PASSWORD=//p' deploy/dev/up.sh)"
      export QHBENCH_MYSQL_ROOT_PASSWORD="$(/usr/bin/sed -n 's/^MYSQL_ROOT_PASSWORD=//p' deploy/dev/up.sh)"

  The MySQL password is passed to `podman exec` through its environment, not on the command line.

## Commands and what each one changes

`$APP` below is the one copy you target: add `--path /Applications/QueryHive.app` (or the TablePro
copy) and/or `--pid N`. Add `--app tablepro` for TablePro and `--competitor-rev <sha>` to `bench_app.py`.
`bench_app.py --dry-run` checks parsing without appending to `bench-results.jsonl`.

    $Q --check-permissions          # read-only

Axis 1, TTFR. Changes: the editor text is replaced each run with
`SELECT {n} AS qhbench_run, * FROM wide_500k` (`{n}` is the run number, so every run paints a new
result; override with `--sql`); Run is pressed N times.
`ttfr_ms` is the first frame that already shows the settled grid, `ttfr_first_change_ms` the first
frame that differs at all.

    $Q ttfr --drive $APP --scenario ttfr-s1-1k  --runs 20 | $B --repeat 20     # row cap 1,000 set in the app
    $Q ttfr --drive $APP --scenario ttfr-s1-10k --runs 20 | $B --repeat 20     # row cap 10,000
    $Q ttfr --drive $APP --scenario ttfr-s2-500k --runs 20 | $B --repeat 20    # row cap 500,000
    # S3: connection to 127.0.0.1:55435 (toxiproxy, 15 ms each way), tab on that connection
    $Q ttfr --drive $APP --scenario ttfr-s3-rtt30 --runs 20 | $B --repeat 20
    # S4: quit the app, start it, open the connection and tab, do nothing else, then one run
    $Q ttfr --drive $APP --scenario ttfr-s4-first-run --runs 1 | $B

Axis 3, memory. Changes: Run pressed once, then sampled for `--duration` seconds. Without Run
(`--no-run`) it is read-only and needs no permission.

    $Q memory --drive $APP --scenario mem-500k --duration 20 | $B
    $Q memory --drive $APP --scenario mem-5m   --duration 30 | $B

Axis 4, scroll. Changes: the mouse pointer is moved to the grid centre; wheel events at 4,800 px/s;
an `xctrace` trace is written under `$TMPDIR` and deleted afterwards.

    $Q scroll --drive $APP --scenario scroll-30x1m   --seconds 5 | $B
    $Q scroll --drive $APP --scenario scroll-500x10k --seconds 6 | $B

Axis 5, typing. Changes: 40 `M` characters are typed into the middle of the open document and left
there (undo them afterwards, or reopen the file). Open the corpus file first.

    $Q type --drive $APP --scenario type-10k --chars 40 | $B     # target/bench-corpus/lines-10k.sql
    $Q type --drive $APP --scenario type-2m  --chars 40 | $B     # target/bench-corpus/chars-2m.sql

Axis 6, cancel. Changes: the editor text is replaced each run with the scenario statement carrying a
`/* qhbench_cancel_<n> */` marker; Run and Stop pressed N times. Tab must be on the matching server.

    $Q cancel --drive $APP --scenario cancel-pg-sleep    --runs 10 | $B --repeat 10
    $Q cancel --drive $APP --scenario cancel-mysql-sleep --runs 10 | $B --repeat 10
    $Q cancel --drive $APP --scenario cancel-stream-wide --runs 10 | $B --repeat 10
    $Q cancel --drive $APP --scenario cancel-trino-heavy --runs 10 | $B --repeat 10

Axis 7, launch. **`--quit` terminates every running instance whose bundle is exactly the `--path`
bundle, and only those** (each pid is printed first); it needs `--path`. Without `--quit` the tool
refuses if any copy of the bundle id is running. It then runs `open` on the bundle N times.

    $Q launch --drive --path /Applications/QueryHive.app --quit --scenario launch-warm --runs 10 | $B --repeat 10
    sudo purge && $Q launch --drive --path /Applications/QueryHive.app --quit --scenario launch-cold --runs 1 | $B

## Head-to-head fairness (performance-plan section 2)

Run the two apps interleaved, not one after the other: alternate QueryHive, TablePro, QueryHive,
TablePro for each scenario (one `--runs 1` invocation per turn, or a shell loop over the two `--path`
values), so thermal state and background load are shared. Same display, same window size, same server,
same day, both apps with the same row cap.

## How each number is taken

- `ttfr`: the ScreenCaptureKit stream (120 fps) watches the grid region; Run goes through CGEvent; the
  frames after Run are kept, and after the settle time the last frame is taken as the settled result.
- `type`: caret bounds through `AXBoundsForRange`, a band to the right of the caret is captured, one
  `M` per iteration, key-down to the first changed frame. `input_to_photon_ms` (list). Caret blink is
  filtered only by `--min-changed` (default 200 sampled pixels); tune it if numbers look wrong.
- `scroll`: `hitch_ms_per_s` from the exported `hitches` table (layout unverified), else from frame
  intervals; `frame_p99_ms` always from ScreenCaptureKit.
- `cancel`: polls every 5 ms in lock-step (send, read reply, sleep the remainder): `pg_stat_activity`,
  `information_schema.processlist`, or Trino `/v1/query` (newest non-terminal query with the marker).
  `cancel_ms` is Stop to the first poll that no longer sees the statement.
- `launch`: first new on-screen window through `CGWindowListCopyWindowInfo` every 1 ms. Not proof of an
  interactive frame.
- `memory`: `phys_footprint` from `proc_pid_rusage` every 20 ms, peak minus the median of 10 idle samples.

## Known limits

- The frontmost check uses `NSWorkspace` after a zero-length run-loop turn in a CLI process; its
  freshness has not been verified on a live run.
- The grid is the largest `AXTable`/`AXOutline` at least half the window wide (a sidebar outline is
  skipped); otherwise a window fraction (noted in every sample). None of this is verified live.
