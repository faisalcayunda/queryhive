#!/usr/bin/env python3
"""Compare freshly recorded engine output against the golden snapshots.

    python3 tools/golden/compare.py           # exit 0 if nothing moved
    python3 tools/golden/compare.py --full    # print every diffed line, not a summary

This runs the same cases record.py does, in memory, and diffs the result against
tests/golden/. A difference is either a regression in the engine or a snapshot
that was recorded on purpose -- the two are told apart by reading
docs/golden-deltas.md, which is also where the Rust engine's accepted
differences are listed.

The comparison is exact on every key except the ones record.py normalises
(elapsed_ms, query_id, tmp paths). Anything else that moves is a failure: the
whole point of freezing the snapshots is that an unclassified difference has to
stop the build rather than be smoothed over.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import record  # noqa: E402  (same directory, imported as a script companion)

ROOT = record.ROOT
GOLDEN_DIR = record.GOLDEN_DIR

try:  # the live cases are recorded by a separate tool, against real servers
    import live_cases  # noqa: E402

    LIVE_IDS = set(live_cases.LIVE_IDS)
except Exception:  # pragma: no cover - only when the module is missing
    LIVE_IDS = set()


def load_recorded(destination: Path = GOLDEN_DIR) -> dict[str, list[str]]:
    """{case_id: lines} read back from disk."""
    if not destination.exists():
        return {}
    found: dict[str, list[str]] = {}
    for path in sorted(destination.glob("*/*.ndjson")):
        found[path.stem] = path.read_text(encoding="utf-8").splitlines()
    return found


def diff_lines(expected: list[str], actual: list[str], full: bool) -> list[str]:
    """Line differences, with the first differing JSON keys called out."""
    problems: list[str] = []
    if len(expected) != len(actual):
        problems.append(f"event count {len(expected)} -> {len(actual)}")
    for index, (want, got) in enumerate(zip(expected, actual)):
        if want == got:
            continue
        if full:
            problems.append(f"line {index + 1}:\n  golden: {want}\n  now:    {got}")
            continue
        try:
            want_event, got_event = json.loads(want), json.loads(got)
        except json.JSONDecodeError:
            problems.append(f"line {index + 1}: one side is not JSON")
            continue
        keys = sorted(set(want_event) | set(got_event))
        changed = [key for key in keys if want_event.get(key) != got_event.get(key)]
        problems.append(f"line {index + 1}: keys differ: {', '.join(changed)}")
    # Lines only on one side still have to be reported when the counts differ.
    for index in range(min(len(expected), len(actual)), max(len(expected), len(actual))):
        side = "golden" if index < len(expected) else "now"
        line = expected[index] if index < len(expected) else actual[index]
        problems.append(f"line {index + 1} ({side} only): {line}")
    return problems


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    full = "--full" in argv
    golden = load_recorded()
    if not golden:
        print(f"no snapshots in {GOLDEN_DIR.relative_to(ROOT)}; run tools/golden/record.py first")
        return 2

    fresh = record.collect()
    failed = 0
    for case_id in sorted(golden):
        if case_id not in fresh:
            if case_id in LIVE_IDS:
                # Recorded against a real server by tools/golden/live_cases.py:
                # this tool drives the engine with a fake cursor and cannot
                # re-derive it, so it is reported rather than counted as a
                # failure. Re-record it with that tool instead.
                print(f"live {case_id}: real-server snapshot, re-record with live_cases.py")
                continue
            print(f"MISSING case {case_id}: in the snapshot but no longer recorded")
            failed += 1
            continue
        problems = diff_lines(golden[case_id], fresh[case_id]["lines"], full)
        if problems:
            failed += 1
            print(f"DIFF {case_id}")
            for problem in problems:
                print(f"     {problem}")
        else:
            print(f"ok   {case_id}")

    for case_id in sorted(set(fresh) - set(golden)):
        print(f"NEW   {case_id}: recorded but not in the snapshot; re-record on purpose")

    live = len([case_id for case_id in golden if case_id not in fresh and case_id in LIVE_IDS])
    print(f"\n{len(golden) - failed - live}/{len(golden) - live} cases match"
          + (f" ({live} live cases not re-recorded here)" if live else ""))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
