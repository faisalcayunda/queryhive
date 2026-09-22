#!/usr/bin/env python3
"""Search the web through kenari's server tools.

    python3 tools/kenari_search.py search "Trino query cancellation"
    python3 tools/kenari_search.py fetch https://trino.io/docs/current/client/client-protocol.html
    python3 tools/kenari_search.py x "Trino" --from-date 2026-01-01

Why this exists
---------------
`kenari:web_search` is a *server* tool: it runs on kenari's side and is normally
reached by declaring it in a chat request's `tools` array. That is no use to a
script or to an agent that wants results as data, so this calls the same
endpoints directly. The endpoints are the ones documented at
https://kenari.id/docs/server-tools:

    POST /v1/web/search   {"query": ..., "max_results": N}
    POST /v1/web/fetch    {"url": ...}
    POST /v1/x/search     {"query": ..., "x_search_filter": {...}}

The key
-------
Read from `KENARI_API_KEY`, falling back to a `.env` file at the repository root.
Both are outside version control: `.gitignore` covers `.env` and `.env.*`. The key
is never printed, never logged, and never included in an error message -- an
error that echoes the credential is how it ends up in a bug report.

    printf 'KENARI_API_KEY=kn-...\n' > .env && chmod 600 .env

Billing, which is worth knowing before a loop calls this
-------------------------------------------------------
`search` and `fetch` are billed per call. On a subscription they draw on the
plan's daily quota and then roll into pay-as-you-go. `x` is billed per search
straight from the balance and is *not* part of the subscription quota, so it
answers `plan_limit_reached` when the balance is short. A failed call is not
billed.

Exit codes, so a caller can tell the failures apart without parsing prose:

    0  ok
    1  usage
    2  no key, or the key was rejected (401)
    3  the plan or balance is short (402 / plan_limit_reached) -- retrying will not help
    4  anything else, including a network failure
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

BASE_URL = "https://kenari.id/v1"
TIMEOUT_SECONDS = 60.0

EXIT_OK = 0
EXIT_USAGE = 1
EXIT_AUTH = 2
EXIT_BALANCE = 3
EXIT_OTHER = 4


def repository_root() -> Path:
    """The repository root, which is where `.env` lives."""
    return Path(__file__).resolve().parent.parent


def load_key() -> str | None:
    """The API key, from the environment first and `.env` second.

    A real environment variable wins so that a shell export can override the file
    without editing it -- which is what CI does.
    """
    from_environment = os.environ.get("KENARI_API_KEY")
    if from_environment:
        return from_environment.strip()

    env_file = repository_root() / ".env"
    if not env_file.is_file():
        return None
    for line in env_file.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, _, value = line.partition("=")
        if name.strip() == "KENARI_API_KEY":
            return value.strip().strip("'\"") or None
    return None


class KenariError(Exception):
    """A call that failed, carrying the exit code the caller should use."""

    def __init__(self, message: str, exit_code: int) -> None:
        super().__init__(message)
        self.exit_code = exit_code


def call(path: str, payload: dict, key: str) -> dict:
    """POST one request and return the decoded JSON.

    Nothing here includes `key` in a message. The credential must not be able to
    reach a terminal, a log, or a pasted error report.
    """
    request = urllib.request.Request(
        f"{BASE_URL}/{path}",
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )

    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
            body = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        # The body's `code` is more precise than the HTTP status, and the two
        # genuinely disagree: a balance problem arrives as **429** with
        # `plan_limit_reached`, which looks like a rate limit and is not one. A
        # caller that classified by status alone would retry a billing limit
        # forever, so the body is read first and the status is the fallback.
        parsed = parse_json(body)
        error_body = parsed.get("error") if isinstance(parsed, dict) else None
        if error_body:
            raise KenariError(
                explain_body_error(error_body), exit_code_for(error.code, error_body)
            ) from None
        raise KenariError(explain_http_error(error.code, body), exit_code_for(error.code)) from None
    except urllib.error.URLError as error:
        raise KenariError(f"could not reach {BASE_URL}: {error.reason}", EXIT_OTHER) from None
    except TimeoutError:
        raise KenariError(
            f"{BASE_URL}/{path} did not answer within {TIMEOUT_SECONDS:.0f}s", EXIT_OTHER
        ) from None

    decoded = parse_json(body)
    if decoded is None:
        raise KenariError(
            f"{BASE_URL}/{path} returned something that is not JSON "
            f"({len(body)} bytes, starting {body[:80]!r})",
            EXIT_OTHER,
        )
    if not isinstance(decoded, dict):
        raise KenariError(
            f"{BASE_URL}/{path} returned a {type(decoded).__name__}, not an object",
            EXIT_OTHER,
        )

    # Some failures arrive with HTTP 200 and an `error` body, so this is checked
    # on the success path too.
    if decoded.get("error"):
        raise KenariError(
            explain_body_error(decoded["error"]), exit_code_for(200, decoded["error"])
        )

    return decoded


def parse_json(body: str) -> object | None:
    """Decode JSON, or `None` when the body is not JSON at all."""
    try:
        return json.loads(body)
    except json.JSONDecodeError:
        return None


def explain_http_error(status: int, body: str) -> str:
    """A message that says what to do, without echoing anything sensitive."""
    detail = body.strip()[:200] if body.strip() else "(no body)"
    if status == 401:
        return (
            "kenari rejected the key (HTTP 401). Check that KENARI_API_KEY is set and "
            f"still valid. Response: {detail}"
        )
    if status == 402:
        return (
            "the plan or balance is short (HTTP 402). This is a billing limit, not a "
            f"temporary error, so retrying will not help. Response: {detail}"
        )
    return f"kenari returned HTTP {status}: {detail}"


BALANCE_CODES = {
    "plan_limit_reached",
    "insufficient_balance",
    "quota_exceeded",
    "balance_exhausted",
}


def exit_code_for(status: int, body_error: object = None) -> int:
    """Which code a caller should see for this failure.

    The structured `code` is consulted **before** the HTTP status, because the two
    disagree in practice: `x/search` reports a balance shortfall as HTTP 429 with
    `plan_limit_reached`. Classifying that as a rate limit would invite a retry
    loop against a billing limit, which no amount of waiting fixes.
    """
    if isinstance(body_error, dict) and str(body_error.get("code", "")) in BALANCE_CODES:
        return EXIT_BALANCE
    if status == 401:
        return EXIT_AUTH
    if status == 402:
        return EXIT_BALANCE
    return EXIT_OTHER


def explain_body_error(error: object) -> str:
    if not isinstance(error, dict):
        return f"kenari reported an error: {error}"
    code = error.get("code", "unknown")
    message = error.get("message", "")
    hint = ""
    if code == "plan_limit_reached":
        # `x_search` is billed from the balance and is not part of the
        # subscription quota, so this is the expected answer when the balance is
        # low -- worth saying, because it reads like a bug otherwise.
        hint = " (x_search is billed from the balance, not the plan quota)"
    return f"kenari reported {code}: {message}{hint}"


def print_search(payload: dict, as_json: bool) -> None:
    if as_json:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
        return
    results = payload.get("results") or []
    if not results:
        print("no results")
        return
    for index, result in enumerate(results, start=1):
        print(f"{index}. {result.get('title', '(untitled)')}")
        print(f"   {result.get('url', '')}")
        content = (result.get("content") or "").strip()
        if content:
            print()
            print(content)
        print()


def print_fetch(payload: dict, as_json: bool) -> None:
    if as_json:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
        return
    title = (payload.get("title") or "").strip()
    if title:
        print(title)
        print()
    print((payload.get("content") or "").strip())


def print_answer(payload: dict, as_json: bool) -> None:
    if as_json:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
        return
    print((payload.get("answer") or "").strip())
    citations = payload.get("citations") or []
    if citations:
        print()
        for citation in citations:
            print(f"- {citation}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Search the web through kenari's server tools.",
        epilog="The key comes from KENARI_API_KEY, or from .env at the repository root.",
    )
    parser.add_argument("--json", action="store_true", help="print the raw response instead of text")
    subcommands = parser.add_subparsers(dest="command", required=True)

    search = subcommands.add_parser("search", help="search the web")
    search.add_argument("query")
    search.add_argument("--max-results", type=int, default=5)

    fetch = subcommands.add_parser("fetch", help="fetch one page as markdown")
    fetch.add_argument("url")

    x_search = subcommands.add_parser("x", help="search X (billed from the balance, not the plan)")
    x_search.add_argument("query")
    x_search.add_argument("--from-date", help="YYYY-MM-DD")
    x_search.add_argument("--to-date", help="YYYY-MM-DD")
    x_search.add_argument("--include-handle", action="append", default=[], help="repeatable")
    x_search.add_argument("--exclude-handle", action="append", default=[], help="repeatable")

    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)

    key = load_key()
    if not key:
        print(
            "no kenari key. Set KENARI_API_KEY, or put it in .env at the repository root:\n"
            "  printf 'KENARI_API_KEY=kn-...\\n' > .env && chmod 600 .env",
            file=sys.stderr,
        )
        return EXIT_AUTH

    try:
        if args.command == "search":
            if args.max_results < 1:
                print("--max-results must be at least 1", file=sys.stderr)
                return EXIT_USAGE
            payload = call("web/search", {"query": args.query, "max_results": args.max_results}, key)
            print_search(payload, args.json)
        elif args.command == "fetch":
            payload = call("web/fetch", {"url": args.url}, key)
            print_fetch(payload, args.json)
        elif args.command == "x":
            if args.include_handle and args.exclude_handle:
                # The API refuses both together, so it is caught here rather than
                # spending a billed call to be told so.
                print("--include-handle and --exclude-handle cannot be combined", file=sys.stderr)
                return EXIT_USAGE
            filters: dict[str, object] = {}
            if args.from_date:
                filters["from_date"] = args.from_date
            if args.to_date:
                filters["to_date"] = args.to_date
            if args.include_handle:
                filters["allowed_x_handles"] = args.include_handle[:20]
            if args.exclude_handle:
                filters["excluded_x_handles"] = args.exclude_handle[:20]
            payload = {"query": args.query}
            if filters:
                payload["x_search_filter"] = filters
            print_answer(call("x/search", payload, key), args.json)
        else:  # pragma: no cover - argparse enforces the choices
            return EXIT_USAGE
    except KenariError as error:
        print(str(error), file=sys.stderr)
        return error.exit_code

    return EXIT_OK


if __name__ == "__main__":
    sys.exit(run(sys.argv[1:]))
