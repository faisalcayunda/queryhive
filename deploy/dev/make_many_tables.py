#!/usr/bin/env python3
"""Create 5,000 small tables in schema `bench_many` for the sidebar/schema-browser benchmarks.

    python3 deploy/dev/make_many_tables.py            # PostgreSQL (qh-postgres)
    python3 deploy/dev/make_many_tables.py --mysql    # also MySQL (qh-mysql); database bench_many
    python3 deploy/dev/make_many_tables.py --drop     # remove the schema first (or only, with --drop-only)

Idempotent: tables are `CREATE TABLE IF NOT EXISTS`. Runs psql/mysql inside the
containers via `podman exec`, with the throwaway fixture credentials from up.sh.
"""
import argparse
import subprocess

N = 5000


def run(cmd: list[str], sql: str) -> None:
    subprocess.run(["podman", "exec", "-i", *cmd], input=sql, text=True, check=True, stdout=subprocess.DEVNULL)


def pg(sql: str) -> None:
    run(["qh-postgres", "psql", "-q", "-v", "ON_ERROR_STOP=1", "-U", "qh", "-d", "qh"],
        "SET client_min_messages = warning;\n" + sql)


def my(sql: str) -> None:
    run(["qh-mysql", "mysql", "-uroot", "-pqh-dev-root-only"], sql)


def tables(fmt: str) -> str:
    return "".join(fmt.format(n=f"t{i:04d}") for i in range(1, N + 1))


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--mysql", action="store_true", help="also create the tables in MySQL")
    ap.add_argument("--drop", action="store_true", help="drop the schema first, then recreate")
    ap.add_argument("--drop-only", action="store_true", help="drop the schema and stop")
    a = ap.parse_args()
    drop = a.drop or a.drop_only
    if drop:
        # DROP SCHEMA ... CASCADE on 5,000 tables overruns max_locks_per_transaction,
        # so drop the tables in batches (one transaction each) first.
        for i in range(1, N + 1, 500):
            names = ", ".join(f"bench_many.t{j:04d}" for j in range(i, min(i + 500, N + 1)))
            pg(f"DROP TABLE IF EXISTS {names};")
        pg("DROP SCHEMA IF EXISTS bench_many CASCADE;")
    if not a.drop_only:
        pg("CREATE SCHEMA IF NOT EXISTS bench_many;\n" + tables(
            "CREATE TABLE IF NOT EXISTS bench_many.{n} (id integer PRIMARY KEY, label text, amount numeric(10,2));\n"))
    if a.mysql:
        if drop:
            my("DROP DATABASE IF EXISTS bench_many;")
        if not a.drop_only:
            my("CREATE DATABASE IF NOT EXISTS bench_many;\nUSE bench_many;\n" + tables(
                "CREATE TABLE IF NOT EXISTS {n} (id int PRIMARY KEY, label text, amount decimal(10,2));\n")
               + "GRANT ALL ON bench_many.* TO 'qh'@'%';\n")
    print("done" + (" (MySQL bench_many left in place: pass --mysql to drop it too)" if drop and not a.mysql else ""))
