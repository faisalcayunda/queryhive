//! The MCP server's protocol, tools and scope rules, driven in-process.
//!
//! The end-to-end test over the real stdio transport lives beside this one in
//! `tests/mcp_stdio.rs`; this file is about the framing and the decisions, so it calls
//! [`Server::handle_line`] directly and never spawns anything. The engine is the real
//! [`RealEngine`] — the tools that need no network (`db_drivers`, `connections_list`) are
//! exercised for real, and the rest are refused before a connection is ever opened.
//!
//! `serde_json::Value` is compared rather than the raw text, for the same reason the
//! golden harness parses before comparing: key order is not part of the contract, but
//! which keys are present is.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qh_core::EngineError;
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};
use qh_ffi::mcp::{self, Handshake, McpToken, Server};
use qh_ffi::{Engine, RealEngine};
use qh_storage::{ConnectionKind, ConnectionRecord, McpTokenRecord, Storage};
use qh_sync::SyncId;
use serde_json::{json, Value as Json};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("a runtime")
}

/// A token that may call everything the read-only set offers.
fn full_token() -> McpToken {
    McpToken {
        id: "11111111-1111-7111-8111-111111111111".to_owned(),
        name: "test".to_owned(),
        scopes: mcp::read_only_tool_names(),
        connections: Vec::new(),
        expires_at: None,
        revoked_at: None,
    }
}

/// One request through the protocol, with the response asserted to be there.
fn request(server: &Server, runtime: &tokio::runtime::Runtime, request: &Json) -> Json {
    let handled = server.handle_line(&request.to_string(), &RealEngine::new(), runtime);
    handled
        .response
        .expect("a request with an id gets a response")
}

/// `tools/call` for one name with one arguments object.
fn call(server: &Server, runtime: &tokio::runtime::Runtime, name: &str, arguments: Json) -> Json {
    request(
        server,
        runtime,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}}),
    )
}

/// The text inside a successful `tools/call` result, parsed back as the events array.
fn call_events(result: &Json) -> Vec<Json> {
    assert_eq!(result["result"]["isError"], json!(false), "{result}");
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    serde_json::from_str(text).expect("the events are a JSON array")
}

// --------------------------------------------------------------------------- //
// initialize
// --------------------------------------------------------------------------- //

#[test]
fn initialize_reports_the_tool_capability_and_the_server_name() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}),
    );
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], json!(mcp::PROTOCOL_VERSION));
    assert_eq!(result["capabilities"]["tools"], json!({}));
    // Resources and prompts are declared, because this change implements both.
    assert_eq!(result["capabilities"]["resources"], json!({}));
    assert_eq!(result["capabilities"]["prompts"], json!({}));
    assert_eq!(result["serverInfo"]["name"], json!(mcp::SERVER_NAME));
    assert_eq!(
        result["serverInfo"]["version"],
        json!(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn initialize_echoes_a_known_revision_that_is_not_the_default() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05"}}),
    );
    assert_eq!(response["result"]["protocolVersion"], json!("2024-11-05"));
}

#[test]
fn initialize_refuses_a_revision_it_does_not_speak_and_lists_the_ones_it_does() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "1999-01-01"}}),
    );
    assert_eq!(
        response["error"]["code"],
        json!(mcp::UNSUPPORTED_PROTOCOL_VERSION)
    );
    assert_eq!(response["error"]["code"], json!(-32022));
    let supported: Vec<&str> = response["error"]["data"]["supported"]
        .as_array()
        .expect("a supported list")
        .iter()
        .map(|version| version.as_str().expect("a version"))
        .collect();
    assert_eq!(supported, mcp::SUPPORTED_VERSIONS.to_vec());
    // A refusal is not a result that happens to carry an error alongside it.
    assert!(response["result"].is_null(), "{response}");
}

// --------------------------------------------------------------------------- //
// tools/list and scope
// --------------------------------------------------------------------------- //

#[test]
fn tools_list_is_filtered_by_the_tokens_scope() {
    let mut token = full_token();
    token.scopes = vec!["db_drivers".to_owned(), "preview".to_owned()];
    let server = Server::new(token, Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, vec!["db_drivers", "preview"]);
    // Every listed tool carries the schema a client needs before calling it.
    for tool in response["result"]["tools"].as_array().expect("an array") {
        assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
    }
}

#[test]
fn to_table_is_not_in_the_tool_list_even_for_a_full_token() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    assert!(!names.contains(&"to_table"), "{names:?}");
    assert_eq!(names.len(), 11, "the read-only set is eleven tools");
}

// --------------------------------------------------------------------------- //
// tools/call
// --------------------------------------------------------------------------- //

#[test]
fn a_tool_call_succeeds_and_carries_the_engines_events() {
    let server = Server::new(full_token(), Vec::new());
    let result = call(&server, &runtime(), "db_drivers", json!({}));
    let events = call_events(&result);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"], json!("drivers"));
    let drivers = events[0]["drivers"].as_array().expect("a drivers array");
    assert_eq!(drivers.len(), 3, "trino, postgres and mysql");
}

#[test]
fn a_tool_outside_the_scope_is_refused_with_a_message() {
    let mut token = full_token();
    token.scopes = vec!["db_drivers".to_owned()];
    let server = Server::new(token, Vec::new());
    let result = call(
        &server,
        &runtime(),
        "preview",
        json!({"connection": "x", "sql": "select 1"}),
    );
    assert_eq!(result["result"]["isError"], json!(true));
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("outside this token's scope"), "{text}");
}

#[test]
fn an_unknown_tool_is_refused_by_name() {
    let server = Server::new(full_token(), Vec::new());
    let result = call(&server, &runtime(), "nope", json!({}));
    assert_eq!(result["result"]["isError"], json!(true));
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("unknown tool 'nope'"), "{text}");
}

#[test]
fn to_table_is_refused_by_name_even_when_it_is_not_in_scope() {
    let server = Server::new(full_token(), Vec::new());
    let result = call(
        &server,
        &runtime(),
        "to_table",
        json!({"connection": "x", "sql": "select 1"}),
    );
    assert_eq!(result["result"]["isError"], json!(true));
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    // The reason has to be the write mode, not merely "unknown tool": a reader has to
    // know why this one is refused while the others are offered.
    assert!(text.contains("not available over MCP"), "{text}");
    assert!(text.contains("drops the target table"), "{text}");
}

#[test]
fn a_missing_tool_argument_is_a_readable_tool_error() {
    let server = Server::new(full_token(), Vec::new());
    let result = call(&server, &runtime(), "preview", json!({"connection": "x"}));
    assert_eq!(result["result"]["isError"], json!(true));
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("'sql' is required"), "{text}");
}

// --------------------------------------------------------------------------- //
// connections_list
// --------------------------------------------------------------------------- //

/// A database with one allowed connection and one that must not be returned.
fn seeded_allowlisted_db() -> (tempfile::TempDir, String, String, String) {
    let dir = tempfile::tempdir().expect("a temp directory");
    let path = dir.path().join("queryhive.sqlite3");
    let mut storage = Storage::open(&path).expect("open");
    storage.migrate().expect("migrate");

    let mut allowed = ConnectionRecord::new("Allowed", ConnectionKind::Trino, 1_700_000_000_000);
    allowed.host = Some("trino.internal".to_owned());
    allowed.port = Some(8443);
    allowed.database_name = Some("hive".to_owned());
    allowed.user_name = Some("secret-user".to_owned());
    allowed.options_json = r#"{"scheme":"https","verify":true}"#.to_owned();
    allowed.secret_ref = Some(allowed.meta.id.to_string());
    storage.save_connection(&allowed).expect("save allowed");
    let allowed_id = allowed.meta.id.to_string();

    let denied = ConnectionRecord::new("Denied", ConnectionKind::Postgres, 1_700_000_000_001);
    storage.save_connection(&denied).expect("save denied");
    let denied_id = denied.meta.id.to_string();

    (
        dir,
        path.to_string_lossy().into_owned(),
        allowed_id,
        denied_id,
    )
}

#[test]
fn connections_list_returns_only_the_allowlist_and_no_credentials() {
    let (_dir, db_path, allowed_id, _denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    let result = call(&server, &runtime(), "connections_list", json!({}));

    let events = call_events(&result);
    let listed = events[0]["connections"].as_array().expect("an array");
    assert_eq!(listed.len(), 1, "only the allowed connection is listed");
    assert_eq!(listed[0]["id"], json!(allowed_id));
    assert_eq!(listed[0]["name"], json!("Allowed"));
    assert_eq!(listed[0]["kind"], json!("trino"));
    assert_eq!(listed[0]["host"], json!("trino.internal"));
    assert_eq!(listed[0]["port"], json!(8443));
    assert_eq!(listed[0]["database"], json!("hive"));

    // The fields that must never be here, asserted on the serialised text so a nested
    // copy cannot slip past a key check.
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    for forbidden in [
        "secret-user",
        "secret_ref",
        "options",
        "password",
        "user_name",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
    assert!(
        !text.contains("Denied"),
        "the disallowed row leaked: {text}"
    );
}

#[test]
fn an_empty_allowlist_reaches_no_connection() {
    let (_dir, db_path, _allowed_id, _denied_id) = seeded_allowlisted_db();
    let server = Server::new(full_token(), vec![("DB_PATH".to_owned(), db_path)]);
    let result = call(&server, &runtime(), "connections_list", json!({}));
    let events = call_events(&result);
    assert!(events[0]["connections"]
        .as_array()
        .expect("an array")
        .is_empty());
}

#[test]
fn a_connection_outside_the_allowlist_is_refused_without_saying_it_exists() {
    let (_dir, db_path, allowed_id, _denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    // The allowlist names a connection that is not in the database at all, so the
    // argument below is a real row the token may not touch.
    token.connections = vec!["99999999-9999-7999-8999-999999999999".to_owned()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);

    // Allowed but absent from the store: "not found" is the honest answer, and it is
    // only reachable for an id the operator put on the allowlist.
    let allowed = call(
        &server,
        &runtime(),
        "tables",
        json!({"connection": "99999999-9999-7999-8999-999999999999"}),
    );
    assert_eq!(allowed["result"]["isError"], json!(true));
    assert!(allowed["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("was not found"));

    // A real row that is not on the allowlist: refused before the store is read, and the
    // message must not distinguish it from an id that never existed.
    let denied = call(
        &server,
        &runtime(),
        "tables",
        json!({"connection": allowed_id}),
    );
    assert_eq!(denied["result"]["isError"], json!(true));
    let text = denied["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("not allowed for this token"), "{text}");
    assert!(!text.contains("was not found"), "{text}");
}

#[test]
fn mcp_runs_caller_sql_read_only_so_a_drop_is_refused_before_connecting() {
    // The gap ADR-0015 documented: token scope names tools, and a tool that runs caller
    // SQL can carry any SQL. The engine's Safe Mode is what closes it, and the MCP
    // server pins every call to `read_only` whatever the connection's own mode says.
    // The refusal names the mode, which is what proves the setting was injected: without
    // it the run would have tried to reach `trino.internal` and failed there instead.
    let (_dir, db_path, allowed_id, _denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    let result = call(
        &server,
        &runtime(),
        "preview",
        json!({"connection": allowed_id, "sql": "DROP TABLE people"}),
    );
    assert_eq!(result["result"]["isError"], json!(true));
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(
        text.contains("read-only"),
        "MCP must run caller SQL read-only: {text}"
    );
}

// --------------------------------------------------------------------------- //
// describe_table and table_ddl (W11-T5, blueprint w11 section 10.2)
// --------------------------------------------------------------------------- //

/// The two tools W11 added, and the nine that were there before.
const METADATA_TOOLS: [&str; 2] = ["describe_table", "table_ddl"];

/// The text of a refused (or failed) `tools/call`, asserting it is a tool error.
fn error_text(result: &Json) -> String {
    assert_eq!(result["result"]["isError"], json!(true), "{result}");
    result["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block")
        .to_owned()
}

fn tool_names(response: &Json) -> Vec<String> {
    response["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name").to_owned())
        .collect()
}

#[test]
fn the_metadata_tools_are_listed_for_a_full_token_and_filtered_for_a_narrow_one() {
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});

    let full = request(&Server::new(full_token(), Vec::new()), &runtime(), &list);
    let names = tool_names(&full);
    assert_eq!(names.len(), 11, "{names:?}");
    for tool in METADATA_TOOLS {
        assert!(names.iter().any(|name| name == tool), "{tool} missing");
        assert!(
            mcp::is_read_only_tool(tool),
            "{tool} is not a read-only tool"
        );
        assert!(mcp::read_only_tool_names().iter().any(|name| name == tool));
    }
    // Both take the same arguments, and a client reads them from the schema.
    for tool in full["result"]["tools"].as_array().expect("an array") {
        if METADATA_TOOLS.contains(&tool["name"].as_str().expect("a name")) {
            assert_eq!(
                tool["inputSchema"]["required"],
                json!(["connection", "table"])
            );
        }
    }

    let mut narrow = full_token();
    narrow.scopes = vec!["describe_table".to_owned(), "tables".to_owned()];
    let names = tool_names(&request(
        &Server::new(narrow, Vec::new()),
        &runtime(),
        &list,
    ));
    assert_eq!(names, vec!["tables", "describe_table"]);
}

#[test]
fn a_token_issued_before_the_metadata_tools_existed_does_not_reach_them() {
    // The scope list is exactly what `issue` wrote then: nine names. It fails closed, with the
    // same sentence any out-of-scope tool gets.
    let mut token = full_token();
    token
        .scopes
        .retain(|scope| !METADATA_TOOLS.contains(&scope.as_str()));
    assert_eq!(token.scopes.len(), 9);
    let (_dir, db_path, allowed_id, _denied_id) = seeded_allowlisted_db();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    for tool in METADATA_TOOLS {
        let text = error_text(&call(
            &server,
            &runtime(),
            tool,
            json!({"connection": allowed_id, "table": "t", "schema": "s"}),
        ));
        assert!(
            text.contains("outside this token's scope"),
            "{tool}: {text}"
        );
    }
}

#[test]
fn an_empty_allowlist_refuses_the_metadata_tools_without_saying_whether_the_connection_exists() {
    let (_dir, db_path, allowed_id, denied_id) = seeded_allowlisted_db();
    // `full_token` has an empty allowlist: the scope names every tool and the allowlist grants
    // nothing, which is the one combination a misconfigured token lands on.
    let server = Server::new(full_token(), vec![("DB_PATH".to_owned(), db_path)]);
    for tool in METADATA_TOOLS {
        // A real row, a real row of another kind, and an id that was never there: the three
        // answers must be the same sentence, differing only in the id the caller sent.
        let mut shapes = Vec::new();
        for id in [
            allowed_id.as_str(),
            denied_id.as_str(),
            "99999999-9999-7999-8999-999999999999",
        ] {
            let text = error_text(&call(
                &server,
                &runtime(),
                tool,
                json!({"connection": id, "table": "t", "schema": "s"}),
            ));
            assert!(
                text.contains("not allowed for this token"),
                "{tool}: {text}"
            );
            assert!(!text.contains("was not found"), "{tool}: {text}");
            assert!(!text.contains("not a connection id"), "{tool}: {text}");
            shapes.push(text.replace(id, "<id>"));
        }
        assert!(
            shapes.windows(2).all(|pair| pair[0] == pair[1]),
            "{shapes:?}"
        );
    }
}

#[test]
fn a_connection_outside_the_allowlist_is_refused_for_the_metadata_tools_as_columns_refuses_it() {
    let (_dir, db_path, allowed_id, denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![allowed_id];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    let arguments = json!({"connection": denied_id, "table": "t", "schema": "s"});
    let columns = error_text(&call(&server, &runtime(), "columns", arguments.clone()));
    assert!(columns.contains("not allowed for this token"), "{columns}");
    for tool in METADATA_TOOLS {
        assert_eq!(
            error_text(&call(&server, &runtime(), tool, arguments.clone())),
            columns,
            "{tool} must refuse exactly as `columns` does"
        );
    }
}

#[test]
fn a_slot_nothing_supplies_is_refused_by_the_arguments_name_before_any_connection() {
    // The seeded PostgreSQL row has no `schema` option, and PostgreSQL names a table by schema
    // and table. The engine would say `TARGET_SCHEMA`, a setting no MCP client has ever seen.
    let (_dir, db_path, _allowed_id, denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![denied_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    for tool in METADATA_TOOLS {
        let text = error_text(&call(
            &server,
            &runtime(),
            tool,
            json!({"connection": denied_id, "table": "t"}),
        ));
        assert!(
            text.contains("the argument 'schema' is required"),
            "{tool}: {text}"
        );
        assert!(!text.contains("TARGET_"), "{tool}: {text}");

        let text = error_text(&call(
            &server,
            &runtime(),
            tool,
            json!({"connection": denied_id}),
        ));
        assert!(
            text.contains("the argument 'table' is required"),
            "{tool}: {text}"
        );
    }
}

/// A session that records every statement and answers the metadata ones with one canned row, so
/// a recipe that asks for several statements asks for all of them.
struct ScriptedSession {
    driver: Box<dyn Driver>,
    statements: Arc<Mutex<Vec<String>>>,
    show_create: String,
}

/// One batch of text cells, column-major as `ColumnBatch` wants them.
struct OneBatch {
    columns: Vec<qh_core::ColumnMeta>,
    batch: Option<qh_core::ColumnBatch>,
}

#[async_trait]
impl qh_driver::Cursor for OneBatch {
    fn columns(&self) -> &[qh_core::ColumnMeta] {
        &self.columns
    }

    async fn next_batch(
        &mut self,
        _max_rows: usize,
    ) -> Result<Option<qh_core::ColumnBatch>, EngineError> {
        Ok(self.batch.take())
    }
}

impl ScriptedSession {
    /// The cells of the one row a statement is answered with, `None` for a NULL, or no row.
    fn canned(&self, sql: &str) -> Option<Vec<Option<String>>> {
        let text = |cell: &str| Some(cell.to_owned());
        if sql.contains("pg_get_viewdef") {
            // The PostgreSQL head: an ordinary table.
            Some(vec![
                text("r"),
                text("s"),
                text("t"),
                None,
                None,
                None,
                None,
            ])
        } else if sql.contains("pg_get_constraintdef") || sql.contains("pg_get_indexdef") {
            None
        } else if sql.contains("information_schema.tables") && sql.contains("table_name =") {
            // Trino's kind lookup.
            Some(vec![text("BASE TABLE")])
        } else if sql.starts_with("SHOW CREATE") {
            Some(vec![
                Some(self.show_create.clone()),
                Some(self.show_create.clone()),
            ])
        } else if sql.contains("pg_attribute") {
            Some(vec![text("id"), text("bigint"), text("NO"), None, text("")])
        } else {
            Some(vec![text("t"), text("BASE TABLE")])
        }
    }
}

#[async_trait]
impl Session for ScriptedSession {
    fn capabilities(&self) -> qh_driver::Capabilities {
        self.driver.capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        _options: &qh_driver::ExecuteOptions,
    ) -> Result<Box<dyn qh_driver::Cursor>, EngineError> {
        self.statements.lock().unwrap().push(sql.to_owned());
        let Some(row) = self.canned(sql) else {
            return Ok(Box::new(OneBatch {
                columns: Vec::new(),
                batch: None,
            }));
        };
        let columns = (0..row.len())
            .map(|index| qh_core::ColumnMeta::new(format!("c{index}"), "text"))
            .collect();
        let batch = qh_core::ColumnBatch::new(
            row.into_iter()
                .map(|cell| {
                    vec![cell.map_or(qh_core::Value::Null, |text| {
                        qh_core::Value::Text(text.into())
                    })]
                })
                .collect(),
        )
        .expect("one row of equal columns");
        Ok(Box::new(OneBatch {
            columns,
            batch: Some(batch),
        }))
    }

    async fn browse(
        &mut self,
        _level: qh_driver::BrowseLevel,
        _path: &qh_driver::ObjectPath,
        _include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        Ok(Vec::new())
    }

    async fn objects(
        &mut self,
        _path: &qh_driver::ObjectPath,
    ) -> Result<qh_driver::ObjectsPage, EngineError> {
        Ok(qh_driver::ObjectsPage::default())
    }

    fn explain_statement(&self, sql: &str) -> String {
        format!("EXPLAIN {sql}")
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        Ok(())
    }

    fn read_only_statement(&self) -> Option<&'static str> {
        Some(READ_ONLY_SWITCH)
    }
}

/// The statement a session is switched read-only with. It is sent only when the run's Safe Mode is
/// `read_only`, so seeing it is seeing the mode MCP pinned.
const READ_ONLY_SWITCH: &str = "SET SESSION READ ONLY (test)";

/// The real drivers' metadata and capabilities over sessions that never leave the process.
struct ScriptedEngine {
    inner: RealEngine,
    statements: Arc<Mutex<Vec<String>>>,
    show_create: String,
}

impl ScriptedEngine {
    fn new(show_create: &str) -> Self {
        Self {
            inner: RealEngine::new(),
            statements: Arc::new(Mutex::new(Vec::new())),
            show_create: show_create.to_owned(),
        }
    }

    fn statements(&self) -> Vec<String> {
        self.statements.lock().unwrap().clone()
    }
}

#[async_trait]
impl Engine for ScriptedEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        let driver: Box<dyn Driver> = match config.kind {
            DriverKind::Postgres => Box::new(qh_driver_postgres::PostgresDriver::new()),
            DriverKind::Mysql => Box::new(qh_driver_mysql::MysqlDriver::new()),
            DriverKind::Trino => Box::new(qh_driver_trino::TrinoDriver::new()),
        };
        Ok(Box::new(ScriptedSession {
            driver,
            statements: self.statements.clone(),
            show_create: self.show_create.clone(),
        }))
    }
}

/// `tools/call` through a given engine.
fn call_with(server: &Server, engine: &dyn Engine, name: &str, arguments: Json) -> Json {
    server
        .handle_line(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}})
            .to_string(),
            engine,
            &runtime(),
        )
        .response
        .expect("a request with an id gets a response")
}

/// One connection of each kind, every one carrying a user name and a Keychain reference that must
/// not reach a tool's output. The ids are returned with their kinds.
fn metadata_db() -> (tempfile::TempDir, String, Vec<(ConnectionKind, String)>) {
    let dir = tempfile::tempdir().expect("a temp directory");
    let path = dir.path().join("queryhive.sqlite3");
    let mut storage = Storage::open(&path).expect("open");
    storage.migrate().expect("migrate");

    let mut ids = Vec::new();
    for (kind, name, database, options) in [
        (
            ConnectionKind::Postgres,
            "Pg",
            "app",
            r#"{"schema":"sales"}"#,
        ),
        (ConnectionKind::Mysql, "My", "shop", "{}"),
        (
            ConnectionKind::Trino,
            "Tr",
            "hive",
            r#"{"schema":"web","scheme":"http"}"#,
        ),
    ] {
        let mut record = ConnectionRecord::new(name, kind, 1_700_000_000_000);
        record.host = Some("db.invalid".to_owned());
        record.database_name = Some(database.to_owned());
        record.user_name = Some("meta-secret-user".to_owned());
        record.options_json = options.to_owned();
        record.secret_ref = Some(format!("keychain-ref-{}", record.meta.id));
        storage.save_connection(&record).expect("save");
        ids.push((kind, record.meta.id.to_string()));
    }
    (dir, path.to_string_lossy().into_owned(), ids)
}

fn allowing(ids: &[(ConnectionKind, String)]) -> McpToken {
    let mut token = full_token();
    token.connections = ids.iter().map(|(_, id)| id.clone()).collect();
    token
}

#[test]
fn the_metadata_tools_name_the_object_the_way_each_driver_does_and_stay_read_only() {
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);

    for (kind, id) in &ids {
        let (dialect, object) = match kind {
            ConnectionKind::Postgres => (
                qh_sql::Dialect::Postgres,
                json!({"catalog": null, "schema": "sales", "table": "t"}),
            ),
            ConnectionKind::Mysql => (
                qh_sql::Dialect::Mysql,
                json!({"catalog": "shop", "schema": null, "table": "t"}),
            ),
            ConnectionKind::Trino => (
                qh_sql::Dialect::Trino,
                json!({"catalog": "hive", "schema": "web", "table": "t"}),
            ),
        };
        for (tool, event) in [
            ("describe_table", "table_columns"),
            ("table_ddl", "table_ddl"),
        ] {
            let engine = ScriptedEngine::new("CREATE TABLE t (id int)");
            let result = call_with(
                &server,
                &engine,
                tool,
                json!({"connection": id, "table": "t"}),
            );
            let events = call_events(&result);
            assert_eq!(events.len(), 1, "{kind:?} {tool}: {events:?}");
            assert_eq!(events[0]["event"], json!(event), "{kind:?} {tool}");
            assert_eq!(events[0]["object"], object, "{kind:?} {tool}");

            // `read_only` kept: the session was switched read-only, which the engine does only
            // under that mode and which a `full` connection would never have been sent, and
            // everything else it sent is one read under every reading of the dialect.
            let sent = engine.statements();
            assert!(
                sent.iter().any(|sql| sql == READ_ONLY_SWITCH),
                "{kind:?} {tool}: {sent:?}"
            );
            let reads: Vec<&String> = sent.iter().filter(|sql| *sql != READ_ONLY_SWITCH).collect();
            assert!(!reads.is_empty(), "{kind:?} {tool} sent nothing");
            for sql in reads {
                let decisions =
                    qh_sql::decisions_readings(qh_sql::SafeMode::ReadOnly, sql, dialect.readings());
                assert_eq!(decisions.len(), 1, "{kind:?} {tool}: {sql}");
                assert_eq!(
                    decisions[0].kind,
                    qh_sql::StatementKind::ReadOnly,
                    "{kind:?} {tool}: {sql}"
                );
            }
        }
    }
}

#[test]
fn an_argument_overrides_the_connections_own_database_and_schema() {
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);
    let (_, trino) = ids
        .iter()
        .find(|(kind, _)| *kind == ConnectionKind::Trino)
        .unwrap();
    let engine = ScriptedEngine::new("CREATE TABLE t (id int)");
    let result = call_with(
        &server,
        &engine,
        "describe_table",
        json!({"connection": trino, "table": "orders", "catalog": "tpch", "schema": "tiny"}),
    );
    let events = call_events(&result);
    assert_eq!(
        events[0]["object"],
        json!({"catalog": "tpch", "schema": "tiny", "table": "orders"})
    );
}

#[test]
fn a_ddl_with_a_password_in_it_comes_out_masked_and_no_credential_reaches_either_tool() {
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);
    let (_, mysql) = ids
        .iter()
        .find(|(kind, _)| *kind == ConnectionKind::Mysql)
        .unwrap();
    // A FEDERATED table prints the URL it forwards to, password included. The engine masks it
    // before the event leaves, so the MCP output is masked by the same code the app's tab is.
    let federated = "CREATE TABLE t (id int) ENGINE=FEDERATED \
                     CONNECTION='mysql://app:hunter2@db.internal:3306/shop/t'";
    for tool in METADATA_TOOLS {
        let engine = ScriptedEngine::new(federated);
        let result = call_with(
            &server,
            &engine,
            tool,
            json!({"connection": mysql, "table": "t"}),
        );
        let text = result["result"]["content"][0]["text"]
            .as_str()
            .expect("a text block");
        for forbidden in [
            "hunter2",
            "meta-secret-user",
            "keychain-ref",
            "secret_ref",
            "DB_PASSWORD",
            "DB_JWT",
        ] {
            assert!(
                !text.contains(forbidden),
                "{tool}: {forbidden} leaked: {text}"
            );
        }
    }
    let engine = ScriptedEngine::new(federated);
    let events = call_events(&call_with(
        &server,
        &engine,
        "table_ddl",
        json!({"connection": mysql, "table": "t"}),
    ));
    assert_eq!(events[0]["redacted"], json!(true), "{events:?}");
    assert!(
        events[0]["ddl"].as_str().expect("a ddl").contains("***"),
        "{events:?}"
    );
}

#[test]
fn a_ddl_with_newlines_is_still_one_line_on_the_wire() {
    // MCP framing is one JSON-RPC message per line, and a DDL is the first tool output with real
    // newlines in it. They must travel escaped inside the one line, never as a second line.
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);
    let (_, mysql) = ids
        .iter()
        .find(|(kind, _)| *kind == ConnectionKind::Mysql)
        .unwrap();
    let engine = ScriptedEngine::new("CREATE TABLE t (\n  id int\n)");
    let line = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                      "params": {"name": "table_ddl", "arguments": {"connection": mysql, "table": "t"}}})
    .to_string();
    let mut output = Vec::new();
    mcp::serve(
        &server,
        &engine,
        &runtime(),
        line.as_bytes(),
        &mut output,
        &mut || {},
    )
    .expect("serve");
    let written = String::from_utf8(output).expect("utf-8");
    assert_eq!(written.matches('\n').count(), 1, "{written:?}");
    assert!(written.ends_with('\n'));
    let reply: Json = serde_json::from_str(written.trim_end()).expect("one JSON reply");
    assert_eq!(reply["id"], json!(7));
    let events = call_events(&reply);
    assert!(
        events[0]["ddl"]
            .as_str()
            .expect("a ddl")
            .contains("\n  id int\n"),
        "{events:?}"
    );
}

// --------------------------------------------------------------------------- //
// resources
// --------------------------------------------------------------------------- //

/// The `uri` of every resource a `resources/list` reply carries.
fn resource_uris(response: &Json) -> Vec<String> {
    response["result"]["resources"]
        .as_array()
        .expect("a resources array")
        .iter()
        .map(|resource| resource["uri"].as_str().expect("a uri").to_owned())
        .collect()
}

#[test]
fn resources_list_is_filtered_by_scope_and_the_allowlist() {
    let (_dir, db_path, allowed_id, denied_id) = seeded_allowlisted_db();

    // A full token sees the aggregate, the allowed connection, and its object resources.
    let mut token = full_token();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path.clone())]);
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
    );
    let uris = resource_uris(&response);
    assert!(
        uris.contains(&"queryhive://connections".to_owned()),
        "{uris:?}"
    );
    assert!(
        uris.contains(&format!("queryhive://connections/{allowed_id}")),
        "{uris:?}"
    );
    assert!(
        uris.contains(&format!("queryhive://connections/{allowed_id}/objects")),
        "{uris:?}"
    );
    assert!(
        uris.contains(&format!("queryhive://connections/{allowed_id}/tables")),
        "{uris:?}"
    );
    // Nothing mentions the denied connection, by id or by name.
    assert!(!uris.iter().any(|uri| uri.contains(&denied_id)), "{uris:?}");
    let text = serde_json::to_string(&response).expect("json");
    assert!(!text.contains("Denied"), "{text}");

    // A token that may not call `objects`/`tables` gets the connection resources only.
    let mut fields_only = full_token();
    fields_only.scopes = vec!["connections_list".to_owned()];
    fields_only.connections = vec![allowed_id.clone()];
    let server = Server::new(fields_only, vec![("DB_PATH".to_owned(), db_path.clone())]);
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
    );
    let uris = resource_uris(&response);
    assert!(
        uris.contains(&format!("queryhive://connections/{allowed_id}")),
        "{uris:?}"
    );
    assert!(
        !uris.iter().any(|uri| uri.ends_with("/objects")),
        "{uris:?}"
    );
    assert!(!uris.iter().any(|uri| uri.ends_with("/tables")), "{uris:?}");

    // A token with no connection-scoped tool gets nothing, whatever is in the store.
    let mut none = full_token();
    none.scopes = vec!["db_drivers".to_owned()];
    none.connections = vec![allowed_id.clone()];
    let server = Server::new(none, vec![("DB_PATH".to_owned(), db_path)]);
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
    );
    assert!(resource_uris(&response).is_empty());
}

#[test]
fn resources_read_refuses_out_of_allowlist_without_saying_it_exists() {
    let (_dir, db_path, allowed_id, denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);

    // A real connection the token may not reach, and an id that never existed. Both get
    // the same not-found, and neither message says which case it was.
    let denied_uri = format!("queryhive://connections/{denied_id}");
    let absent_uri = "queryhive://connections/99999999-9999-7999-8999-999999999999".to_owned();
    let denied = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": {"uri": denied_uri}}),
    );
    let absent = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": {"uri": absent_uri}}),
    );
    for response in [&denied, &absent] {
        assert_eq!(response["error"]["code"], json!(mcp::RESOURCE_NOT_FOUND));
        assert_eq!(response["error"]["code"], json!(-32002));
        assert!(response["result"].is_null(), "{response}");
    }
    let denied_message = denied["error"]["message"].as_str().expect("a message");
    assert!(
        denied_message.contains("resource not found"),
        "{denied_message}"
    );
    assert!(!denied_message.contains("not allowed"), "{denied_message}");
    assert!(
        !denied_message.contains("was not found"),
        "{denied_message}"
    );
    assert!(!denied_message.contains("Denied"), "{denied_message}");

    // The object resource of a denied connection is refused too, even though this token
    // has the `objects` scope: the allowlist is the second gate.
    let objects = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": {"uri": format!("queryhive://connections/{denied_id}/objects")}}),
    );
    assert_eq!(objects["error"]["code"], json!(mcp::RESOURCE_NOT_FOUND));

    // A resource the token may reach reads, and carries no credential field.
    let allowed = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": {"uri": format!("queryhive://connections/{allowed_id}")}}),
    );
    assert!(allowed["error"].is_null(), "{allowed}");
    let text = allowed["result"]["contents"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("Allowed"), "{text}");
    assert!(text.contains(&allowed_id), "{text}");
    for forbidden in [
        "secret-user",
        "secret_ref",
        "options",
        "password",
        "user_name",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
}

#[test]
fn resources_read_of_the_aggregate_lists_only_the_allowlist() {
    let (_dir, db_path, allowed_id, denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": {"uri": "queryhive://connections"}}),
    );
    let text = response["result"]["contents"][0]["text"]
        .as_str()
        .expect("a text block");
    let rows: Vec<Json> = serde_json::from_str(text).expect("a JSON array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], json!(allowed_id));
    assert!(!text.contains(&denied_id));
    assert!(!text.contains("Denied"), "{text}");
}

// --------------------------------------------------------------------------- //
// prompts
// --------------------------------------------------------------------------- //

#[test]
fn prompts_list_answers_and_is_filtered_by_scope() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "prompts/list"}),
    );
    let prompts = response["result"]["prompts"]
        .as_array()
        .expect("a prompts array");
    let names: Vec<&str> = prompts
        .iter()
        .map(|prompt| prompt["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, vec!["explain_query", "summarize_tables"]);
    for prompt in prompts {
        assert!(!prompt["description"]
            .as_str()
            .expect("a description")
            .is_empty());
        assert!(!prompt["arguments"]
            .as_array()
            .expect("arguments")
            .is_empty());
    }

    // A token scoped to one tool is offered only the prompt for that tool.
    let mut token = full_token();
    token.scopes = vec!["explain".to_owned()];
    let server = Server::new(token, Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "prompts/list"}),
    );
    let names: Vec<&str> = response["result"]["prompts"]
        .as_array()
        .expect("a prompts array")
        .iter()
        .map(|prompt| prompt["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, vec!["explain_query"]);
}

#[test]
fn prompts_get_renders_the_template_from_its_arguments() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "prompts/get",
                "params": {"name": "explain_query",
                           "arguments": {"connection": "abc", "sql": "select 1"}}}),
    );
    assert_eq!(response["result"]["messages"][0]["role"], json!("user"));
    let text = response["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("abc"), "{text}");
    assert!(text.contains("select 1"), "{text}");

    // A missing required argument is a params error, not a rendered prompt.
    let missing = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "prompts/get",
                "params": {"name": "explain_query", "arguments": {"connection": "abc"}}}),
    );
    assert_eq!(missing["error"]["code"], json!(-32602));
    assert!(missing["result"].is_null(), "{missing}");

    // An unknown prompt is refused.
    let unknown = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "prompts/get",
                "params": {"name": "nope"}}),
    );
    assert_eq!(unknown["error"]["code"], json!(-32602));
}

// --------------------------------------------------------------------------- //
// framing
// --------------------------------------------------------------------------- //

#[test]
fn one_request_per_line_gets_one_response_per_line_and_notifications_get_none() {
    let server = Server::new(full_token(), Vec::new());
    let mut input = String::new();
    input.push_str(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}).to_string());
    input.push('\n');
    // A notification: no id, so no line comes back.
    input.push_str(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string());
    input.push('\n');
    input.push_str(&json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}).to_string());
    input.push('\n');
    input.push('\n'); // a blank line is skipped, not answered
    input.push_str(&json!({"jsonrpc": "2.0", "id": 3, "method": "no/such/method"}).to_string());
    input.push('\n');

    let mut output: Vec<u8> = Vec::new();
    let mut touches = 0;
    mcp::serve(
        &server,
        &RealEngine::new(),
        &runtime(),
        input.as_bytes(),
        &mut output,
        &mut || touches += 1,
    )
    .expect("the stream");

    let text = String::from_utf8(output).expect("utf-8");
    let lines: Vec<Json> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect();
    assert_eq!(lines.len(), 3, "three requests, three replies: {text}");
    assert_eq!(lines[0]["id"], json!(1));
    assert_eq!(
        lines[0]["result"]["serverInfo"]["name"],
        json!(mcp::SERVER_NAME)
    );
    assert_eq!(lines[1]["id"], json!(2));
    assert_eq!(lines[1]["result"], json!({}));
    assert_eq!(lines[2]["id"], json!(3));
    assert_eq!(lines[2]["error"]["code"], json!(-32601));
    assert_eq!(touches, 0, "no tool was called");
}

#[test]
fn an_unparseable_line_is_a_parse_error_with_a_null_id() {
    let server = Server::new(full_token(), Vec::new());
    let handled = server.handle_line("{not json", &RealEngine::new(), &runtime());
    let response = handled.response.expect("a response");
    assert_eq!(response["error"]["code"], json!(-32700));
    assert_eq!(response["id"], Json::Null);
}

#[test]
fn malformed_tool_call_params_are_a_json_rpc_error_not_a_tool_result() {
    let server = Server::new(full_token(), Vec::new());
    let response = request(
        &server,
        &runtime(),
        &json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": {"arguments": {}}}),
    );
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(response["result"].is_null(), "{response}");
}

#[test]
fn a_notification_that_calls_a_tool_gets_no_reply() {
    let server = Server::new(full_token(), Vec::new());
    let handled = server.handle_line(
        &json!({"jsonrpc": "2.0", "method": "tools/call",
                "params": {"name": "db_drivers", "arguments": {}}})
        .to_string(),
        &RealEngine::new(),
        &runtime(),
    );
    assert!(handled.response.is_none());
    assert!(!handled.touch);
}

// --------------------------------------------------------------------------- //
// connection environment
// --------------------------------------------------------------------------- //

fn record(kind: ConnectionKind) -> ConnectionRecord {
    let mut record = ConnectionRecord::new("Example", kind, 1_700_000_000_000);
    record.host = Some("db.internal".to_owned());
    record.port = Some(8443);
    record.user_name = Some("faisal".to_owned());
    record.database_name = Some("hive".to_owned());
    record
}

fn environment(kind: ConnectionKind, options: Json, password: &str) -> Json {
    pairs_to_json(mcp::connection_environment(
        &record(kind),
        &options,
        &mcp::Secrets {
            password: password.to_owned(),
            ..mcp::Secrets::default()
        },
    ))
}

/// A settings list as one JSON object, which is what makes the assertions read by key.
fn pairs_to_json(pairs: Vec<(String, String)>) -> Json {
    Json::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key, Json::String(value)))
            .collect(),
    )
}

#[test]
fn the_trino_transports_map_the_way_the_app_maps_them() {
    for (scheme, verify, expected_scheme, expected_sslmode, expected_insecure) in [
        ("https", true, "https", "", ""),
        ("https", false, "https", "", "1"),
        ("http", true, "http", "", ""),
        ("prefer", true, "https", "prefer", ""),
        // The stored verify flag cannot apply to `prefer`, so it must not leak into
        // DB_INSECURE and turn the fallback into a required, unverified connection.
        ("prefer", false, "https", "prefer", ""),
    ] {
        let options = json!({"scheme": scheme, "verify": verify});
        let env = environment(ConnectionKind::Trino, options, "hunter2");
        assert_eq!(env["DB_KIND"], json!("trino"), "{scheme}");
        assert_eq!(env["DB_SCHEME"], json!(expected_scheme), "{scheme}");
        assert_eq!(env["DB_SSLMODE"], json!(expected_sslmode), "{scheme}");
        assert_eq!(env["DB_INSECURE"], json!(expected_insecure), "{scheme}");
        assert_eq!(env["DB_HOST"], json!("db.internal"));
        // Settings are text: the engine parses DB_PORT itself, and the app sends a string.
        assert_eq!(env["DB_PORT"], json!("8443"));
        assert_eq!(env["DB_USER"], json!("faisal"));
        assert_eq!(env["DB_PASSWORD"], json!("hunter2"));
        assert_eq!(env["DB_DATABASE"], json!("hive"));
    }
}

#[test]
fn postgres_and_mysql_express_encryption_through_sslmode() {
    // A blank stored mode takes the kind's own default, and Trino sends no scheme.
    let postgres = environment(ConnectionKind::Postgres, json!({}), "");
    assert_eq!(postgres["DB_SCHEME"], json!(""));
    assert_eq!(postgres["DB_SSLMODE"], json!("prefer"));
    assert_eq!(postgres["DB_INSECURE"], json!(""));
    assert_eq!(postgres["DB_KIND"], json!("postgres"));

    let mysql = environment(ConnectionKind::Mysql, json!({}), "");
    assert_eq!(mysql["DB_SSLMODE"], json!("disable"));
    assert_eq!(mysql["DB_INSECURE"], json!(""));

    // An explicit mode is sent unchanged, and `verify:false` becomes DB_INSECURE.
    let explicit = environment(
        ConnectionKind::Postgres,
        json!({"sslmode": "require", "verify": false}),
        "",
    );
    assert_eq!(explicit["DB_SSLMODE"], json!("require"));
    assert_eq!(explicit["DB_INSECURE"], json!("1"));
}

#[test]
fn the_stored_schema_and_show_all_schemas_reach_the_settings() {
    let env = environment(
        ConnectionKind::Postgres,
        json!({"schema": "analytics", "showAllSchemas": true}),
        "",
    );
    assert_eq!(env["DB_SCHEMA"], json!("analytics"));
    assert_eq!(env["DB_ALL_SCHEMAS"], json!("1"));

    let hidden = environment(ConnectionKind::Postgres, json!({}), "");
    assert_eq!(hidden["DB_ALL_SCHEMAS"], json!("0"));
}

#[test]
fn a_port_that_was_never_set_takes_the_kinds_default() {
    let mut bare = ConnectionRecord::new("Bare", ConnectionKind::Mysql, 1);
    bare.port = None;
    let env = pairs_to_json(mcp::connection_environment(
        &bare,
        &json!({}),
        &mcp::Secrets::default(),
    ));
    assert_eq!(env["DB_PORT"], json!("3306"));
}

// --------------------------------------------------------------------------- //
// the connection mapping shared with the app (W11 section 10.1)
// --------------------------------------------------------------------------- //

fn fixture() -> Json {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/connection_env.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("the shared fixture"))
        .expect("the fixture is JSON")
}

/// A `connections.json` row (the app's own field names) as the record the store holds: the
/// columns for what has one, the whole row as the options bag, exactly as `import.rs` keeps it.
fn record_from_row(row: &Json) -> ConnectionRecord {
    let kind = ConnectionKind::parse(row["kind"].as_str().unwrap()).unwrap();
    let mut record = ConnectionRecord::new("fixture", kind, 1);
    record.host = row["host"].as_str().map(str::to_owned);
    record.port = row["port"].as_i64().map(|port| port as u16);
    record.user_name = row["user"].as_str().map(str::to_owned);
    record.database_name = row["database"].as_str().map(str::to_owned);
    record.options_json = row.to_string();
    record
}

fn secrets_from(json: &Json) -> mcp::Secrets {
    let text = |key: &str| json[key].as_str().unwrap_or_default().to_owned();
    mcp::Secrets {
        password: text("password"),
        ssh_password: text("sshPassword"),
        ssh_passphrase: text("sshPassphrase"),
        jwt: text("jwt"),
    }
}

fn app_known_hosts() -> String {
    qh_storage::import::legacy_directory()
        .expect("HOME is set")
        .join("known_hosts")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn the_mapping_matches_the_fixture_the_app_is_tested_against() {
    let fixture = fixture();
    let keys: Vec<&str> = fixture["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let record = record_from_row(&case["connection"]);
        let options: Json = serde_json::from_str(&record.options_json).unwrap();
        let env = pairs_to_json(mcp::connection_environment(
            &record,
            &options,
            &secrets_from(&case["secrets"]),
        ));
        for key in &keys {
            let expected = case["expect"][key].as_str().unwrap_or_default();
            let expected = if expected == "<app-known-hosts>" {
                app_known_hosts()
            } else {
                expected.to_owned()
            };
            let actual = env.get(*key).and_then(Json::as_str).unwrap_or_default();
            assert_eq!(actual, expected, "{name}: {key}");
        }
    }
}

#[test]
fn a_saved_connection_never_yields_a_host_key_pin_or_detail_and_never_prints_a_secret() {
    // Over every combination of the options that shape the tunnel, with every secret filled in:
    // MCP has no accept path, so neither key may exist (W11 5.6 item 8), and `Secrets` is not
    // printable.
    let secrets = mcp::Secrets {
        password: "SENTINEL-db".to_owned(),
        ssh_password: "SENTINEL-ssh".to_owned(),
        ssh_passphrase: "SENTINEL-phrase".to_owned(),
        jwt: "SENTINEL-jwt".to_owned(),
    };
    let rendered = format!("{secrets:?}");
    assert!(!rendered.contains("SENTINEL"), "{rendered}");

    for kind in [
        ConnectionKind::Postgres,
        ConnectionKind::Mysql,
        ConnectionKind::Trino,
    ] {
        for host in ["", "bastion"] {
            for auth in ["agent", "key", "password", "surprise"] {
                for db_auth in ["password", "jwt"] {
                    for use_config in [false, true] {
                        let options = json!({
                            "sshHost": host, "sshAuth": auth, "dbAuth": db_auth,
                            "sshUseConfig": use_config, "sshPort": 22, "sshUser": "u",
                            "sshKeyPath": "/k", "caFile": "/ca.pem",
                        });
                        let pairs = mcp::connection_environment(&record(kind), &options, &secrets);
                        for (key, _) in &pairs {
                            assert_ne!(key, "SSH_HOST_KEY_ACCEPT");
                            assert_ne!(key, "SSH_HOST_KEY_DETAIL");
                        }
                        let env = pairs_to_json(pairs);
                        // The app's trust file is named exactly when there is a bastion.
                        assert_eq!(
                            env.get("SSH_APP_KNOWN_HOSTS").is_some(),
                            !host.is_empty(),
                            "{kind:?} {host} {auth}"
                        );
                        // A secret appears only where its own auth mode uses it.
                        let has = |key: &str| env.get(key).is_some();
                        assert_eq!(has("SSH_PASSWORD"), !host.is_empty() && auth == "password");
                        assert_eq!(has("SSH_KEY_PASSPHRASE"), !host.is_empty() && auth == "key");
                        let jwt = kind == ConnectionKind::Trino && db_auth == "jwt";
                        assert_eq!(has("DB_JWT"), jwt);
                        assert_eq!(
                            env["DB_PASSWORD"],
                            json!(if jwt { "" } else { "SENTINEL-db" })
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_statement_bound_is_the_connections_own_or_the_default_and_never_unbounded() {
    let bound = |options: Json| {
        environment(ConnectionKind::Postgres, options, "")["STATEMENT_TIMEOUT_MS"].clone()
    };
    assert_eq!(
        bound(json!({})),
        json!("60000"),
        "nothing chosen: the app's default"
    );
    assert_eq!(bound(json!({"statementTimeoutMS": 15000})), json!("15000"));
    // `0` means no bound in the app; nobody watches an MCP call, so it is not honoured here.
    assert_eq!(bound(json!({"statementTimeoutMS": 0})), json!("60000"));
    assert_eq!(bound(json!({"statementTimeoutMS": -5})), json!("60000"));
    assert_eq!(
        bound(json!({"statementTimeoutMS": 9_999_999})),
        json!("600000")
    );
    assert_eq!(bound(json!({"statementTimeoutMS": "soon"})), json!("60000"));
}

// --------------------------------------------------------------------------- //
// handshake
// --------------------------------------------------------------------------- //

#[test]
fn a_handshake_round_trips_and_is_removed() {
    let dir = tempfile::tempdir().expect("a temp directory");
    let path = dir.path().join("mcp-handshake.json");
    let handshake = Handshake {
        pid: std::process::id(),
        started_at: 1_700_000_000_000,
        token_id: "11111111-1111-7111-8111-111111111111".to_owned(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    mcp::write_handshake(&path, &handshake).expect("write");
    assert_eq!(mcp::read_handshake(&path), Some(handshake));
    mcp::remove_handshake(&path);
    assert_eq!(mcp::read_handshake(&path), None);
    // Removing twice is a no-op rather than an error.
    mcp::remove_handshake(&path);
}

#[test]
fn a_handshake_whose_pid_is_gone_is_ignored() {
    let dir = tempfile::tempdir().expect("a temp directory");
    let path = dir.path().join("mcp-handshake.json");
    // A process that exists long enough to have a pid, and is then gone.
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn");
    let pid = child.id();
    child.kill().expect("kill");
    child.wait().expect("wait");
    // Give the OS a moment to reap it.
    std::thread::sleep(Duration::from_millis(50));
    assert!(!mcp::pid_is_alive(pid), "the pid is gone");

    let handshake = Handshake {
        pid,
        started_at: 1,
        token_id: "t".to_owned(),
        server_version: "0.1.0".to_owned(),
    };
    mcp::write_handshake(&path, &handshake).expect("write");
    assert_eq!(mcp::read_handshake(&path), None);
}

#[test]
fn the_servers_own_pid_is_alive() {
    assert!(mcp::pid_is_alive(std::process::id()));
}

#[test]
fn a_handshake_others_can_read_is_ignored() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("mcp-handshake.json");
    let handshake = Handshake {
        pid: std::process::id(),
        started_at: 1,
        token_id: "t".to_owned(),
        server_version: "0.1.0".to_owned(),
    };
    mcp::write_handshake(&path, &handshake).expect("write");

    // The writer makes it 0600, so its own reader accepts it.
    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the handshake is written for this user only");
    assert!(mcp::read_handshake(&path).is_some());

    // A file somebody else loosened names a live token row, so it is not believed.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("loosen");
    assert_eq!(mcp::read_handshake(&path), None);
}

#[test]
fn the_token_listing_carries_the_prefix_and_never_the_hash() {
    // What an operator reads when deciding which token to revoke: a name and a head they can match
    // against a string they are holding. The hash is not useful to them and is not shown.
    let record = McpTokenRecord {
        id: SyncId::parse("01a0eb73-68d2-73d1-8e7a-813a293335b9").expect("an id"),
        name: "Claude Desktop".to_owned(),
        token_hash: "deadbeef".to_owned(),
        token_prefix: "qhmcp_0123abcd".to_owned(),
        scopes_json: "[\"db_drivers\"]".to_owned(),
        connections_json: "[]".to_owned(),
        expires_at: None,
        revoked_at: None,
        last_used_at: None,
        updated_at: 1,
        deleted_at: None,
        version: 1,
    };
    let listing = mcp::token_listing(&[record]);
    let row = &listing["tokens"][0];
    assert_eq!(row["prefix"], "qhmcp_0123abcd");
    assert_eq!(row["name"], "Claude Desktop");
    assert_eq!(row["scopes"], json!(["db_drivers"]));
    assert!(
        row.get("token_hash").is_none(),
        "the listing never carries the hash: {row}"
    );
}

// --------------------------------------------------------------------------- //
// windowing (DBX-14) and the qualifying names (DBX-15)
// --------------------------------------------------------------------------- //

fn rows_events(result: &Json) -> Vec<Json> {
    call_events(result)
        .into_iter()
        .filter(|event| event["event"] == json!("rows"))
        .collect()
}

#[test]
fn a_long_cell_is_cut_to_the_cell_char_limit_by_characters_and_counted() {
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);
    let (_, mysql) = ids
        .iter()
        .find(|(kind, _)| *kind == ConnectionKind::Mysql)
        .unwrap();
    // Two bytes per character: a cut on bytes would split one in half.
    let engine = ScriptedEngine::new(&"é".repeat(10_000));
    let sql = "SHOW CREATE TABLE t";

    let default = call_with(
        &server,
        &engine,
        "preview",
        json!({"connection": mysql, "sql": sql}),
    );
    let rows = rows_events(&default);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let cells: Vec<&str> = rows[0]["data"][0]
        .as_array()
        .unwrap()
        .iter()
        .map(|cell| cell.as_str().unwrap())
        .collect();
    assert!(
        cells.iter().all(|cell| cell.chars().count() == 4096),
        "default cap"
    );
    assert_eq!(rows[0]["cells_truncated"], json!(2));

    let small = call_with(
        &server,
        &engine,
        "preview",
        json!({"connection": mysql, "sql": sql, "cell_char_limit": 20}),
    );
    let rows = rows_events(&small);
    assert!(rows[0]["data"][0]
        .as_array()
        .unwrap()
        .iter()
        .all(|cell| cell.as_str().unwrap() == "é".repeat(20)));

    // A limit below the floor is raised, not honoured: a cap of one character helps nobody.
    let floor = call_with(
        &server,
        &engine,
        "preview",
        json!({"connection": mysql, "sql": sql, "cell_char_limit": 1}),
    );
    assert_eq!(
        rows_events(&floor)[0]["data"][0][0]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        16
    );

    // A cell that fits is left alone and the event carries no counter.
    let short = ScriptedEngine::new("CREATE TABLE t (id int)");
    let fits = rows_events(&call_with(
        &server,
        &short,
        "preview",
        json!({"connection": mysql, "sql": sql}),
    ));
    assert!(fits[0].get("cells_truncated").is_none(), "{fits:?}");
}

#[test]
fn a_qualifying_name_that_is_not_a_plain_name_is_refused_after_the_allowlist() {
    let (_dir, db_path, allowed_id, denied_id) = seeded_allowlisted_db();
    let mut token = full_token();
    token.connections = vec![allowed_id.clone()];
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);

    for (argument, value) in [
        ("catalog", "hive\nDROP"),
        ("schema", "a\u{0}b"),
        ("catalog", &"x".repeat(256)),
    ] {
        for tool in [
            "objects",
            "tables",
            "columns",
            "describe_table",
            "table_ddl",
        ] {
            let mut arguments = json!({"connection": allowed_id, "table": "t", "schema": "s"});
            arguments[argument] = json!(value);
            let text = error_text(&call(&server, &runtime(), tool, arguments));
            assert!(
                text.contains(&format!("the argument '{argument}' is not a valid name")),
                "{tool} {argument}: {text}"
            );
            // The refusal repeats nothing the caller sent.
            assert!(!text.contains("DROP"), "{text}");
        }
    }
    let text = error_text(&call(
        &server,
        &runtime(),
        "describe_table",
        json!({"connection": allowed_id, "table": "t\tx", "schema": "s"}),
    ));
    assert!(
        text.contains("the argument 'table' is not a valid name"),
        "{text}"
    );

    // Order: a connection outside the allowlist is refused as ever, whatever the names say, so the
    // name check cannot be used to learn anything about a connection the token may not reach.
    let text = error_text(&call(
        &server,
        &runtime(),
        "describe_table",
        json!({"connection": denied_id, "table": "t", "catalog": "bad\nname"}),
    ));
    assert!(text.contains("not allowed for this token"), "{text}");
}

#[test]
fn a_postgres_connection_cannot_be_pointed_at_another_database_through_catalog() {
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);
    let id_of = |wanted: ConnectionKind| {
        ids.iter()
            .find(|(kind, _)| *kind == wanted)
            .map(|(_, id)| id.clone())
            .unwrap()
    };
    let (pg, mysql) = (
        id_of(ConnectionKind::Postgres),
        id_of(ConnectionKind::Mysql),
    );

    for tool in [
        "tables",
        "objects",
        "columns",
        "describe_table",
        "table_ddl",
    ] {
        let engine = ScriptedEngine::new("CREATE TABLE t (id int)");
        let text = error_text(&call_with(
            &server,
            &engine,
            tool,
            json!({"connection": pg, "table": "t", "schema": "sales", "catalog": "other_db"}),
        ));
        assert!(
            text.contains("outside this token's scope"),
            "{tool}: {text}"
        );
        assert!(
            engine.statements().is_empty(),
            "{tool} reached the engine: {:?}",
            engine.statements()
        );
    }

    // The connection's own database, spelled out, is not a foreign one.
    let engine = ScriptedEngine::new("CREATE TABLE t (id int)");
    let own = call_with(
        &server,
        &engine,
        "describe_table",
        json!({"connection": pg, "table": "t", "catalog": "app"}),
    );
    assert_ne!(own["result"]["isError"], json!(true), "{own}");

    // MySQL keeps today's behaviour: `preview` already reaches any database its user can read.
    let engine = ScriptedEngine::new("CREATE TABLE t (id int)");
    let other = call_with(
        &server,
        &engine,
        "describe_table",
        json!({"connection": mysql, "table": "t", "catalog": "other_db"}),
    );
    assert_ne!(other["result"]["isError"], json!(true), "{other}");
}

#[test]
fn the_postgres_catalog_check_comes_after_the_allowlist() {
    let (_dir, db_path, ids) = metadata_db();
    let pg = ids
        .iter()
        .find(|(kind, _)| *kind == ConnectionKind::Postgres)
        .map(|(_, id)| id.clone())
        .unwrap();
    // A token that does not list the PostgreSQL connection learns nothing from the catalog check.
    let mut token = full_token();
    token.connections = ids
        .iter()
        .filter(|(_, id)| *id != pg)
        .map(|(_, id)| id.clone())
        .collect();
    let server = Server::new(token, vec![("DB_PATH".to_owned(), db_path)]);
    let engine = ScriptedEngine::new("CREATE TABLE t (id int)");
    let text = error_text(&call_with(
        &server,
        &engine,
        "describe_table",
        json!({"connection": pg, "table": "t", "catalog": "other_db"}),
    ));
    assert!(text.contains("not allowed for this token"), "{text}");
}

// --------------------------------------------------------------------------- //
// the tool surface is a baseline that only grows (DBX-60)
// --------------------------------------------------------------------------- //

/// What a client can see of the server, reduced to what a client can break on: each tool's name,
/// `required` and property names, each prompt's name and argument names, and the resource shapes
/// with the connection id left as `{id}`. No descriptions: those may be clarified (docs/mcp-stability.md).
fn surface() -> Json {
    let (_dir, db_path, ids) = metadata_db();
    let server = Server::new(allowing(&ids), vec![("DB_PATH".to_owned(), db_path)]);
    let runtime = runtime();
    let tools = request(
        &server,
        &runtime,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let prompts = request(
        &server,
        &runtime,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "prompts/list"}),
    );
    let resources = request(
        &server,
        &runtime,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}),
    );

    let sorted = |mut names: Vec<String>| {
        names.sort();
        names.dedup();
        Json::from(names)
    };
    let mut tool_map = serde_json::Map::new();
    for tool in tools["result"]["tools"].as_array().unwrap() {
        let schema = &tool["inputSchema"];
        let required: Vec<String> = schema["required"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .map(|n| n.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let properties: Vec<String> = schema["properties"]
            .as_object()
            .map(|props| props.keys().cloned().collect())
            .unwrap_or_default();
        tool_map.insert(
            tool["name"].as_str().unwrap().to_owned(),
            json!({"required": sorted(required), "properties": sorted(properties)}),
        );
    }
    let mut prompt_map = serde_json::Map::new();
    for prompt in prompts["result"]["prompts"].as_array().unwrap() {
        let arguments = prompt["arguments"].as_array().unwrap();
        let required: Vec<String> = arguments
            .iter()
            .filter(|a| a["required"] == json!(true))
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect();
        let all: Vec<String> = arguments
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect();
        prompt_map.insert(
            prompt["name"].as_str().unwrap().to_owned(),
            json!({"required": sorted(required), "arguments": sorted(all)}),
        );
    }
    let mut uris: Vec<String> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| {
            let mut uri = resource["uri"].as_str().unwrap().to_owned();
            for (_, id) in &ids {
                uri = uri.replace(id.as_str(), "{id}");
            }
            uri
        })
        .collect();
    uris.sort();
    uris.dedup();
    json!({"tools": tool_map, "prompts": prompt_map, "resources": uris})
}

fn surface_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mcp_surface.json")
}

fn names(value: &Json) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect()
}

#[test]
fn nothing_a_client_could_rely_on_has_been_removed_or_made_required() {
    let current = surface();
    if std::env::var("QH_RECORD_MCP_SURFACE").as_deref() == Ok("1") {
        std::fs::write(
            surface_path(),
            serde_json::to_string_pretty(&current).unwrap() + "\n",
        )
        .expect("record the baseline");
        return;
    }
    let baseline: Json =
        serde_json::from_str(&std::fs::read_to_string(surface_path()).expect("the baseline"))
            .expect("the baseline is JSON");

    for (name, was) in baseline["tools"].as_object().unwrap() {
        let now = current["tools"]
            .get(name)
            .unwrap_or_else(|| panic!("tool '{name}' was removed or renamed"));
        for property in names(&was["properties"]) {
            assert!(
                names(&now["properties"]).contains(&property),
                "{name}: property '{property}' was removed"
            );
        }
        for required in names(&now["required"]) {
            assert!(
                names(&was["required"]).contains(&required),
                "{name}: '{required}' became required"
            );
        }
    }
    for (name, was) in baseline["prompts"].as_object().unwrap() {
        let now = current["prompts"]
            .get(name)
            .unwrap_or_else(|| panic!("prompt '{name}' was removed or renamed"));
        for argument in names(&was["arguments"]) {
            assert!(
                names(&now["arguments"]).contains(&argument),
                "{name}: argument '{argument}' was removed"
            );
        }
        for required in names(&now["required"]) {
            assert!(
                names(&was["required"]).contains(&required),
                "{name}: '{required}' became required"
            );
        }
    }
    for uri in names(&baseline["resources"]) {
        assert!(
            names(&current["resources"]).contains(&uri),
            "resource '{uri}' was removed"
        );
    }
    // The baseline is only ever grown on purpose: a new tool, property or resource passes the
    // check above and shows up here, which is the prompt to re-record it
    // (`QH_RECORD_MCP_SURFACE=1 cargo test -p qh-ffi --test mcp nothing_a_client`).
    if current != baseline {
        eprintln!("the MCP surface grew; re-record tests/mcp_surface.json (it may only grow)");
    }
}

#[test]
fn the_baseline_check_has_teeth() {
    // A baseline that asks for something the server does not have must fail the same comparison,
    // otherwise the check above could never fail.
    let current = surface();
    let mut baseline = current.clone();
    baseline["tools"]["preview"]["properties"] =
        json!(["connection", "sql", "a_property_that_was_removed"]);
    let removed = names(&baseline["tools"]["preview"]["properties"])
        .into_iter()
        .any(|property| !names(&current["tools"]["preview"]["properties"]).contains(&property));
    assert!(removed, "the comparison cannot see a removed property");
}
