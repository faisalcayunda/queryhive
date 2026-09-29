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

use std::time::Duration;

use qh_ffi::mcp::{self, Handshake, McpToken, Server};
use qh_ffi::RealEngine;
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
    assert_eq!(names.len(), 9, "the read-only set is nine tools");
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
        password,
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
    let env = pairs_to_json(mcp::connection_environment(&bare, &json!({}), ""));
    assert_eq!(env["DB_PORT"], json!("3306"));
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
