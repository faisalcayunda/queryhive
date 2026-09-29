# 0023 — MCP resources, template prompts, and refusing an unknown protocol revision

- **Status:** Accepted
- **Date:** 29 Sep 2026 (Phase 2)
- **Instruction context:** `docs/architecture/tablepro-source-study.md` §7 (the MCP section);
  `docs/mcp-stability.md`; `docs/decisions/0015-mcp-token-scope.md`

## Context

Phase 2 shipped a read-only MCP server with nine tools and a token that carries a list of tool
names and a list of connection ids (`crates/qh-ffi/src/mcp.rs`). Two parts of the protocol surface
were still missing, and one was a stated gap:

1. **Protocol negotiation.** `initialize` echoed the client's revision when the server knew it and
   otherwise answered with the server's own. That fallback is friendly but hides the mismatch: a
   client that asked for a revision nobody built cannot tell that from being understood. The
   stability page recorded this as not done.
2. **Resources and prompts.** `initialize` declared only `capabilities.tools`. A client that
   expected the other two capabilities found neither.

The binding constraint is ADR-0015: a token's scope is a **list of tool names**, and a connection
may only be named if it appears on the token's **allowlist**; an empty allowlist means no
connections. Resources and prompts must not invent a second authorisation model, and must not
become a side door around the one the tools already enforce.

## Options considered

| Decision | Option | Upside | Downside |
|---|---|---|---|
| Unknown revision | **Refuse with `-32022` and `data.supported`** | A mismatch is visible; the client learns what it could ask for | Breaks a client that relied on the silent downgrade |
| | Keep the silent fallback | Never errors | Hides the mismatch, which is exactly the recorded gap |
| Resource gating | **Backing tool's scope + allowlist** | One authorisation model; no second copy of either rule | A resource is unreachable unless the corresponding tool is in scope |
| | A new synthetic scope name | Independent of the tool list | Scope is tool names by ADR-0015; a non-tool scope needs a new issue rule |
| Resource set | **Connections, and per-connection objects/tables** | Covers the read-only data a client already reaches through tools | Listing is cheap; a read can touch the network |
| | Connections only | No network on `resources/read` | Misses the object tree the brief names as the second obvious resource |
| Refusal | **One `-32002` for absent and not-allowed alike** | The error cannot be used to probe for connection ids | Loses the distinction between "not yours" and "gone" in the message |
| Prompts | **Two templates, gated by the tool they name** | Honest: no server-side model; a client is never pointed at a call it cannot make | A token scoped to the tool loses the prompt when the scope changes |
| | Empty list | Nothing to maintain | The capability is declared for no reason |

## Decision

**`initialize` refuses a protocol revision the server does not speak with JSON-RPC code `-32022`
carrying `error.data.supported`, and echoes a revision it knows; `resources` and `prompts` are
declared as capabilities; `resources/list` and `resources/read` expose only resources the token's
scope and allowlist reach, and `prompts/list` and `prompts/get` expose pure templates with no
server-side generation.**

Details that bind:

1. **Resources are the connections the token may reach, and one object-tree resource per
   connection.** `queryhive://connections` is the aggregate `connections_list` view;
   `queryhive://connections/{id}` is one connection's hand-picked metadata;
   `queryhive://connections/{id}/objects` and `.../tables` run the `objects`/`tables` command, so a
   read of them is the tool's own answer. A connection's resource carries exactly the fields the
   tool carries — `id`, `name`, `kind`, `host`, `port`, `database` — and never a user name, a
   password, an `options_json` bag or a `secret_ref`.
2. **A resource appears only when its backing tool is in scope.** The aggregate and per-connection
   metadata resources need `connections_list`; the object and table resources need `objects` and
   `tables`. The per-connection resources are built from the same filtered rows
   `connections_list` returns, so a connection outside the allowlist is never enumerated.
3. **A refusal says nothing about existence.** `resources/read` answers every URI outside the
   token's reach — absent, outside the allowlist, outside scope, or simply not this server's URI
   shape — with the same `-32002` "resource not found". The message does not contain "not allowed"
   or "was not found", so it cannot distinguish a real id from a made-up one. The allowlist is
   checked before any store read, exactly as on the tool path.
4. **Prompts are templates, not conversations.** `explain_query` and `summarize_tables` substitute
   their arguments into fixed text; nothing in the server calls a model, and nothing reads a
   connection. Each prompt names the tool it tells a client to call, and is offered only when that
   tool is in the token's scope.
5. **The hand-picked connection row has one copy.** `visible_connections` builds it, and both the
   `connections_list` tool and the connection resources read it. A field added to one path cannot
   be missed on the other.

## Reasons

1. **One authorisation model, not two.** Reusing `McpToken::allows` and `McpToken::allows_connection`
   means a resource cannot be reached by a token the corresponding tool would refuse. A second
   scope vocabulary would drift from the tool list the issue command validates against.
2. **Non-disclosure is the point of the shared `-32002`.** ADR-0015 already refuses a disallowed
   connection before reading the store, so an error cannot be used to enumerate ids. A resource
   read that said "not allowed" for one id and "not found" for another would reopen precisely that
   probe on a new surface.
3. **Refusing an unknown revision is more honest than downgrading.** The client set the version it
   speaks; answering with a different one leaves it believing it was understood. `-32022` plus the
   supported list is the one answer that lets it retry correctly.
4. **A template prompt cannot lie about a model the server does not have.** Rendering is `format!`
   over fixed text; there is no generation, no tool call and no data access hiding behind
   `prompts/get`.
5. **The object/tables resources are the tool's own answer.** Running `Command::Objects` /
   `Command::Tables` with the same settings the tool builds means a resource read and a tool call
   cannot disagree about what a connection's tree looks like.

## Consequences

- **The FFI surface is unchanged.** No `EngineCommand` variant is added; the MCP binary keeps
  mapping onto commands that already exist (invariant #11 does not apply).
- **`resources/read` records a token use.** It is an access to the token's data, so it sets
  `last_used_at` like a `tools/call`; `resources/list` and the prompt methods do not.
- **The stability page's gap note is closed in the same change.** `docs/mcp-stability.md` now states
  the refusal, and its additive table carries resources and prompts alongside tools.
- **The object/tables resources are lazy, not live.** `resources/list` never opens a connection;
  only `resources/read` of an `objects`/`tables` resource does, and that read fails with the same
  engine error the tool would return.
- **`resources/templates/list` is not implemented.** Every URI this server publishes is concrete;
  there is no templated family to advertise, and an empty template list would be noise.
- **A prompt's availability follows its tool's scope.** A token that loses the `explain` scope
  loses `explain_query` with it. That is intended: the prompt exists to drive a call the token must
  still be allowed to make.
