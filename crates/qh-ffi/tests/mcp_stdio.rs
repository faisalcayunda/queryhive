//! The MCP server end to end, over the real stdio transport.
//!
//! This is the test the brief asks for and the one a protocol unit test cannot replace:
//! it spawns the actual `queryhive-mcp` binary, has it issue a token into a temporary
//! database through its own `issue` subcommand, then starts a second process of the same
//! binary in serve mode and writes JSON-RPC lines at its stdin.
//!
//! # The client is hand-rolled, and that is deliberate
//!
//! There is no third-party MCP client crate in this workspace, so the client here is
//! about forty lines of `write!`/`read_line`. That is enough to prove the transport —
//! newline-delimited framing, one reply per request, notifications silent — without
//! adding a dependency whose version would have to be kept current for one test. What it
//! does not prove is that a particular client's own schema handling accepts the tool
//! descriptions; only a real client can say that.
//!
//! # Nothing here touches the network or the Keychain
//!
//! The tools exercised (`db_drivers`, `connections_list`) read no driver and no password,
//! so the test is hermetic. The live acceptance against Trino is a separate, measured
//! run, recorded in `PROGRESS.md`; it is not this test.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use qh_storage::{ConnectionKind, ConnectionRecord, Storage};
use serde_json::{json, Value as Json};

const BIN: &str = env!("CARGO_BIN_EXE_queryhive-mcp");

/// The temporary database, one seeded connection, and that connection's id.
fn seeded_database() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().expect("a temp directory");
    let path = dir.path().join("queryhive.sqlite3");
    let mut storage = Storage::open(&path).expect("open");
    storage.migrate().expect("migrate");

    let mut connection = ConnectionRecord::new("E2E", ConnectionKind::Trino, 1_700_000_000_000);
    connection.host = Some("trino.internal".to_owned());
    connection.port = Some(8443);
    connection.database_name = Some("hive".to_owned());
    connection.user_name = Some("e2e-user".to_owned());
    connection.options_json = r#"{"scheme":"https","verify":true}"#.to_owned();
    connection.secret_ref = Some(connection.meta.id.to_string());
    storage.save_connection(&connection).expect("save");

    (
        dir,
        path.to_string_lossy().into_owned(),
        connection.meta.id.to_string(),
    )
}

/// Issue a token through the binary's own `issue` subcommand.
fn issue_token(db_path: &str, connection_id: &str) -> (String, String) {
    let output = Command::new(BIN)
        .args([
            "issue",
            "--name",
            "e2e",
            "--scope",
            "db_drivers,connections_list,explain",
            "--connection",
            connection_id,
        ])
        .env("DB_PATH", db_path)
        .output()
        .expect("run issue");
    assert!(
        output.status.success(),
        "issue failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed: Json = serde_json::from_slice(&output.stdout).expect("issue prints a JSON object");
    let token = printed["token"].as_str().expect("a token").to_owned();
    let id = printed["id"].as_str().expect("an id").to_owned();
    // The one-time token must not be recoverable from `list`.
    let list = Command::new(BIN)
        .arg("list")
        .env("DB_PATH", db_path)
        .output()
        .expect("run list");
    let listing = String::from_utf8_lossy(&list.stdout);
    assert!(
        !listing.contains(&token),
        "the token string reached `list`: {listing}"
    );
    assert!(listing.contains(&id), "the token's id is listed: {listing}");
    (token, id)
}

#[test]
fn the_stdio_server_answers_over_a_real_pipe_and_cleans_up_its_handshake() {
    let (_dir, db_path, connection_id) = seeded_database();
    let (token, _id) = issue_token(&db_path, &connection_id);
    let handshake = _dir.path().join("mcp-handshake.json");

    let mut child = Command::new(BIN)
        .env("DB_PATH", &db_path)
        .env("QH_MCP_TOKEN", &token)
        .env("QH_MCP_HANDSHAKE", &handshake)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the server");

    // The handshake is written on startup; poll briefly rather than assume the scheduler
    // has already run the child.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !handshake.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let written = qh_ffi::mcp::read_handshake(&handshake).expect("a live handshake");
    assert_eq!(written.pid, child.id());

    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
               "params": {"name": "db_drivers", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
               "params": {"name": "connections_list", "arguments": {}}}),
        // Refused by name, before the scope is even consulted.
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
               "params": {"name": "to_table", "arguments": {"connection": connection_id, "sql": "select 1"}}}),
        // In the registry but outside this token's scope.
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
               "params": {"name": "preview", "arguments": {"connection": connection_id, "sql": "select 1"}}}),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
               "params": {"name": "no-such-tool", "arguments": {}}}),
        // A revision nobody speaks is refused with the supported list, not echoed.
        json!({"jsonrpc": "2.0", "id": 8, "method": "initialize",
               "params": {"protocolVersion": "1999-01-01"}}),
        json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"}),
        json!({"jsonrpc": "2.0", "id": 10, "method": "resources/read",
               "params": {"uri": format!("queryhive://connections/{connection_id}")}}),
        json!({"jsonrpc": "2.0", "id": 11, "method": "prompts/list"}),
        // A tool that runs caller SQL under the server's own `read_only`: refused before any
        // connection, and — the point of this request — recorded by the binary's log sink.
        json!({"jsonrpc": "2.0", "id": 12, "method": "tools/call",
               "params": {"name": "explain", "arguments": {"connection": connection_id, "sql": "DROP TABLE people"}}}),
    ];

    {
        let mut stdin = child.stdin.take().expect("stdin");
        for request in &requests {
            writeln!(stdin, "{request}").expect("write");
        }
        stdin.flush().expect("flush");
    }

    // Twelve replies: the notification is silent, the other twelve requests each answer.
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut replies: Vec<Json> = Vec::new();
    for _ in 0..12 {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("read");
        assert!(read > 0, "the server closed stdout early");
        replies.push(serde_json::from_str(line.trim()).expect("a JSON reply"));
    }

    assert_eq!(replies[0]["id"], json!(1));
    assert_eq!(
        replies[0]["result"]["serverInfo"]["name"],
        json!("queryhive-mcp")
    );
    assert_eq!(replies[1]["id"], json!(2));
    assert_eq!(replies[2]["id"], json!(3));
    assert_eq!(replies[2]["result"]["isError"], json!(false));

    // connections_list: the allowlisted row, with the user name left out.
    assert_eq!(replies[3]["id"], json!(4));
    let text = replies[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(text.contains("E2E"), "{text}");
    assert!(
        !text.contains("e2e-user"),
        "a credential field leaked: {text}"
    );

    // to_table and preview are both refused, in their own words.
    assert_eq!(replies[4]["result"]["isError"], json!(true));
    assert!(replies[4]["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("not available over MCP"));
    assert_eq!(replies[5]["result"]["isError"], json!(true));
    assert!(replies[5]["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("outside this token's scope"));
    assert_eq!(replies[6]["result"]["isError"], json!(true));
    assert!(replies[6]["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("unknown tool"));

    // An unknown protocol revision is refused with the supported list, not echoed.
    assert_eq!(replies[7]["id"], json!(8));
    assert_eq!(replies[7]["error"]["code"], json!(-32022));
    assert!(
        replies[7]["error"]["data"]["supported"]
            .as_array()
            .is_some_and(|versions| !versions.is_empty()),
        "{:?}",
        replies[7]
    );

    // resources/list offers the aggregate and the allowlisted connection, and nothing
    // for a connection this token may not reach.
    assert_eq!(replies[8]["id"], json!(9));
    let uris: Vec<&str> = replies[8]["result"]["resources"]
        .as_array()
        .expect("a resources array")
        .iter()
        .map(|resource| resource["uri"].as_str().expect("a uri"))
        .collect();
    assert!(uris.contains(&"queryhive://connections"), "{uris:?}");
    let connection_uri = format!("queryhive://connections/{connection_id}");
    assert!(uris.contains(&connection_uri.as_str()), "{uris:?}");

    // resources/read of that connection answers, with no credential field.
    assert_eq!(replies[9]["id"], json!(10));
    let resource_text = replies[9]["result"]["contents"][0]["text"]
        .as_str()
        .expect("a text block");
    assert!(resource_text.contains("E2E"), "{resource_text}");
    assert!(
        !resource_text.contains("e2e-user"),
        "a credential field leaked: {resource_text}"
    );
    assert!(
        !resource_text.contains("secret_ref"),
        "a credential field leaked: {resource_text}"
    );

    // prompts/list answers; `explain` is in this token's scope.
    assert_eq!(replies[10]["id"], json!(11));
    let prompts = replies[10]["result"]["prompts"]
        .as_array()
        .expect("a prompts array");
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == json!("explain_query")),
        "{prompts:?}"
    );

    // The `explain` of a `DROP` is refused by the server's own `read_only`, before it would
    // connect: a tool result, not a protocol error.
    assert_eq!(replies[11]["id"], json!(12));
    assert_eq!(replies[11]["result"]["isError"], json!(true));
    assert!(
        replies[11]["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("read-only"),
        "{:?}",
        replies[11]
    );

    // Closing stdin is a clean end of the stream: exit 0, handshake gone.
    drop(reader);
    let status = child.wait().expect("wait");
    assert!(status.success(), "the server exited with {status:?}");
    assert!(!handshake.exists(), "the handshake was not removed");
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    assert!(
        !stderr.contains(&token),
        "the token reached stderr: {stderr}"
    );

    // The server wrote its own decision to the execution log: the sink is installed by the
    // binary (`src/bin/mcp.rs`), not by the library, so this is the end-to-end proof of that
    // wiring. Only the refused `explain` is a `guard` decision; the other tools never guard.
    let storage = Storage::open(&db_path).expect("open the log database");
    let decisions = storage.execution_log(10).expect("read the execution log");
    assert_eq!(
        decisions.len(),
        1,
        "one guarded call, one decision: {decisions:?}"
    );
    assert_eq!(decisions[0].decision, "refused");
    assert_eq!(decisions[0].statement_kind, "ddl");
    assert_eq!(decisions[0].safe_mode, "read_only");
    assert_eq!(storage.verify_execution_log().unwrap(), 1);

    // The call was recorded: `list` now shows a last-used time.
    let list = Command::new(BIN)
        .arg("list")
        .env("DB_PATH", &db_path)
        .output()
        .expect("run list");
    let listing: Json = serde_json::from_slice(&list.stdout).expect("list JSON");
    let used = &listing["tokens"][0]["last_used_at"];
    assert!(!used.is_null(), "last_used_at was not recorded: {listing}");
}

/// `issue` with the given extra flags, and the token it printed.
fn issue_with(db_path: &str, flags: &[&str]) -> String {
    let output = Command::new(BIN)
        .args(["issue", "--name", "metadata"])
        .args(flags)
        .env("DB_PATH", db_path)
        .output()
        .expect("run issue");
    assert!(
        output.status.success(),
        "issue failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed: Json = serde_json::from_slice(&output.stdout).expect("issue prints a JSON object");
    printed["token"].as_str().expect("a token").to_owned()
}

/// Serve one process over a pipe: write every request, close stdin, and return the replies. The
/// handshake goes to the temporary directory, never to the owner's Application Support.
fn serve_once(dir: &std::path::Path, db_path: &str, token: &str, requests: &[Json]) -> Vec<Json> {
    let mut child = Command::new(BIN)
        .env("DB_PATH", db_path)
        .env("QH_MCP_TOKEN", token)
        .env("QH_MCP_HANDSHAKE", dir.join("mcp-handshake.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the server");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write");
        }
    }
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "{:?}", output.status);
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("every stdout line is one JSON reply"))
        .collect()
}

/// W11-T5: the two metadata tools are in a default-scope token, and an empty allowlist still
/// refuses them, without saying whether the connection is real, over the real transport.
#[test]
fn the_metadata_tools_are_in_the_default_scope_and_an_empty_allowlist_refuses_them() {
    let (dir, db_path, connection_id) = seeded_database();
    // No --scope: every read-only tool. No --connection: no connection at all.
    let token = issue_with(&db_path, &[]);
    let arguments = |id: &str| json!({"connection": id, "table": "t", "schema": "s"});
    let replies = serve_once(
        dir.path(),
        &db_path,
        &token,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "describe_table", "arguments": arguments(&connection_id)}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "table_ddl", "arguments": arguments(&connection_id)}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                   "params": {"name": "table_ddl",
                              "arguments": arguments("99999999-9999-7999-8999-999999999999")}}),
        ],
    );
    assert_eq!(replies.len(), 4, "{replies:?}");

    let names: Vec<&str> = replies[0]["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names.len(), 11, "{names:?}");
    assert!(
        names.contains(&"describe_table") && names.contains(&"table_ddl"),
        "{names:?}"
    );

    // The real row and the made-up id get the same sentence, apart from the id itself.
    let texts: Vec<String> = replies[1..]
        .iter()
        .map(|reply| {
            assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
            reply["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .to_owned()
        })
        .collect();
    for text in &texts {
        assert!(text.contains("not allowed for this token"), "{text}");
        assert!(!text.contains("was not found"), "{text}");
    }
    assert_eq!(
        texts[1].replace(&connection_id, "<id>"),
        texts[2].replace("99999999-9999-7999-8999-999999999999", "<id>")
    );
}

/// A token scoped to one of the two cannot call the other, and the refusal says why.
#[test]
fn a_token_scoped_to_one_metadata_tool_cannot_call_the_other() {
    let (dir, db_path, connection_id) = seeded_database();
    let token = issue_with(
        &db_path,
        &["--scope", "describe_table", "--connection", &connection_id],
    );
    let replies = serve_once(
        dir.path(),
        &db_path,
        &token,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "table_ddl",
                              "arguments": {"connection": connection_id, "table": "t", "schema": "s"}}}),
        ],
    );
    let names: Vec<&str> = replies[0]["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, vec!["describe_table"]);
    assert_eq!(replies[1]["result"]["isError"], json!(true));
    assert!(replies[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("outside this token's scope"));
}

#[test]
fn a_revoked_token_is_refused_at_startup() {
    let (_dir, db_path, connection_id) = seeded_database();
    let (token, id) = issue_token(&db_path, &connection_id);

    let revoke = Command::new(BIN)
        .args(["revoke", "--id", &id])
        .env("DB_PATH", &db_path)
        .output()
        .expect("run revoke");
    assert!(
        revoke.status.success(),
        "revoke failed: {}",
        String::from_utf8_lossy(&revoke.stderr)
    );

    let output = Command::new(BIN)
        .env("DB_PATH", &db_path)
        .env("QH_MCP_TOKEN", &token)
        .env("QH_MCP_HANDSHAKE", _dir.path().join("handshake.json"))
        .stdin(Stdio::null())
        .output()
        .expect("run the server");
    assert!(!output.status.success(), "a revoked token must not serve");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("revoked"), "{stderr}");
    assert!(
        !stderr.contains(&token),
        "the token reached stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing but protocol goes to stdout"
    );
}

#[test]
fn a_missing_token_is_refused_at_startup() {
    let (_dir, db_path, _connection_id) = seeded_database();
    let output = Command::new(BIN)
        .env("DB_PATH", &db_path)
        .env_remove("QH_MCP_TOKEN")
        .stdin(Stdio::null())
        .output()
        .expect("run the server");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no MCP token"), "{stderr}");
    assert!(output.stdout.is_empty());
}
