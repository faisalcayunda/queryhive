//! The MCP server: the read-only tool surface, the JSON-RPC framing, and the token that
//! gates both.
//!
//! Fase 2 of `docs/architecture/tablepro-adoption-plan.md`. The pattern is TablePro's —
//! a separate executable inside the bundle that speaks MCP over stdio and is never the
//! app's own process — but no TablePro code is copied; see the ADR for what was taken as
//! an idea and what was decided here.
//!
//! # The surface is intentionally small, and one command is deliberately absent
//!
//! The tools map onto commands that already exist in [`crate`], so there is no second
//! engine to keep in step: [`ToolSpec`] names the command and [`Server::call_tool`]
//! builds the same [`Settings`] the app builds. The set is read-only, and `to_table` is
//! not merely out of scope — it is not a tool at all, and a `tools/call` that names it
//! is refused by name. It has a `replace` mode that runs `DROP TABLE IF EXISTS` before
//! its query (`crate::commands::to_table`), and opening that to a client before the
//! engine has a tested authorisation model is not a thing this phase does.
//!
//! `describe_table` and `table_ddl` (W11) are the same kind of mapping: they run the
//! engine's `columns` and `ddl` commands, whose statements are single catalog reads, and
//! they emit the engine's `table_columns` and `table_ddl` events as they are. The engine
//! masks credentials in a DDL before the event leaves it, so this module has no masking of
//! its own to keep in step. The older `columns` tool stays as it was.
//!
//! # Framing is newline-delimited JSON-RPC, and stdout carries nothing else
//!
//! One JSON-RPC message per line, in both directions. **Every byte on stdout is a
//! protocol message**; a stray `println!` would corrupt the stream for the client, so
//! logs go to stderr and nothing in this module writes to stdout. The reader is
//! [`serve`], which takes its input and output as arguments precisely so a test can
//! drive the framing without a child process.
//!
//! # Scope is checked in two places, and both matter
//!
//! A token carries a list of tool names and a list of connection ids. `tools/list`
//! filters the first; `tools/call` refuses a tool outside it; and any tool that takes a
//! `connection` argument has that argument checked against the second **before** the
//! connection is read, so a token cannot use an error message to discover which
//! connection ids exist. An empty connection list means **no connections**, not all of
//! them: the only safe default for a security list is the one that grants nothing.
//!
//! # Resources and prompts carry the same two checks, not a second copy
//!
//! The resource set is read-only and small: the saved connections a token may reach, and
//! one object-tree resource per connection. A resource appears only when the token's
//! scope reaches the tool it stands for and, for a connection's own resources, only when
//! the allowlist names that connection. Reading a resource the token may not reach is
//! refused with the same `-32002` a resource that does not exist gets, so the error cannot
//! be used to probe for connection ids. Prompts are pure templates rendered from their
//! own arguments — no model lives in this server — and a prompt is offered only when the
//! token may call the tool it tells a client to call.
//!
//! # Version negotiation refuses what it does not speak
//!
//! A client that names a revision this server has is echoed it; a client that names one
//! it does not gets `-32022` with the supported list in `data.supported`, rather than a
//! silent fall back that hides the mismatch.
//!
//! # Credentials never leave this module
//!
//! A password is read from the Keychain, put straight into the per-call [`Settings`],
//! and dropped when the call ends. It is never logged, never returned in an error, and
//! never part of a tool's output. `connections_list` is the one tool that reads the
//! connection store directly and it returns a hand-picked subset — id, name, kind, host,
//! port, database — with the user, the options bag and the `secret_ref` left out.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use qh_credentials::{account_key, KeychainStore, SecretStore};
use qh_driver::DriverKind;
use qh_sql::SafeMode;
use qh_storage::{ConnectionKind, ConnectionRecord, McpTokenRecord, Storage, TokenState};
use qh_sync::SyncId;
use secrecy::ExposeSecret;
use serde_json::{json, Value as Json};

use crate::events::Capture;
use crate::sql_ident::{self, Part, SlotStyle};
use crate::{CancelFlag, CliError, Command, Engine, Settings};

/// The MCP revision this server speaks when the client does not name one it knows.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The revisions this server will echo back.
///
/// Echoing the client's own revision when it is one of these is what the specification
/// asks for. One that is not is **refused** with [`UNSUPPORTED_PROTOCOL_VERSION`] and this
/// list in `error.data.supported`. Falling back to [`PROTOCOL_VERSION`] was the older
/// behaviour and it hid the mismatch: the client could not learn which revisions it could
/// have asked for. [`PROTOCOL_VERSION`] is what an `initialize` naming no revision at all
/// still gets, because a client that merely omitted the field is not a client asking for
/// something nobody built.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The JSON-RPC error code the specification reserves for a protocol revision the server
/// does not speak. The supported list travels in `error.data.supported`.
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// The JSON-RPC error code for a resource the caller may not see. It is deliberately the
/// same whether the resource is absent or merely outside the token, so the error cannot
/// be used to discover connection ids.
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// The base URI of the resources this server exposes.
pub const CONNECTIONS_RESOURCE: &str = "queryhive://connections";

/// The name this server reports in `initialize`.
pub const SERVER_NAME: &str = "queryhive-mcp";

/// The file name of the handshake inside the Application Support directory.
pub const HANDSHAKE_FILE_NAME: &str = "mcp-handshake.json";

/// The `QH_MCP_HANDSHAKE` override, so a test (or a second instance) does not fight over
/// the one path the real server uses.
pub const HANDSHAKE_ENV: &str = "QH_MCP_HANDSHAKE";

/// The environment variable the token may come from instead of `--token`.
pub const TOKEN_ENV: &str = "QH_MCP_TOKEN";

// --------------------------------------------------------------------------- //
// the token, as the server uses it
// --------------------------------------------------------------------------- //

/// One validated token, in the form the request handlers read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToken {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub connections: Vec<String>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl McpToken {
    /// The runtime form of a stored row, refusing a row whose scope columns are
    /// unreadable rather than guessing what they meant.
    pub fn from_record(record: &McpTokenRecord) -> Result<Self, String> {
        Ok(Self {
            id: record.id.to_string(),
            name: record.name.clone(),
            scopes: record.scopes().map_err(|error| {
                format!("token '{}' has an unreadable scope: {error}", record.id)
            })?,
            connections: record.connections().map_err(|error| {
                format!("token '{}' has an unreadable allowlist: {error}", record.id)
            })?,
            expires_at: record.expires_at,
            revoked_at: record.revoked_at,
        })
    }

    /// Whether this token may call one tool.
    pub fn allows(&self, tool: &str) -> bool {
        self.scopes.iter().any(|scope| scope == tool)
    }

    /// Whether this token may name one connection.
    ///
    /// An empty allowlist answers `false` for every id: "no connections" is the only
    /// reading under which a misconfigured token is safe.
    pub fn allows_connection(&self, id: &str) -> bool {
        self.connections.iter().any(|allowed| allowed == id)
    }
}

/// Why a token string could not become a live token. The token itself is never in here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenRefusal {
    /// No `--token` and no `QH_MCP_TOKEN`.
    Missing,
    /// Nothing in the database hashes to it.
    Unknown,
    /// The row is there, but it has been revoked.
    Revoked,
    /// The row is there, but its deadline has passed.
    Expired,
    /// The row is there, but it has been tombstoned.
    Deleted,
}

impl TokenRefusal {
    pub fn message(&self) -> &'static str {
        match self {
            TokenRefusal::Missing => "no MCP token was given; pass --token or set QH_MCP_TOKEN",
            TokenRefusal::Unknown => "the MCP token is not recognised",
            TokenRefusal::Revoked => "the MCP token has been revoked",
            TokenRefusal::Expired => "the MCP token has expired",
            TokenRefusal::Deleted => "the MCP token is not recognised",
        }
    }
}

/// Find the live token for a candidate string, or say why it is not usable.
///
/// The candidate is hashed and looked up; it is never stored, logged or echoed.
pub fn resolve_token(
    store: &Storage,
    candidate: Option<&str>,
    now: i64,
) -> Result<(McpToken, McpTokenRecord), TokenRefusal> {
    let Some(candidate) = candidate else {
        return Err(TokenRefusal::Missing);
    };
    let hash = qh_storage::hash_token(candidate);
    let Some(record) = store
        .mcp_token_by_hash(&hash)
        .map_err(|_| TokenRefusal::Unknown)?
    else {
        return Err(TokenRefusal::Unknown);
    };
    match record.state(now) {
        TokenState::Live => {}
        TokenState::Revoked => return Err(TokenRefusal::Revoked),
        TokenState::Expired => return Err(TokenRefusal::Expired),
        TokenState::Deleted => return Err(TokenRefusal::Deleted),
    }
    let token = McpToken::from_record(&record).map_err(|_| TokenRefusal::Unknown)?;
    Ok((token, record))
}

// --------------------------------------------------------------------------- //
// the tool set
// --------------------------------------------------------------------------- //

/// One tool: its name, what a client reads before calling it, and the command behind it.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Json,
}

/// The registry, in the order `tools/list` returns it.
///
/// The order is the order of the plan's own table. `to_table` is absent on purpose and
/// is refused by name in [`Server::call_tool`], so removing it here cannot be undone by
/// a caller that guesses the name.
pub const TOOLS: [ToolSpec; 11] = [
    ToolSpec {
        name: "db_drivers",
        description:
            "List the database drivers this engine can talk to, with their labels, default \
             ports and object-tree levels. Reads no connection and no setting.",
        schema: no_args,
    },
    ToolSpec {
        name: "connections_list",
        description:
            "List the saved connections this token may reach. Never returns a user name, a \
             password, an options bag or a Keychain reference.",
        schema: no_args,
    },
    ToolSpec {
        name: "objects",
        description:
            "List one level of a connection's object tree with whatever metadata the driver \
             can answer, as column-major columns and rows.",
        schema: connection_scope_schema,
    },
    ToolSpec {
        name: "tables",
        description:
            "List the tables of a catalog (Trino), schema (PostgreSQL) or database (MySQL).",
        schema: connection_scope_schema,
    },
    ToolSpec {
        name: "columns",
        description:
            "Return a table's columns. Runs `SELECT * FROM <table> LIMIT 0` and reports the \
             engine's `columns` event; no row is returned. For nullability, defaults and \
             extras use `describe_table`.",
        schema: columns_schema,
    },
    ToolSpec {
        name: "preview",
        description:
            "Run a read-only SELECT and return its first rows, as the engine's columns/rows \
             events. The statement is sent to the server unchanged.",
        schema: preview_schema,
    },
    ToolSpec {
        name: "count",
        description:
            "Count the rows a SELECT really returns, by wrapping it in a COUNT(*). Returns the \
             engine's count event.",
        schema: sql_schema,
    },
    ToolSpec {
        name: "explain",
        description:
            "Return the plan the server would use for a statement, as the engine's explain \
             events.",
        schema: sql_schema,
    },
    ToolSpec {
        name: "export_to_file",
        description:
            "Stream a SELECT into one or more files under a directory on the machine running \
             this server, and report the paths written.",
        schema: export_schema,
    },
    ToolSpec {
        name: "describe_table",
        description:
            "Describe one table or view from the server's own catalog: each column's type, \
             nullability, default and extras (such as identity or auto_increment). Reads the \
             catalog, not the table, and returns the engine's `table_columns` event.",
        schema: columns_schema,
    },
    ToolSpec {
        name: "table_ddl",
        description:
            "Return the DDL of one table, view or materialized view as the server prints it, \
             as the engine's `table_ddl` event. Credentials the server prints into it are \
             masked and `redacted` says so; a view's definition is shown as written.",
        schema: columns_schema,
    },
];

/// Every tool name, which is also the default scope of a token issued without `--scope`.
pub fn read_only_tool_names() -> Vec<String> {
    TOOLS.iter().map(|tool| tool.name.to_owned()).collect()
}

/// Whether a name is one of the read-only tools. Used to refuse an `--scope` that names
/// a tool nobody has, at issue time rather than at call time.
pub fn is_read_only_tool(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

/// One prompt: a name, what it is for, the tool whose scope gates it, and its arguments.
///
/// A prompt here is a **template**, not a conversation: rendering substitutes the
/// caller's arguments into fixed text and nothing else. There is no model in this process
/// to call, so a prompt that needed one would be a lie.
pub struct PromptSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The tool this prompt tells a client to call. The prompt is offered only to a token
    /// whose scope reaches that tool, so a client is never pointed at a call it cannot make.
    pub scope: &'static str,
    /// `(name, description, required)` for every argument, in the order they are declared.
    pub arguments: &'static [(&'static str, &'static str, bool)],
}

/// The prompts this server offers, in the order `prompts/list` returns them.
pub const PROMPTS: [PromptSpec; 2] = [
    PromptSpec {
        name: "explain_query",
        description: "Ask for a plain-language explanation of a query's execution plan, \
                      fetched with the `explain` tool.",
        scope: "explain",
        arguments: &[
            ("connection", "A connection id from connections_list.", true),
            ("sql", "The SELECT whose plan should be explained.", true),
        ],
    },
    PromptSpec {
        name: "summarize_tables",
        description: "Ask for a short description of the tables on one connection, using \
                      the `tables` tool.",
        scope: "tables",
        arguments: &[("connection", "A connection id from connections_list.", true)],
    },
];

fn no_args() -> Json {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

fn connection_scope_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "connection": {"type": "string", "description": "A connection id from connections_list."},
            "catalog": {"type": "string", "description": "Trino catalog (or PostgreSQL/MySQL database) to browse instead of the connection's own."},
            "schema": {"type": "string", "description": "Schema to browse instead of the connection's own."}
        },
        "required": ["connection"],
        "additionalProperties": false
    })
}

fn columns_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "connection": {"type": "string", "description": "A connection id from connections_list."},
            "table": {"type": "string", "description": "The table (or view) the call is about."},
            "catalog": {"type": "string", "description": "Trino catalog (or PostgreSQL/MySQL database) that qualifies the table."},
            "schema": {"type": "string", "description": "Schema that qualifies the table."}
        },
        "required": ["connection", "table"],
        "additionalProperties": false
    })
}

fn preview_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "connection": {"type": "string", "description": "A connection id from connections_list."},
            "sql": {"type": "string", "description": "The SELECT to run."},
            "limit": {"type": "integer", "description": "Maximum rows to return (default 1000, minimum 1)."}
        },
        "required": ["connection", "sql"],
        "additionalProperties": false
    })
}

fn sql_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "connection": {"type": "string", "description": "A connection id from connections_list."},
            "sql": {"type": "string", "description": "The SELECT to run."}
        },
        "required": ["connection", "sql"],
        "additionalProperties": false
    })
}

fn export_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "connection": {"type": "string", "description": "A connection id from connections_list."},
            "sql": {"type": "string", "description": "The SELECT to stream into files."},
            "format": {"type": "string", "description": "One of the engine's export formats, e.g. csv, jsonl, parquet."},
            "out_dir": {"type": "string", "description": "Directory the files are written under."},
            "name": {"type": "string", "description": "Base file name, without extension (default \"export\")."}
        },
        "required": ["connection", "sql", "format", "out_dir"],
        "additionalProperties": false
    })
}

// --------------------------------------------------------------------------- //
// connection settings
// --------------------------------------------------------------------------- //

/// A resolved connection: its row, its stored options, and the catalog/schema a call
/// will actually use once the arguments have had their say.
struct Resolved {
    record: ConnectionRecord,
    options: Json,
    catalog: String,
    schema: String,
}

/// The per-call settings built from a connection, mirrored from
/// `AppModel.connectionEnvironment` in the app.
///
/// The mapping is reproduced rather than invented, because the engine reads the two
/// sides the same way and a divergence would show up as a connection that works in the
/// app and not through MCP. The one place this is not the app's exact code is
/// `DB_ALL_SCHEMAS`, which the app sets on its browse path rather than in
/// `connectionEnvironment`; it is set here from the same stored option so the object
/// tree looks the same whichever entry point asked.
pub fn connection_environment(
    record: &ConnectionRecord,
    options: &Json,
    password: &str,
) -> Vec<(String, String)> {
    let scheme = option_string(options, "scheme", "https");
    let sslmode = option_string(options, "sslmode", "");
    let schema = option_string(options, "schema", "");
    let verify = options
        .get("verify")
        .and_then(Json::as_bool)
        .unwrap_or(true);
    let show_all_schemas = options
        .get("showAllSchemas")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let trino = matches!(record.kind, ConnectionKind::Trino);
    let transport = transport_of(&scheme);
    // `prefer` decides its own verification and never checks, so `DB_INSECURE` would
    // otherwise demote it to a required, unverified connection — a different mode and not
    // the fallback the user asked for.
    let insecure = if (trino && transport == Transport::Prefer) || verify {
        String::new()
    } else {
        "1".to_owned()
    };

    let mut pairs = vec![
        ("DB_KIND".to_owned(), record.kind.as_str().to_owned()),
        (
            "DB_HOST".to_owned(),
            record.host.clone().unwrap_or_default(),
        ),
        (
            "DB_PORT".to_owned(),
            record
                .port
                .unwrap_or_else(|| default_port(record.kind))
                .to_string(),
        ),
        (
            "DB_USER".to_owned(),
            record.user_name.clone().unwrap_or_default(),
        ),
        ("DB_PASSWORD".to_owned(), password.to_owned()),
        (
            "DB_DATABASE".to_owned(),
            record.database_name.clone().unwrap_or_default(),
        ),
        ("DB_SCHEMA".to_owned(), schema),
        // Trino's transport is a scheme; the other two express encryption through
        // sslmode. `prefer` is the one mode that needs both, and needs `sslmode` to be
        // the one that names it, because a scheme can say "clear" or "encrypt" but never
        // "encrypt if you can" — the same note `AppModel.connectionEnvironment` carries.
        (
            "DB_SCHEME".to_owned(),
            if trino {
                match transport {
                    Transport::Prefer => "https".to_owned(),
                    _ if scheme.is_empty() => "http".to_owned(),
                    _ => scheme.clone(),
                }
            } else {
                String::new()
            },
        ),
        (
            "DB_SSLMODE".to_owned(),
            if trino {
                match transport {
                    Transport::Prefer => "prefer".to_owned(),
                    _ => String::new(),
                }
            } else if sslmode.is_empty() {
                default_ssl_mode(record.kind).to_owned()
            } else {
                sslmode.clone()
            },
        ),
        ("DB_INSECURE".to_owned(), insecure),
    ];
    pairs.push((
        "DB_ALL_SCHEMAS".to_owned(),
        if show_all_schemas { "1" } else { "0" }.to_owned(),
    ));
    pairs
}

/// Trino's transport words, plus the fallback every other stored value lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Https,
    Http,
    Prefer,
}

fn transport_of(scheme: &str) -> Transport {
    match scheme.trim().to_ascii_lowercase().as_str() {
        "https" => Transport::Https,
        "prefer" => Transport::Prefer,
        _ => Transport::Http,
    }
}

fn option_string(options: &Json, key: &str, default: &str) -> String {
    options
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or(default)
        .to_owned()
}

fn default_port(kind: ConnectionKind) -> u16 {
    match kind {
        ConnectionKind::Trino => 8080,
        ConnectionKind::Postgres => 5432,
        ConnectionKind::Mysql => 3306,
    }
}

fn default_ssl_mode(kind: ConnectionKind) -> &'static str {
    match kind {
        ConnectionKind::Postgres => "prefer",
        ConnectionKind::Trino | ConnectionKind::Mysql => "disable",
    }
}

fn driver_kind(kind: ConnectionKind) -> DriverKind {
    match kind {
        ConnectionKind::Trino => DriverKind::Trino,
        ConnectionKind::Postgres => DriverKind::Postgres,
        ConnectionKind::Mysql => DriverKind::Mysql,
    }
}

// --------------------------------------------------------------------------- //
// the server
// --------------------------------------------------------------------------- //

/// Everything one run of the server needs that does not change between requests.
pub struct Server {
    token: McpToken,
    /// The settings every call starts from: `DB_PATH` when the caller named one, so a
    /// test's token store and a production one are addressed the same way.
    base: Settings,
    base_pairs: Vec<(String, String)>,
}

/// What one line produced: a reply, whether it counts as a use of the token, and nothing
/// else. The touch is reported rather than performed so that [`Server`] stays free of
/// blocking storage work and stays testable.
pub struct Handled {
    pub response: Option<Json>,
    pub touch: bool,
}

impl Server {
    /// A server for one already-validated token.
    ///
    /// `base_pairs` is the connection-store settings — in practice `DB_PATH` — and is
    /// prepended to every call's settings, so the tools read the same database the token
    /// was issued into.
    pub fn new(token: McpToken, base_pairs: Vec<(String, String)>) -> Self {
        Self {
            token,
            base: Settings::from_pairs(base_pairs.clone()),
            base_pairs,
        }
    }

    pub fn token(&self) -> &McpToken {
        &self.token
    }

    /// The connection store this server was built against.
    pub fn store(&self) -> Result<Storage, CliError> {
        crate::local::open_storage(&self.base)
    }

    /// Handle one line of the protocol, or say why it could not be handled.
    ///
    /// A notification — a request with no `id` — gets no reply and does nothing, which
    /// covers `notifications/initialized` and every other notification a client may one
    /// day send. The engine is a parameter so that a test can drive a fake one.
    pub fn handle_line(
        &self,
        line: &str,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Handled {
        let parsed: Json = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Handled {
                    response: Some(error_response(
                        Json::Null,
                        -32700,
                        &format!("parse error: {error}"),
                    )),
                    touch: false,
                }
            }
        };
        let Some(request) = parsed.as_object() else {
            return Handled {
                response: Some(error_response(
                    Json::Null,
                    -32600,
                    "invalid request: not a JSON object",
                )),
                touch: false,
            };
        };

        // A request with no `id` is a notification. It is answered with nothing, by the
        // specification and by design: replying to `notifications/initialized` is the
        // classic way to break a client's framing.
        if !request.contains_key("id") {
            return Handled {
                response: None,
                touch: false,
            };
        }
        let id = request.get("id").cloned().unwrap_or(Json::Null);

        let Some(method) = request.get("method").and_then(Json::as_str) else {
            return Handled {
                response: Some(error_response(
                    id,
                    -32600,
                    "invalid request: method must be a string",
                )),
                touch: false,
            };
        };

        match method {
            "initialize" => Handled {
                response: Some(self.initialize_response(id, request.get("params"))),
                touch: false,
            },
            // Declared as a notification and treated as one even if a client puts an `id`
            // on it, because the client is wrong and a reply would be a second thing it
            // does not expect.
            "notifications/initialized" => Handled {
                response: None,
                touch: false,
            },
            "ping" => Handled {
                response: Some(success(id, json!({}))),
                touch: false,
            },
            "tools/list" => Handled {
                response: Some(success(id, json!({"tools": self.list_tools()}))),
                touch: false,
            },
            "tools/call" => match self.tool_call_params(request.get("params")) {
                Ok((name, arguments)) => {
                    let response = match self.call(&name, &arguments, engine, runtime) {
                        Ok(events) => success(id, tool_content(&events_text(&events), false)),
                        Err(message) => success(id, tool_content(&message, true)),
                    };
                    Handled {
                        response: Some(response),
                        // A call is a use of the token whether it succeeded or not: the
                        // point of `last_used_at` is "was this token presented", and a
                        // refusal still presented it.
                        touch: true,
                    }
                }
                // A malformed request is not a tool result: it is the client's own bug,
                // and the JSON-RPC error code says so.
                Err(message) => Handled {
                    response: Some(error_response(id, -32602, &message)),
                    touch: false,
                },
            },
            "resources/list" => Handled {
                response: Some(match self.list_resources(engine, runtime) {
                    Ok(result) => success(id, result),
                    Err(message) => error_response(id, -32603, &message),
                }),
                touch: false,
            },
            "resources/read" => match self.resource_read_params(request.get("params")) {
                Ok(uri) => Handled {
                    response: Some(match self.read_resource(&uri, engine, runtime) {
                        Ok(result) => success(id, result),
                        // One message for "not allowed" and for "not there", so a client
                        // cannot use the refusal to discover connection ids.
                        Err(ResourceReadError::NotFound) => error_response(
                            id,
                            RESOURCE_NOT_FOUND,
                            &format!("resource not found: {uri}"),
                        ),
                        Err(ResourceReadError::Failed(message)) => {
                            error_response(id, -32603, &message)
                        }
                    }),
                    // A read is an access to the token's data, so it counts as a use.
                    touch: true,
                },
                Err(message) => Handled {
                    response: Some(error_response(id, -32602, &message)),
                    touch: false,
                },
            },
            "prompts/list" => Handled {
                response: Some(success(id, json!({"prompts": self.list_prompts()}))),
                touch: false,
            },
            "prompts/get" => match self.prompt_get_params(request.get("params")) {
                Ok((name, arguments)) => Handled {
                    response: Some(match self.get_prompt(&name, &arguments) {
                        Ok(result) => success(id, result),
                        Err(message) => error_response(id, -32602, &message),
                    }),
                    touch: false,
                },
                Err(message) => Handled {
                    response: Some(error_response(id, -32602, &message)),
                    touch: false,
                },
            },
            other => Handled {
                response: Some(error_response(
                    id,
                    -32601,
                    &format!("method not found: {other}"),
                )),
                touch: false,
            },
        }
    }

    /// The reply to `initialize`: a result for a revision this server knows, and an error
    /// carrying the supported list for one it does not.
    ///
    /// Echoing a known revision is what the specification asks for. Refusing an unknown
    /// one instead of quietly answering with the server's own is the change TablePro's
    /// `-32022` names: a fall back hides the mismatch, and the client has no way to learn
    /// which revisions it could have asked for.
    fn initialize_response(&self, id: Json, params: Option<&Json>) -> Json {
        let requested = params
            .and_then(|params| params.get("protocolVersion"))
            .and_then(Json::as_str);
        match requested {
            // No revision named at all: answer with the server's own rather than refuse a
            // client that merely omitted the field.
            None => success(id, self.initialize_result(PROTOCOL_VERSION)),
            Some(version) if SUPPORTED_VERSIONS.contains(&version) => {
                success(id, self.initialize_result(version))
            }
            Some(version) => error_response_data(
                id,
                UNSUPPORTED_PROTOCOL_VERSION,
                &format!("unsupported protocol version '{version}'"),
                json!({"supported": SUPPORTED_VERSIONS}),
            ),
        }
    }

    /// The `initialize` result: the negotiated revision and the capabilities on offer.
    fn initialize_result(&self, version: &str) -> Json {
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
            "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")}
        })
    }

    /// The tools this token may call, in registry order.
    fn list_tools(&self) -> Vec<Json> {
        TOOLS
            .iter()
            .filter(|tool| self.token.allows(tool.name))
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": (tool.schema)(),
                })
            })
            .collect()
    }

    /// The `(name, arguments)` a `tools/call` params object must carry.
    ///
    /// A missing name or a non-object `arguments` is a malformed request (-32602), which
    /// is different from a tool that exists and fails: the first is a client bug the
    /// caller can fix, the second is a result.
    fn tool_call_params(&self, params: Option<&Json>) -> Result<(String, Json), String> {
        let Some(params) = params.and_then(Json::as_object) else {
            return Err("invalid params: tools/call needs an object".to_owned());
        };
        let Some(name) = params.get("name").and_then(Json::as_str) else {
            return Err("invalid params: tools/call needs a string 'name'".to_owned());
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err("invalid params: 'arguments' must be an object".to_owned());
        }
        Ok((name.to_owned(), arguments))
    }

    /// The `uri` a `resources/read` params object must carry.
    fn resource_read_params(&self, params: Option<&Json>) -> Result<String, String> {
        let Some(params) = params.and_then(Json::as_object) else {
            return Err("invalid params: resources/read needs an object".to_owned());
        };
        let Some(uri) = params.get("uri").and_then(Json::as_str) else {
            return Err("invalid params: resources/read needs a string 'uri'".to_owned());
        };
        Ok(uri.to_owned())
    }

    /// The `(name, arguments)` a `prompts/get` params object must carry.
    fn prompt_get_params(&self, params: Option<&Json>) -> Result<(String, Json), String> {
        let Some(params) = params.and_then(Json::as_object) else {
            return Err("invalid params: prompts/get needs an object".to_owned());
        };
        let Some(name) = params.get("name").and_then(Json::as_str) else {
            return Err("invalid params: prompts/get needs a string 'name'".to_owned());
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err("invalid params: 'arguments' must be an object".to_owned());
        }
        Ok((name.to_owned(), arguments))
    }

    /// Run one tool, or say why it was refused.
    fn call(
        &self,
        name: &str,
        arguments: &Json,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Vec<Json>, String> {
        // Refused before the registry is consulted, so the refusal names the reason and
        // the name cannot be smuggled back in by a future edit to `TOOLS`.
        if name == "to_table" {
            return Err(
                "the tool 'to_table' is not available over MCP: it has a write mode that \
                 drops the target table before it runs, and this server is read-only."
                    .to_owned(),
            );
        }
        let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
            return Err(format!("unknown tool '{name}'"));
        };
        if !self.token.allows(tool.name) {
            return Err(format!(
                "the tool '{}' is outside this token's scope",
                tool.name
            ));
        }

        match tool.name {
            "db_drivers" => self.run_tool(Command::DbDrivers, self.base.clone(), engine, runtime),
            "connections_list" => self.connections_list(engine, runtime),
            "objects" => {
                let settings = self.connection_scope(arguments)?;
                self.run_tool(Command::Objects, settings, engine, runtime)
            }
            "tables" => {
                let settings = self.connection_scope(arguments)?;
                self.run_tool(Command::Tables, settings, engine, runtime)
            }
            "columns" => {
                let connection = required_str(arguments, "connection")?;
                let table = required_str(arguments, "table")?;
                let catalog = optional_str(arguments, "catalog");
                let schema = optional_str(arguments, "schema");
                // The target is resolved once, without extra settings, so the qualified
                // name can be built from the same catalog/schema the connection will use.
                let resolved = self.resolve(connection, catalog, schema)?;
                let qualified = sql_ident::qualified(
                    SlotStyle::of(driver_kind(resolved.record.kind)),
                    &resolved.catalog,
                    &resolved.schema,
                    table,
                );
                let settings = self.settings_for(
                    &resolved,
                    // `LIMIT 0` in the statement, not the `LIMIT` setting: the engine
                    // floors that setting to one, and the goal here is columns without a
                    // row. The servers describe a result set before sending any of it, so
                    // the `columns` event arrives and the `rows` events stay empty.
                    &[("SQL", format!("SELECT * FROM {qualified} LIMIT 0"))],
                )?;
                self.run_tool(Command::Preview, settings, engine, runtime)
            }
            "preview" => {
                let connection = required_str(arguments, "connection")?;
                let sql = required_str(arguments, "sql")?;
                let mut extra: Vec<(&str, String)> = vec![("SQL", sql.to_owned())];
                if let Some(limit) = arguments.get("limit").and_then(Json::as_i64) {
                    extra.push(("LIMIT", limit.to_string()));
                }
                let settings = self.connection_settings(connection, None, None, &extra)?;
                self.run_tool(Command::Preview, settings, engine, runtime)
            }
            "count" => {
                let connection = required_str(arguments, "connection")?;
                let sql = required_str(arguments, "sql")?;
                let settings =
                    self.connection_settings(connection, None, None, &[("SQL", sql.to_owned())])?;
                self.run_tool(Command::Count, settings, engine, runtime)
            }
            "explain" => {
                let connection = required_str(arguments, "connection")?;
                let sql = required_str(arguments, "sql")?;
                let settings =
                    self.connection_settings(connection, None, None, &[("SQL", sql.to_owned())])?;
                self.run_tool(Command::Explain, settings, engine, runtime)
            }
            "export_to_file" => {
                let connection = required_str(arguments, "connection")?;
                let sql = required_str(arguments, "sql")?;
                let format = required_str(arguments, "format")?;
                let out_dir = required_str(arguments, "out_dir")?;
                let name = optional_str(arguments, "name").unwrap_or("export");
                let settings = self.connection_settings(
                    connection,
                    None,
                    None,
                    &[
                        ("SQL", sql.to_owned()),
                        ("FORMAT", format.to_owned()),
                        ("OUT_DIR", out_dir.to_owned()),
                        ("NAME", name.to_owned()),
                    ],
                )?;
                self.run_tool(Command::Export, settings, engine, runtime)
            }
            "describe_table" => {
                let settings = self.target_settings(arguments)?;
                self.run_tool(Command::Columns, settings, engine, runtime)
            }
            "table_ddl" => {
                let settings = self.target_settings(arguments)?;
                self.run_tool(Command::Ddl, settings, engine, runtime)
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    /// `connections_list`: the allowlisted rows, reduced to what a client may see.
    fn connections_list(
        &self,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Vec<Json>, String> {
        let listed = self.visible_connections(engine, runtime)?;
        Ok(vec![json!({"event": "connections", "connections": listed})])
    }

    /// The allowlisted connection rows, reduced to what a client may see.
    ///
    /// One copy of the hand-picked field list, shared by the `connections_list` tool and
    /// the connection resources, so a credential cannot be added to one path and missed
    /// on the other. The allowlist is applied here rather than in SQL: the rule has one
    /// reading whatever ordered the rows.
    fn visible_connections(
        &self,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Vec<Json>, String> {
        let events = self.run_tool(Command::Connections, self.base.clone(), engine, runtime)?;
        let listed = events
            .iter()
            .find(|event| event.get("event").and_then(Json::as_str) == Some("connections"))
            .and_then(|event| event.get("connections"))
            .and_then(Json::as_array)
            .map(|rows| {
                rows.iter()
                    .filter(|row| {
                        row.get("id")
                            .and_then(Json::as_str)
                            .is_some_and(|id| self.token.allows_connection(id))
                    })
                    .map(connection_row)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(listed)
    }

    /// `resources/list`: the read-only resources this token may reach.
    ///
    /// A resource is offered only when the token's scope reaches the tool it stands for.
    /// The per-connection resources come from the same filtered rows the
    /// `connections_list` tool returns, so a connection outside the allowlist is not
    /// merely hidden by the filter — it is never built.
    fn list_resources(
        &self,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Json, String> {
        let mut resources = Vec::new();
        let shows_connections = self.token.allows("connections_list");
        let shows_objects = self.token.allows("objects");
        let shows_tables = self.token.allows("tables");
        if shows_connections {
            resources.push(json!({
                "uri": CONNECTIONS_RESOURCE,
                "name": "Saved connections",
                "description": "The saved connections this token may reach, as id, name, \
                                 kind, host, port and database. Never a user name, a \
                                 password, an options bag or a Keychain reference.",
                "mimeType": "application/json",
            }));
        }
        if shows_connections || shows_objects || shows_tables {
            for row in self.visible_connections(engine, runtime)? {
                if shows_connections {
                    resources.push(connection_resource(&row));
                }
                if shows_objects {
                    resources.push(connection_sub_resource(
                        &row,
                        "objects",
                        "The first level of this connection's object tree, as the `objects` \
                         tool answers it.",
                    ));
                }
                if shows_tables {
                    resources.push(connection_sub_resource(
                        &row,
                        "tables",
                        "The tables of this connection, as the `tables` tool answers them.",
                    ));
                }
            }
        }
        Ok(json!({"resources": resources}))
    }

    /// `resources/read`: the text of one resource, or a refusal that says nothing about
    /// whether the resource exists.
    fn read_resource(
        &self,
        uri: &str,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Json, ResourceReadError> {
        match parse_resource_uri(uri) {
            Some(ResourceUri::Connections) => {
                if !self.token.allows("connections_list") {
                    return Err(ResourceReadError::NotFound);
                }
                let rows = self
                    .visible_connections(engine, runtime)
                    .map_err(ResourceReadError::Failed)?;
                Ok(resource_contents(uri, &Json::Array(rows)))
            }
            Some(ResourceUri::Connection(id)) => {
                if !self.token.allows("connections_list") || !self.token.allows_connection(id) {
                    return Err(ResourceReadError::NotFound);
                }
                // Rebuilt from the filtered rows rather than read straight from the store,
                // so the shape a resource read returns cannot drift from the tool's.
                let found = self
                    .visible_connections(engine, runtime)
                    .map_err(ResourceReadError::Failed)?
                    .into_iter()
                    .find(|row| row.get("id").and_then(Json::as_str) == Some(id));
                match found {
                    Some(row) => Ok(resource_contents(uri, &row)),
                    None => Err(ResourceReadError::NotFound),
                }
            }
            Some(ResourceUri::ConnectionObjects(id)) => {
                self.read_connection_tool(uri, id, "objects", Command::Objects, engine, runtime)
            }
            Some(ResourceUri::ConnectionTables(id)) => {
                self.read_connection_tool(uri, id, "tables", Command::Tables, engine, runtime)
            }
            None => Err(ResourceReadError::NotFound),
        }
    }

    /// Read a connection-scoped resource by running the tool it stands for.
    ///
    /// The scope and the allowlist are checked before any store read, exactly as the tool
    /// path checks them, and a connection outside the allowlist is refused as not found
    /// rather than told apart from one that never existed.
    fn read_connection_tool(
        &self,
        uri: &str,
        id: &str,
        tool: &str,
        command: Command,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Json, ResourceReadError> {
        if !self.token.allows(tool) || !self.token.allows_connection(id) {
            return Err(ResourceReadError::NotFound);
        }
        let settings = self
            .connection_settings(id, None, None, &[])
            .map_err(ResourceReadError::Failed)?;
        let events = self
            .run_tool(command, settings, engine, runtime)
            .map_err(ResourceReadError::Failed)?;
        Ok(resource_contents(uri, &Json::Array(events)))
    }

    /// `prompts/list`: the template prompts this token may use.
    fn list_prompts(&self) -> Vec<Json> {
        PROMPTS
            .iter()
            .filter(|prompt| self.token.allows(prompt.scope))
            .map(|prompt| {
                json!({
                    "name": prompt.name,
                    "description": prompt.description,
                    "arguments": prompt
                        .arguments
                        .iter()
                        .map(|(name, description, required)| json!({
                            "name": name,
                            "description": description,
                            "required": required,
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect()
    }

    /// `prompts/get`: a prompt rendered from its arguments, or a refusal.
    ///
    /// The rendering is `format!` over fixed text: nothing here calls a model, and
    /// nothing here reads a connection.
    fn get_prompt(&self, name: &str, arguments: &Json) -> Result<Json, String> {
        let Some(prompt) = PROMPTS.iter().find(|prompt| prompt.name == name) else {
            return Err(format!("unknown prompt '{name}'"));
        };
        if !self.token.allows(prompt.scope) {
            return Err(format!("unknown prompt '{name}'"));
        }
        for (argument, _, required) in prompt.arguments {
            if *required && required_str(arguments, argument).is_err() {
                return Err(format!("the argument '{argument}' is required"));
            }
        }
        let text = match prompt.name {
            "explain_query" => format!(
                "Explain the execution plan of this query on connection '{}'. Call the \
                 `explain` tool with that connection and statement first, then describe the \
                 plan in plain language for someone who did not write the query.\n\n\
                 ```sql\n{}\n```",
                required_str(arguments, "connection")?,
                required_str(arguments, "sql")?,
            ),
            "summarize_tables" => format!(
                "List the tables available on connection '{}' with the `tables` tool, then \
                 summarize what each is likely to hold for a new teammate.",
                required_str(arguments, "connection")?,
            ),
            other => return Err(format!("unknown prompt '{other}'")),
        };
        Ok(json!({
            "description": prompt.description,
            "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
        }))
    }

    /// The settings an `objects`/`tables` call needs: a connection and nothing else.
    fn connection_scope(&self, arguments: &Json) -> Result<Settings, String> {
        let connection = required_str(arguments, "connection")?;
        let catalog = optional_str(arguments, "catalog");
        let schema = optional_str(arguments, "schema");
        self.connection_settings(connection, catalog, schema, &[])
    }

    /// The settings `describe_table` and `table_ddl` need: the connection, and the
    /// `TARGET_*` slots its driver names an object by (`sql_ident::slots`: PostgreSQL schema
    /// and table, MySQL database and table, Trino all three).
    ///
    /// The allowlist is checked by [`Server::resolve`] before anything else is read. A slot
    /// the arguments and the connection both leave empty is refused here, by the argument's
    /// own name, rather than by the engine's `TARGET_*` setting name a client never sees.
    fn target_settings(&self, arguments: &Json) -> Result<Settings, String> {
        let connection = required_str(arguments, "connection")?;
        let table = required_str(arguments, "table")?;
        let resolved = self.resolve(
            connection,
            optional_str(arguments, "catalog"),
            optional_str(arguments, "schema"),
        )?;
        let mut targets: Vec<(&str, String)> = Vec::new();
        for slot in sql_ident::slots(SlotStyle::of(driver_kind(resolved.record.kind))) {
            let (argument, value) = match slot.part {
                Part::Database => ("catalog", resolved.catalog.as_str()),
                Part::Schema => ("schema", resolved.schema.as_str()),
                Part::Table => ("table", table),
            };
            if value.is_empty() {
                return Err(format!(
                    "the argument '{argument}' is required: this connection has no default for it"
                ));
            }
            targets.push((slot.target, value.to_owned()));
        }
        self.settings_for(&resolved, &targets)
    }

    /// Build the per-call settings for a connection, with the allowlist checked first.
    fn connection_settings(
        &self,
        connection: &str,
        catalog: Option<&str>,
        schema: Option<&str>,
        extra: &[(&str, String)],
    ) -> Result<Settings, String> {
        let resolved = self.resolve(connection, catalog, schema)?;
        self.settings_for(&resolved, extra)
    }

    /// Check the token's allowlist and read the connection row, without touching the
    /// Keychain. Splitting this out lets `columns` resolve its target before it decides
    /// what settings to build.
    fn resolve(
        &self,
        connection: &str,
        catalog: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Resolved, String> {
        // Allowlist first, and before any database read: a token that is not allowed to
        // name a connection must not be able to tell a real id from a made-up one.
        if !self.token.allows_connection(connection) {
            return Err(format!(
                "the connection '{connection}' is not allowed for this token"
            ));
        }
        let store = self
            .store()
            .map_err(|error| format!("could not open the connection store: {error}"))?;
        let id = SyncId::parse(connection)
            .map_err(|_| format!("the connection '{connection}' is not a connection id"))?;
        let Some(record) = store
            .connection(&id)
            .map_err(|error| format!("could not read the connection store: {error}"))?
        else {
            return Err(format!("the connection '{connection}' was not found"));
        };
        if record.meta.is_deleted() {
            return Err(format!("the connection '{connection}' was not found"));
        }
        let options: Json = serde_json::from_str(&record.options_json).unwrap_or(Json::Null);
        // An argument wins when it is there and not blank; otherwise the connection's own
        // stored value is what the app would have used.
        let catalog = match catalog.filter(|value| !value.is_empty()) {
            Some(value) => value.to_owned(),
            None => record.database_name.clone().unwrap_or_default(),
        };
        let schema = match schema.filter(|value| !value.is_empty()) {
            Some(value) => value.to_owned(),
            None => option_string(&options, "schema", ""),
        };
        Ok(Resolved {
            record,
            options,
            catalog,
            schema,
        })
    }

    /// Read the password and turn a resolved connection into the settings a command
    /// reads. The password lives only in the returned map.
    fn settings_for(
        &self,
        resolved: &Resolved,
        extra: &[(&str, String)],
    ) -> Result<Settings, String> {
        let password = password_for(&resolved.record)?;
        let mut pairs = self.base_pairs.clone();
        pairs.extend(connection_environment(
            &resolved.record,
            &resolved.options,
            &password,
        ));
        pairs.push(("DB_DATABASE".to_owned(), resolved.catalog.clone()));
        pairs.push(("DB_SCHEMA".to_owned(), resolved.schema.clone()));
        // Every MCP call runs under `read_only`, whatever the connection's own Safe
        // Mode says. Scope names tools, and a tool that runs caller SQL can carry any
        // SQL — so `preview` was a write path until the engine grew a guard. This is
        // that guard, pinned here rather than left to the connection: a token is not
        // allowed to widen its own reach by naming a `full` connection. ADR-0015's
        // "scope does not constrain SQL" is what this closes.
        pairs.push((
            "SAFE_MODE".to_owned(),
            SafeMode::ReadOnly.as_str().to_owned(),
        ));
        for (key, value) in extra {
            pairs.push(((*key).to_owned(), value.clone()));
        }
        Ok(Settings::from_pairs(pairs))
    }

    /// One engine command, its events collected.
    fn run_tool(
        &self,
        command: Command,
        settings: Settings,
        engine: &dyn Engine,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Vec<Json>, String> {
        let mut capture = Capture::new();
        match runtime.block_on(crate::run(
            command,
            &settings,
            &mut capture,
            engine,
            &CancelFlag::new(),
        )) {
            Ok(()) => Ok(capture.lines),
            Err(error) => {
                let mut message = error.message();
                let warnings = error.warnings();
                if !warnings.is_empty() {
                    message.push_str(&format!(" (warnings: {})", warnings.join("; ")));
                }
                Err(message)
            }
        }
    }
}

/// The password for a connection, from the Keychain, or empty.
///
/// A missing item is not an error: a password-less database is real, and the same
/// reading the `credential` command's `has`/`get` take. An error here is reported with
/// the connection's *name* and the Keychain's own words, never the secret.
fn password_for(record: &ConnectionRecord) -> Result<String, String> {
    let account = record
        .secret_ref
        .clone()
        .unwrap_or_else(|| record.meta.id.to_string());
    let key = account_key(&account);
    match KeychainStore.get(&key) {
        Ok(Some(secret)) => Ok(secret.expose_secret().to_owned()),
        Ok(None) => Ok(String::new()),
        Err(error) => Err(format!(
            "could not read the password for '{}': {error}",
            record.name
        )),
    }
}

fn required_str<'a>(arguments: &'a Json, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("the argument '{key}' is required"))
}

fn optional_str<'a>(arguments: &'a Json, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Json::as_str)
}

// --------------------------------------------------------------------------- //
// resources
// --------------------------------------------------------------------------- //

/// One connection as a resource may expose it: the same hand-picked subset
/// `connections_list` returns, never a user name, an options bag or a `secret_ref`.
fn connection_row(row: &Json) -> Json {
    json!({
        "id": row.get("id").cloned().unwrap_or(Json::Null),
        "name": row.get("name").cloned().unwrap_or(Json::Null),
        "kind": row.get("kind").cloned().unwrap_or(Json::Null),
        "host": row.get("host").cloned().unwrap_or(Json::Null),
        "port": row.get("port").cloned().unwrap_or(Json::Null),
        "database": row.get("database").cloned().unwrap_or(Json::Null),
    })
}

/// The `id`/`name` a row is described by, with the id as the fallback name.
fn row_identity(row: &Json) -> (&str, &str) {
    let id = row.get("id").and_then(Json::as_str).unwrap_or_default();
    let name = row.get("name").and_then(Json::as_str).unwrap_or(id);
    (id, name)
}

/// The resource for one saved connection.
fn connection_resource(row: &Json) -> Json {
    let (id, name) = row_identity(row);
    json!({
        "uri": format!("{CONNECTIONS_RESOURCE}/{id}"),
        "name": name,
        "description": format!(
            "The saved connection '{name}', as id, name, kind, host, port and database."
        ),
        "mimeType": "application/json",
    })
}

/// The `objects`/`tables` resource for one saved connection.
fn connection_sub_resource(row: &Json, kind: &str, description: &str) -> Json {
    let (id, name) = row_identity(row);
    json!({
        "uri": format!("{CONNECTIONS_RESOURCE}/{id}/{kind}"),
        "name": format!("{name} — {kind}"),
        "description": description,
        "mimeType": "application/json",
    })
}

/// A `resources/read` result: the value as one JSON text block.
fn resource_contents(uri: &str, value: &Json) -> Json {
    json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": serde_json::to_string(value).expect("resource values are serialisable"),
        }]
    })
}

/// A resource URI this server owns, or `None` for anything else.
enum ResourceUri<'a> {
    /// `queryhive://connections`
    Connections,
    /// `queryhive://connections/{id}`
    Connection(&'a str),
    /// `queryhive://connections/{id}/objects`
    ConnectionObjects(&'a str),
    /// `queryhive://connections/{id}/tables`
    ConnectionTables(&'a str),
}

/// Parse a resource URI, answering only for URIs in this server's own shape.
///
/// A URI outside the shape is `None`, which the reader turns into the same not-found a
/// resource outside the token gets: the shape of the answer says nothing about whether
/// the thing named exists.
fn parse_resource_uri(uri: &str) -> Option<ResourceUri<'_>> {
    let rest = uri.strip_prefix(CONNECTIONS_RESOURCE)?;
    if rest.is_empty() {
        return Some(ResourceUri::Connections);
    }
    let rest = rest.strip_prefix('/')?;
    if rest.is_empty() || rest.ends_with('/') {
        return None;
    }
    if let Some(id) = rest.strip_suffix("/objects") {
        return (!id.is_empty() && !id.contains('/')).then_some(ResourceUri::ConnectionObjects(id));
    }
    if let Some(id) = rest.strip_suffix("/tables") {
        return (!id.is_empty() && !id.contains('/')).then_some(ResourceUri::ConnectionTables(id));
    }
    if rest.contains('/') {
        return None;
    }
    Some(ResourceUri::Connection(rest))
}

/// Why `resources/read` could not answer.
enum ResourceReadError {
    /// Not a resource this token may see: absent, outside the allowlist, or outside the
    /// token's scope. Deliberately one case, so the refusal cannot be used to probe.
    NotFound,
    /// A resource the token may reach, but the read failed. The message is the engine's.
    Failed(String),
}

// --------------------------------------------------------------------------- //
// JSON-RPC helpers
// --------------------------------------------------------------------------- //

fn success(id: Json, result: Json) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Json, code: i64, message: &str) -> Json {
    error_response_data(id, code, message, Json::Null)
}

/// An error reply that may carry structured `data`; a null `data` is omitted.
fn error_response_data(id: Json, code: i64, message: &str, data: Json) -> Json {
    let mut error = json!({"code": code, "message": message});
    if !data.is_null() {
        error["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// A `tools/call` result: the engine's events as one text block, or a failure message.
fn tool_content(text: &str, is_error: bool) -> Json {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error
    })
}

/// The text a successful call carries: the events as a JSON array.
fn events_text(events: &[Json]) -> String {
    serde_json::to_string(&Json::Array(events.to_vec())).expect("events are serialisable")
}

// --------------------------------------------------------------------------- //
// transport
// --------------------------------------------------------------------------- //

/// Read newline-delimited requests and write newline-delimited responses.
///
/// One line in, at most one line out; a notification produces no line at all. The touch
/// callback is invoked once per tool call and is deliberately the caller's: writing
/// `last_used_at` is blocking storage work, and this loop is where the process decides
/// when it happens (and where a failure is logged rather than turned into a protocol
/// error the client cannot act on).
pub fn serve<R: BufRead, W: Write, F: FnMut()>(
    server: &Server,
    engine: &dyn Engine,
    runtime: &tokio::runtime::Runtime,
    input: R,
    output: &mut W,
    touch: &mut F,
) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let handled = server.handle_line(&line, engine, runtime);
        if handled.touch {
            touch();
        }
        if let Some(response) = handled.response {
            serde_json::to_writer(&mut *output, &response)?;
            output.write_all(b"\n")?;
            // Flushed per line: a client waiting for one reply must not be held up by a
            // buffer that only fills at exit.
            output.flush()?;
        }
    }
    Ok(())
}

// --------------------------------------------------------------------------- //
// handshake
// --------------------------------------------------------------------------- //

/// The handshake record another process reads to find a running server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub pid: u32,
    pub started_at: i64,
    pub token_id: String,
    pub server_version: String,
}

impl Handshake {
    pub fn to_json(&self) -> Json {
        json!({
            "pid": self.pid,
            "started_at": self.started_at,
            "token_id": self.token_id,
            "server_version": self.server_version,
        })
    }
}

/// Where the handshake goes: `QH_MCP_HANDSHAKE`, or the Application Support directory
/// beside `connections.json`.
pub fn handshake_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(HANDSHAKE_ENV) {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("QueryHive")
            .join(HANDSHAKE_FILE_NAME),
    )
}

/// Write the handshake, creating its directory if needed.
///
/// The file is `0600`: it names a live token's row and a pid, and nothing but this user has any
/// business reading it. The mode is the writer's half of the check [`read_handshake`] makes.
pub fn write_handshake(path: &std::path::Path, handshake: &Handshake) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(&handshake.to_json()).expect("a handshake is serialisable");
    // Tightened after the write as well as asked for at creation: `.mode()` only applies when the
    // file is created, so an existing file left looser by an older build would otherwise stay so.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Remove the handshake, ignoring a file that is already gone.
pub fn remove_handshake(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Read a handshake, ignoring one whose process is no longer alive.
///
/// The "ignored" half is the point: a server killed with SIGKILL never removes its file,
/// and a reader that trusted a stale pid would try to talk to a process that is gone.
///
/// A file anybody can read is ignored too, because it names a live token row: the writer creates
/// it `0600`, so a looser mode means somebody else wrote it, and the safe reading of that is to
/// disbelieve the file. The owner and executable checks TablePro makes are deliberately not done:
/// nothing in this tree reads the handshake yet, and there is no safe `getuid` without `libc`
/// (which this crate forbids). That is stated rather than skipped silently.
pub fn read_handshake(path: &std::path::Path) -> Option<Handshake> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path).ok()?;
    if metadata.mode() & 0o077 != 0 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let value: Json = serde_json::from_str(&text).ok()?;
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    if !pid_is_alive(pid) {
        return None;
    }
    Some(Handshake {
        pid,
        started_at: value.get("started_at").and_then(Json::as_i64).unwrap_or(0),
        token_id: value
            .get("token_id")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        server_version: value
            .get("server_version")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Whether a process is still there, by `kill -0` (signal 0: no signal, just the check).
///
/// This shells out rather than calling `kill(2)` directly because this workspace forbids
/// `unsafe`, and there is no safe standard-library way to ask about another process.
/// `/bin/kill -0` is the documented probe and returns non-zero for a pid that is gone or
/// not ours to signal.
pub fn pid_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        // `kill` writes "No such process" to stderr for the ordinary answer here, which
        // is a dead pid. Silenced rather than captured: a probe this frequent must not
        // put noise on the server's own stderr.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// --------------------------------------------------------------------------- //
// issue / list / revoke, for the binary's subcommands
// --------------------------------------------------------------------------- //

/// Open the connection store the token subcommands work against, honouring `DB_PATH`.
pub fn open_store(base_pairs: &[(String, String)]) -> Result<Storage, String> {
    let settings = Settings::from_pairs(base_pairs.to_vec());
    crate::local::open_storage(&settings).map_err(|error| error.message())
}

/// The JSON `list` prints, with no hash and no secret.
pub fn token_listing(tokens: &[McpTokenRecord]) -> Json {
    json!({
        "tokens": tokens
            .iter()
            .map(|token| json!({
                "id": token.id.as_str(),
                "name": token.name,
                // The non-secret head, so a person can match a row to a token they are holding
                // without the row holding anything that could be used as one.
                "prefix": token.token_prefix,
                "scopes": token.scopes().unwrap_or_default(),
                "connections": token.connections().unwrap_or_default(),
                "expires_at": token.expires_at,
                "revoked_at": token.revoked_at,
                "last_used_at": token.last_used_at,
            }))
            .collect::<Vec<_>>()
    })
}

/// Parse a comma-separated argument list into trimmed, non-empty entries.
pub fn comma_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The expiry a `--days` argument asks for, as a unix-millisecond time.
pub fn expires_after_days(days: i64, now: i64) -> i64 {
    now.saturating_add(days.saturating_mul(24 * 60 * 60 * 1_000))
}
