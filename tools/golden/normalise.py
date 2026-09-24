#!/usr/bin/env python3
"""The one normalisation both readers of `tests/golden/` compare through.

    import normalise

    normalise.add_roots(tempfile.gettempdir())
    lines = normalise.normalise(stdout)          # [(event, ...)] as JSON text
    problems = normalise.diff_lines(expected, lines, full=False)

Why this is a module of its own
-------------------------------
It used to live in `tools/golden/record.py`, next to the in-process recorder that
drove the Python engine with a fake cursor. That recorder is gone with the
engine, but the normalisation is not about the engine: it is the set of
decisions about which parts of an event are allowed to differ between two runs.
`live_cases.py` needs exactly those decisions to keep comparing snapshots that
were recorded against real servers, so they moved here rather than going with
the recorder.

What is deliberately not smoothed over
--------------------------------------
Only the keys in `VOLATILE_KEYS`, the temp paths `add_roots` was told about, and
the object identifiers `_mask_identity_oids` can recognise. Everything else has
to match exactly: an unclassified difference has to fail the comparison rather
than be normalised away, which is the whole reason a golden corpus is worth
having.

The snapshots in `tests/golden/` were recorded while the Python engine was still
in this tree, so these rules are also what makes them readable as a record of
that engine rather than only of the current one.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
GOLDEN_DIR = ROOT / "tests" / "golden"

# --------------------------------------------------------------------------- #
# normalisation
# --------------------------------------------------------------------------- #

# Keys whose value cannot be the same twice. Everything else must match exactly:
# an unclassified difference has to fail the comparison, not be smoothed over.
VOLATILE_KEYS = {
    "elapsed_ms": "<TIME>",
    "query_id": "<QUERY_ID>",
}

# The first object identifier a PostgreSQL server hands out to an object *it* did
# not create: system catalogues get 1..16383 (`pg_class`'s `FirstNormalObjectId`),
# anything a client makes starts at 16384 and climbs one per object *per cluster*.
# That is the reason this exists. `deploy/dev/up.sh` drops and recreates its
# containers on every start and replays the seed, so the counter keeps rising and
# a snapshot that froze the number starts failing the moment anyone re-seeds --
# not because the engine changed, but because the server had made more objects in
# the meantime. The identifier is the server's bookkeeping, not the engine's
# answer. Measured on the live fixture cluster (23 Sep 2026): the highest system
# OID was 13665 and the lowest user OID 32820, so the floor separates the two.
USER_OID_FLOOR = 16384


def _is_user_oid(value: Any) -> bool:
    """True for a bare decimal string at or above the first user-assigned OID.

    Digits and nothing else: the drivers render an OID as text, and requiring the
    digits means a value that merely looks numeric (`"0x4000"`, `"16384.0"`, a row
    count that happens to be large) is never mistaken for one. `isascii` first,
    because `str.isdigit()` is true of digits `int()` then refuses (a superscript,
    an Arabic-Indic numeral), and this must not raise on a value a server sent.

    At most ten digits, because an OID is PostgreSQL's 4-byte unsigned integer
    (`pg_class.oid`, `description.type_code`): a longer run of digits is data that
    happens to be numeric, and masking it would hide a real answer. The floor is
    what keeps the mask narrow -- a `count` result, a row number or a timestamp
    above 16384 is likewise a real answer and stays in the snapshot untouched.
    """
    if not isinstance(value, str) or not value.isascii() or not value.isdigit():
        return False
    return 1 <= len(value) <= 10 and int(value) >= USER_OID_FLOOR


def _mask_identity_oids(event: dict) -> dict:
    """Replace the OIDs a server assigned with a token, in the two places they appear.

    Deliberately narrow, because everywhere else a five-digit number is data:

    * `columns[*].type` -- the type OID the driver reports as a string. This is the
      one that moves for the type zoo: the seeded `mood` enum and the `type_zoo`
      table's row type are recreated by every seed.
    * `data[*][i]`, but only when `object_columns[i]` is `"OID"` -- the object
      browser's own column list says which cell holds an identifier, so no other
      driver's rows (`MySQL`'s "Engine"/"Rows", Trino's "Type"/"Name") are touched.
      The column *list* is left alone: it is part of the protocol, and freezing it
      is the point.

    Called after the per-key pass, because both decisions need a sibling key
    (`columns`, `object_columns`) rather than the value on its own.
    """
    columns = event.get("columns")
    if isinstance(columns, list):
        for column in columns:
            if isinstance(column, dict) and _is_user_oid(column.get("type")):
                column["type"] = "<OID>"
    names = event.get("object_columns")
    rows = event.get("data")
    if isinstance(names, list) and isinstance(rows, list):
        for index, name in enumerate(names):
            if name != "OID":
                continue
            for row in rows:
                if isinstance(row, list) and index < len(row) and _is_user_oid(row[index]):
                    row[index] = "<OID>"
    return event

# A real tmp path differs on every run and would make every diff noisy.
PLACEHOLDERS = ("<TMP>",)


def _normalise_value(key: str, value: Any) -> Any:
    if key in VOLATILE_KEYS:
        return VOLATILE_KEYS[key]
    if key == "files" and isinstance(value, list):
        return [{**entry, "path": _normalise_path(entry.get("path", ""))} for entry in value]
    if isinstance(value, str):
        return _normalise_path(value)
    if isinstance(value, list):
        return [_normalise_value(key, item) for item in value]
    return value


def _normalise_path(text: str) -> str:
    """Replace any tmp path with one token, longest prefix first."""
    for raw in sorted(_TMP_ROOTS, key=len, reverse=True):
        if raw and raw in text:
            text = text.replace(raw, "<TMP>")
    return text


_TMP_ROOTS: set[str] = set()


def add_roots(*paths) -> None:
    """Mask one more root out of every string a case emits.

    A caller seeds this with the directory its `export` case writes into, or with
    the temp root, because the same normalisation has to apply to a run whether
    or not that path was chosen by this tool.
    """
    _TMP_ROOTS.update(str(path) for path in paths if path)


def normalise(stdout: str) -> list[str]:
    """One normalised JSON line per event, in the engine's own order."""
    lines: list[str] = []
    for line in stdout.splitlines():
        if not line.strip():
            continue
        event = json.loads(line)  # raises if the engine emitted a non-JSON line
        normalised = {key: _normalise_value(key, value) for key, value in event.items()}
        normalised = _mask_identity_oids(normalised)
        lines.append(json.dumps(normalised, ensure_ascii=False, sort_keys=True))
    return lines


# --------------------------------------------------------------------------- #
# compare
# --------------------------------------------------------------------------- #


def diff_lines(expected: list[str], actual: list[str], full: bool) -> list[str]:
    """Line differences between two normalised event streams, as sentences.

    Lives next to `normalise`, because every reader of a snapshot needs the same
    answer to "did it move". Two copies of this would drift, and the one that
    drifted would be the one nobody read.
    """
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


# --------------------------------------------------------------------------- #
# index
# --------------------------------------------------------------------------- #


def rebuild_index(destination: Path = GOLDEN_DIR) -> None:
    """Write index.json from every `.meta.json` in the tree.

    The index is a view of what is on disk rather than of what a run happened to
    produce, so a snapshot recorded against a real server and one recorded from a
    fake cursor share one index without either recorder knowing about the other's
    case list.
    """
    index = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(destination.glob("*/*.meta.json"))
    ]
    (destination / "index.json").write_text(
        json.dumps(sorted(index, key=lambda entry: entry["case"]), indent=2,
                   ensure_ascii=False, sort_keys=True) + "\n",
        encoding="utf-8",
    )
