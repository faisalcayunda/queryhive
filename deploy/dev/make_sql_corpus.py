#!/usr/bin/env python3
"""Write the SQL corpora the editor benchmarks open.

    python3 deploy/dev/make_sql_corpus.py [DIR]      # default target/bench-corpus/

Two files, deterministic (fixed seed), a realistic mix of multiple statements,
CTEs, line and block comments, single/double-quoted strings, dollar-quoted
strings, :name parameters and long lines:

    lines-10k.sql   10,000 lines, about 400k characters
    chars-2m.sql    2,000,000 characters
"""
from __future__ import annotations

import random
import sys
from pathlib import Path

LINES, CHARS = 10_000, 2_000_000
COLS = ["id", "user_id", "amount", "status", "created_at", "region", "note"]
TABLES = ["orders", "customers", "events", "payments", "sessions"]


def statement(r: random.Random, n: int) -> list[str]:
    t, c = r.choice(TABLES), r.sample(COLS, 3)
    kind = 4 if r.random() < 0.03 else r.choice([0, 1, 2, 3, 5, 6])  # 4 = the long line
    if kind == 0:
        return [f"-- statement {n}: recent {t}", f"SELECT {', '.join(c)}", f"  FROM {t}",
                f" WHERE {c[0]} > :min_id AND status = 'active'", " ORDER BY created_at DESC LIMIT 100;", ""]
    if kind == 1:
        return [f"WITH recent AS (", f"  SELECT {c[0]}, sum(amount) AS total /* running total */",
                f"    FROM {t} WHERE created_at >= :since GROUP BY {c[0]}", "), ranked AS (",
                "  SELECT *, rank() OVER (ORDER BY total DESC) AS rk FROM recent", ")",
                "SELECT * FROM ranked WHERE rk <= :top_n;", ""]
    if kind == 2:
        return [f"/* block comment {n}", "   spanning several lines, with 'quotes' and -- dashes inside */",
                f"UPDATE {t} SET note = 'it''s {n}', status = \"done\" WHERE {c[0]} = :id;", ""]
    if kind == 3:
        return [f"CREATE OR REPLACE FUNCTION f_{n}() RETURNS text AS $body$",
                f"BEGIN RETURN 'x' || '{n}'; -- not a real comment end; $$ inside is fine",
                "END; $body$ LANGUAGE plpgsql;", f"SELECT $tag${t} 'quoted' \"and\" -- text$tag$;", ""]
    if kind == 4:
        cols = ", ".join(f"{r.choice(COLS)}_{i} AS c{i}" for i in range(60))
        return [f"SELECT {cols} FROM {t} WHERE note LIKE '%{'x' * 120}%';", ""]
    if kind == 5:
        return [f"INSERT INTO {t} ({', '.join(c)})", f"VALUES ({r.randrange(10**6)}, 'a;b', {r.random():.4f}),",
                f"       ({r.randrange(10**6)}, 'c -- d', {r.random():.4f});", ""]
    return [f"SELECT count(*) FROM {t} WHERE {c[1]} IN (:a, :b, :c); -- trailing comment {n}", ""]


def build(limit_lines: int | None, limit_chars: int | None) -> str:
    r, out, n, size = random.Random(20260930), [], 0, 0
    while True:
        for line in statement(r, n):
            out.append(line)
            size += len(line) + 1
            if (limit_lines and len(out) >= limit_lines) or (limit_chars and size >= limit_chars):
                text = "\n".join(out) + "\n"
                return text[:limit_chars] if limit_chars else text
        n += 1


if __name__ == "__main__":
    d = Path(sys.argv[1] if len(sys.argv) > 1 else "target/bench-corpus")
    d.mkdir(parents=True, exist_ok=True)
    for name, text in (("lines-10k.sql", build(LINES, None)), ("chars-2m.sql", build(None, CHARS))):
        (d / name).write_text(text)
        print(f"{d / name}: {len(text.splitlines())} lines, {len(text)} chars")
