import Foundation

let usage = """
qhbench: black-box benchmark harness, drives QueryHive or TablePro the same way.

  qhbench --check-permissions
  qhbench ttfr   [--scenario ttfr-s1-1k] [--runs 20]
  qhbench type   [--scenario type-10k|type-2m] [--chars 40] [--min-changed 200]
  qhbench scroll [--scenario scroll-30x1m|scroll-500x10k] [--seconds 5] [--speed 4800]
  qhbench cancel [--scenario cancel-pg-sleep|cancel-mysql-sleep|cancel-stream-wide|cancel-trino-heavy] [--runs 10]
  qhbench launch [--scenario launch-warm|launch-cold] [--runs 10] [--quit] [--path APP.app]
  qhbench memory [--scenario mem-500k|mem-5m] [--duration 20] [--no-run] [--budget-bytes N]

Common: --app queryhive|tablepro  --pid N  --label TEXT  --profiles-dir DIR
Output: one JSON sample per line, the input contract of deploy/dev/bench_app.py --source qhbench.
"""

let argv = Array(CommandLine.arguments.dropFirst())
guard let first = argv.first else {
    logErr(usage)
    exit(2)
}
let rest = Args(Array(argv.dropFirst()))

switch first {
case "--check-permissions", "check-permissions":
    cmdCheckPermissions()
case "ttfr": await cmdTtfr(rest)
case "type": await cmdType(rest)
case "scroll": await cmdScroll(rest)
case "cancel": await cmdCancel(rest)
case "launch": await cmdLaunch(rest)
case "memory": await cmdMemory(rest)
case "-h", "--help", "help":
    print(usage)
default:
    logErr("unknown subcommand \(first)\n" + usage)
    exit(2)
}
exit(0)
