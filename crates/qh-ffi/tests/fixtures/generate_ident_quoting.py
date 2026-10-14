#!/usr/bin/env python3
"""Regenerates ident_quoting.json from the app's reserved-word list.

    python3 -I crates/qh-ffi/tests/fixtures/generate_ident_quoting.py

The expectations are computed here, by an implementation of the rule that shares no code
with the Rust one, from the same words the Swift side uses
(app/Sources/QueryHive/Models/SQLIdentifierQuoting.swift):

* a plain name is [a-z_][a-z0-9_]* and not a reserved word, and it is written bare;
* everything else is quoted, with the dialect's own quote character doubled inside the
  name: a double quote for PostgreSQL and Trino, a backtick for MySQL.

Both sides read the file: crates/qh-ffi/tests/transfer_types.rs (which also reads the Swift
source itself, so a list that drifts fails there) and, when the Swift side adopts it, the
app's tests. The Swift file's git blob is recorded so a reviewer can see which version of the
list this was made from.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[4]
swift = root / "app/Sources/QueryHive/Models/SQLIdentifierQuoting.swift"
source = swift.read_text(encoding="utf-8")
body = source.split("static let reserved: Set<String> = [", 1)[1].split("]", 1)[0]
words = re.findall(r'"([^"]+)"', body)
assert len(words) == len(set(words)), "the Swift list repeats a word"
words.sort()

blob = subprocess.run(
    ["git", "hash-object", str(swift)], capture_output=True, text=True, cwd=root
).stdout.strip()

plain = re.compile(r"^[a-z_][a-z0-9_]*$")


def write(name, mark):
    if plain.match(name) and name not in set(words):
        return name
    return mark + name.replace(mark, mark + mark) + mark


names = list(words)
names += [
    # plain, and words that are only non-reserved or merely look reserved
    "id", "_x", "a1", "order_total", "name", "status", "date", "value", "type",
    "selects", "xselect", "user_id", "_", "a_b_c",
    # not plain: case, a leading digit, symbols, spaces, non-ASCII, the empty name
    "Name", "ID", "SELECT", "Select", "1st", "9", "a b", "a-b", "a.b", "a;b", "a,b",
    "trailing ", " leading", "é", "日本語", "", "count(*)", "a\u0000b",
    # a quote of either kind, a newline, and names that try to close their own quotes
    'a"b', "a`b", 'a""b', "a``b", "a\nb", "a\tb", "a\\b", "a'b",
    'x"; DROP TABLE y; --', "x`; DROP TABLE y; --", '"', "`", '""', "``",
]
cases = [
    {"name": name, "postgres": write(name, '"'), "mysql": write(name, "`")}
    for name in names
]
out = {
    "schema": 1,
    "generated_from": "app/Sources/QueryHive/Models/SQLIdentifierQuoting.swift",
    "swift_blob": blob,
    "word_count": len(words),
    "reserved": words,
    "cases": cases,
}
target = Path(__file__).with_name("ident_quoting.json")
target.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
print(f"{len(words)} words, {len(cases)} cases, swift blob {blob}", file=sys.stderr)
