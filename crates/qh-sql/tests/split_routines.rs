//! W12-T7b: one splitter for a `.sql` script.
//!
//! * a stored program's body is one statement under MySQL's lexers, whatever `;` it holds, and a
//!   construct the tracker could close early is pinned by its own case, because a body that ends
//!   early runs its tail as separate statements;
//! * a plain script splits exactly as before, for every dialect;
//! * a client directive (`DELIMITER`, a psql backslash line, `COPY … FROM STDIN`) is named, with
//!   its line, and text that only looks like one is not.

use qh_sql::{
    check_dialect, client_directive, scan_dialect, statements_agreeing,
    statements_with_lines_dialect, Dialect, Lexer, SafeMode,
};

/// The statements under MySQL's default reading, and a check that all six readings agree.
fn mysql(sql: &str) -> Vec<String> {
    let found: Vec<String> = statements_with_lines_dialect(sql, Dialect::Mysql)
        .into_iter()
        .map(|statement| statement.text.to_owned())
        .collect();
    let agreed = statements_agreeing(sql, Dialect::Mysql.readings());
    assert_eq!(
        agreed
            .as_ref()
            .map(|all| all.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()),
        Some(found.clone()),
        "the six MySQL readings split {sql:?} differently"
    );
    found
}

#[test]
fn a_procedure_body_with_two_deletes_is_one_statement() {
    let sql = "CREATE PROCEDURE p() BEGIN DELETE FROM a; DELETE FROM b; END;\nSELECT 1;";
    assert_eq!(
        mysql(sql),
        [
            "CREATE PROCEDURE p() BEGIN DELETE FROM a; DELETE FROM b; END",
            "SELECT 1"
        ]
    );
}

#[test]
fn the_statement_after_a_body_keeps_its_own_line() {
    let sql = "SELECT 1;\nCREATE PROCEDURE p()\nBEGIN\n  DELETE FROM a;\n  DELETE FROM b;\nEND;\n\nSELECT 2;\n";
    let lines: Vec<usize> = statements_with_lines_dialect(sql, Dialect::Mysql)
        .iter()
        .map(|statement| statement.line)
        .collect();
    assert_eq!(lines, [1, 2, 8]);
}

#[test]
fn a_simple_body_ends_at_its_own_semicolon() {
    assert_eq!(
        mysql("CREATE FUNCTION f() RETURNS INT DETERMINISTIC RETURN 1; DELETE FROM t; SELECT 2"),
        [
            "CREATE FUNCTION f() RETURNS INT DETERMINISTIC RETURN 1",
            "DELETE FROM t",
            "SELECT 2"
        ]
    );
    assert_eq!(
        mysql("CREATE PROCEDURE p() DELETE FROM a; DELETE FROM b"),
        ["CREATE PROCEDURE p() DELETE FROM a", "DELETE FROM b"]
    );
    // `CASE` and `IF(` inside a simple body are expressions, not blocks.
    assert_eq!(
        mysql(
            "CREATE FUNCTION f(x INT) RETURNS INT RETURN CASE WHEN x > 1 THEN IF(x > 2, 3, 2) \
             ELSE 0 END; DELETE FROM t"
        )
        .len(),
        2
    );
}

#[test]
fn every_block_kind_closes_where_the_server_closes_it() {
    let body = "\
CREATE PROCEDURE p(IN n INT)
main: BEGIN
  DECLARE i INT DEFAULT 0;
  IF n > 1 THEN
    DELETE FROM a;
    IF n > 2 THEN DELETE FROM b; ELSEIF n = 2 THEN DELETE FROM c; ELSE DELETE FROM d; END IF;
  ELSE
    DELETE FROM e;
  END IF;
  CASE n WHEN 1 THEN DELETE FROM f; WHEN 2 THEN DELETE FROM g; ELSE DELETE FROM h; END CASE;
  CASE WHEN n > 5 THEN DELETE FROM i; END CASE;
  WHILE i < 3 DO
    SET i = i + 1;
    DELETE FROM j;
  END WHILE;
  REPEAT
    SET i = i - 1;
    DELETE FROM k;
  UNTIL i <= 0 END REPEAT;
  lp: LOOP
    DELETE FROM l;
    LEAVE lp;
  END LOOP lp;
  SET @x = CASE WHEN i > 0 THEN 'a' ELSE 'b' END;
  UPDATE t SET end = IF(i > 0, 1, 2), begin = 3;
  SELECT CASE i WHEN 1 THEN 'x' END AS c;
END main";
    let sql = format!("{body};\nDELETE FROM after;");
    let found = mysql(&sql);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(found[0], body);
    assert_eq!(found[1], "DELETE FROM after");
}

#[test]
fn a_handler_with_a_block_does_not_close_the_outer_one() {
    for condition in [
        "SQLEXCEPTION",
        "SQLWARNING",
        "NOT FOUND",
        "SQLSTATE '23000'",
        "SQLSTATE VALUE '23000'",
        "1062",
        "1062, 1048",
        "SQLEXCEPTION, SQLWARNING",
        "my_condition",
        "NOT FOUND, 1062",
    ] {
        let body = format!(
            "CREATE PROCEDURE p() BEGIN \
             DECLARE EXIT HANDLER FOR {condition} BEGIN ROLLBACK; DELETE FROM err; END; \
             DELETE FROM a; DELETE FROM b; END"
        );
        let found = mysql(&format!("{body}; SELECT 1"));
        assert_eq!(found, [body.as_str(), "SELECT 1"], "{condition}");
    }
    // A simple action ends at its `;` and the block goes on.
    let simple = "CREATE PROCEDURE p() BEGIN DECLARE CONTINUE HANDLER FOR NOT FOUND SET done = 1; \
                  DELETE FROM a; END";
    assert_eq!(mysql(&format!("{simple}; SELECT 1")).len(), 2);
    // An action that is itself an `IF(…)` call is not a block.
    let call = "CREATE PROCEDURE p() BEGIN DECLARE CONTINUE HANDLER FOR SQLEXCEPTION \
                SET e = IF(1, 2, 3); DELETE FROM a; END";
    assert_eq!(mysql(&format!("{call}; SELECT 1")).len(), 2);
    // A cursor named like the keyword is not a handler.
    let cursor = "CREATE PROCEDURE p() BEGIN DECLARE handler CURSOR FOR SELECT IF(1, 2, 3); \
                  DELETE FROM a; END";
    assert_eq!(mysql(&format!("{cursor}; SELECT 1")).len(), 2);
}

#[test]
fn parameters_strings_and_comments_cannot_fake_a_block_boundary() {
    let sql = "\
CREATE DEFINER = 'root'@'localhost' PROCEDURE `p`(IN begin DATE, IN end DATE, k ENUM('a)', 'end;'))
  COMMENT 'BEGIN; END;' NOT DETERMINISTIC SQL SECURITY INVOKER
BEGIN
  -- END; DELETE FROM never;
  /* END; */
  SELECT 'END;', \"BEGIN;\", `end`;
  DELETE FROM a;
END";
    let found = mysql(&format!("{sql}; SELECT 1"));
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(found[0], sql);
}

#[test]
fn a_function_header_with_a_return_type_before_the_body() {
    for header in [
        "CREATE FUNCTION f(a INT) RETURNS VARCHAR(10) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin \
         DETERMINISTIC",
        "CREATE FUNCTION f(a INT) RETURNS DECIMAL(10, 2) UNSIGNED NO SQL",
        "CREATE FUNCTION IF NOT EXISTS db.f() RETURNS ENUM('a', 'b') READS SQL DATA",
        "CREATE DEFINER = CURRENT_USER() FUNCTION f() RETURNS INT",
        "CREATE DEFINER = root@localhost FUNCTION f() RETURNS INT",
        "CREATE DEFINER = `r`@`%` FUNCTION f() RETURNS INT LANGUAGE SQL",
        "CREATE AGGREGATE FUNCTION f() RETURNS INT",
    ] {
        let body = format!("{header} BEGIN DELETE FROM a; RETURN 1; END");
        assert_eq!(
            mysql(&format!("{body}; SELECT 1")),
            [body.as_str(), "SELECT 1"],
            "{header}"
        );
    }
}

#[test]
fn triggers_and_events_carry_their_bodies() {
    let trigger = "CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN \
                   SET NEW.a = 1; DELETE FROM log; END";
    assert_eq!(
        mysql(&format!("{trigger}; SELECT 1")),
        [trigger, "SELECT 1"]
    );
    let ordered = "CREATE TRIGGER IF NOT EXISTS t AFTER UPDATE ON x FOR EACH ROW FOLLOWS other \
                   BEGIN DELETE FROM a; DELETE FROM b; END";
    assert_eq!(
        mysql(&format!("{ordered}; SELECT 1")),
        [ordered, "SELECT 1"]
    );
    // A trigger with one statement for a body ends there.
    assert_eq!(
        mysql("CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW SET NEW.a = 1; DELETE FROM t")
            .len(),
        2
    );
    let event = "CREATE EVENT e ON SCHEDULE EVERY 1 DAY COMMENT 'do it' DO BEGIN \
                 DELETE FROM a; DELETE FROM b; END";
    assert_eq!(mysql(&format!("{event}; SELECT 1")), [event, "SELECT 1"]);
    let alter = "ALTER EVENT e ON SCHEDULE EVERY 2 DAY DO BEGIN DELETE FROM a; DELETE FROM b; END";
    assert_eq!(mysql(&format!("{alter}; SELECT 1")), [alter, "SELECT 1"]);
    // `ALTER` without a body keeps splitting as before.
    assert_eq!(mysql("ALTER EVENT e DISABLE; DELETE FROM t").len(), 2);
    assert_eq!(mysql("ALTER TABLE t ADD c INT; DELETE FROM t").len(), 2);
}

#[test]
fn a_bare_begin_is_a_transaction_and_splits_as_before() {
    assert_eq!(
        mysql("BEGIN; DELETE FROM t; COMMIT; BEGIN WORK; DELETE FROM u; COMMIT"),
        [
            "BEGIN",
            "DELETE FROM t",
            "COMMIT",
            "BEGIN WORK",
            "DELETE FROM u",
            "COMMIT"
        ]
    );
    assert_eq!(
        mysql("START TRANSACTION; DELETE FROM t; COMMIT").len(),
        3,
        "START TRANSACTION is not a block"
    );
    // MariaDB's anonymous block.
    let anonymous = "BEGIN NOT ATOMIC DELETE FROM a; DELETE FROM b; END";
    assert_eq!(
        mysql(&format!("{anonymous}; SELECT 1")),
        [anonymous, "SELECT 1"]
    );
}

#[test]
fn a_mysqldump_routine_in_executable_comments_is_one_statement() {
    // The body of a dump's routine, with the `DELIMITER` lines already gone.
    let sql = "/*!50003 CREATE*/ /*!50020 DEFINER=`root`@`localhost`*/ /*!50003 PROCEDURE `p`()\n\
               BEGIN\n  DELETE FROM a;\n  DELETE FROM b;\nEND */;\nSELECT 1;";
    let found: Vec<_> = statements_with_lines_dialect(sql, Dialect::Mysql)
        .into_iter()
        .map(|statement| statement.text)
        .collect();
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(found[0].ends_with("END */"), "{}", found[0]);
    assert_eq!(found[1], "SELECT 1");
}

#[test]
fn a_body_that_never_closes_swallows_the_rest_and_nothing_runs_in_pieces() {
    let sql = "CREATE PROCEDURE p() BEGIN DELETE FROM a; DELETE FROM b;";
    // The `;` after the last `DELETE` is body text, so it stays.
    assert_eq!(mysql(sql), [sql]);
    // A header the tracker does not know splits as before.
    assert_eq!(
        mysql("CREATE FUNCTION f RETURNS STRING SONAME 'x.so'; DELETE FROM t").len(),
        2
    );
    assert_eq!(
        mysql("CREATE TABLE t (a INT); CREATE VIEW v AS SELECT 1; DROP TABLE t").len(),
        3
    );
}

#[test]
fn a_swallowed_remainder_is_refused_by_every_mode_below_full() {
    // The statement that holds the rest of the file begins with `CREATE`.
    let sql = "CREATE PROCEDURE p() BEGIN SELECT 1; DELETE FROM never_ends;";
    for mode in [SafeMode::NoDdl, SafeMode::Confirm, SafeMode::ReadOnly] {
        assert!(
            check_dialect(mode, sql, Dialect::Mysql).is_err(),
            "{mode:?}"
        );
    }
    assert!(check_dialect(SafeMode::Full, sql, Dialect::Mysql).is_ok());
}

#[test]
fn other_dialects_split_exactly_as_before() {
    let sql = "CREATE PROCEDURE p() BEGIN DELETE FROM a; DELETE FROM b; END; SELECT 1";
    for dialect in [Dialect::Generic, Dialect::Postgres, Dialect::Trino] {
        let found = statements_with_lines_dialect(sql, dialect);
        assert_eq!(found.len(), 4, "{dialect:?}");
    }
}

/// SplitMix64, as in `scan_refactor`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[test]
fn random_scripts_never_panic_and_only_ever_merge_pieces() {
    const PIECES: &[&str] = &[
        "CREATE ",
        "ALTER ",
        "PROCEDURE ",
        "FUNCTION ",
        "TRIGGER ",
        "EVENT ",
        "DEFINER ",
        "= ",
        "@ ",
        "p() ",
        "( ",
        ") ",
        ", ",
        "BEGIN ",
        "END ",
        "IF ",
        "THEN ",
        "ELSE ",
        "ELSEIF ",
        "CASE ",
        "WHEN ",
        "WHILE ",
        "DO ",
        "LOOP ",
        "REPEAT ",
        "UNTIL ",
        "DECLARE ",
        "HANDLER ",
        "FOR ",
        "EACH ",
        "ROW ",
        "NOT ",
        "ATOMIC ",
        "FOUND ",
        "SQLSTATE ",
        "VALUE ",
        "1062 ",
        "RETURNS ",
        "INT ",
        "RETURN ",
        "label: ",
        "; ",
        "; ",
        "; ",
        "SELECT ",
        "DELETE ",
        "t ",
        "'",
        "\"",
        "`",
        "/*",
        "*/",
        "-- ",
        "#",
        "\n",
        "/*!50003 ",
        "$$ ",
        "\\",
    ];
    let mut rng = Rng(0x7_7B);
    let readings: Vec<Lexer> = Dialect::Mysql.readings().to_vec();
    for _ in 0..20_000 {
        let mut sql = String::new();
        for _ in 0..(rng.next() % 40) {
            sql.push_str(PIECES[(rng.next() % PIECES.len() as u64) as usize]);
        }
        for &lexer in &readings {
            let found = statements_with_lines_dialect(&sql, lexer);
            // Raw pieces: every statement of the gated split is one raw piece or several of them
            // joined, never a part of one, and never more of them.
            let raw = scan_dialect(&sql, lexer).separators;
            assert!(found.len() <= raw.len() + 1, "{sql:?}");
            let mut cursor = 0;
            for statement in &found {
                let at = sql[cursor..]
                    .find(statement.text)
                    .unwrap_or_else(|| panic!("{statement:?} is not text of {sql:?}"));
                cursor += at + statement.text.len();
            }
        }
    }
}

// ---- client directives ----------------------------------------------------------------------

#[test]
fn a_delimiter_line_is_found_by_line_not_by_statement() {
    // `USE db` and `\G` need no `;`, so the DELIMITER line is inside a statement, not at its start.
    let routine = "CREATE PROCEDURE p() BEGIN DELETE FROM a; DELETE FROM b; END$$\nDELIMITER ;\n";
    let after_use = format!("USE db\nDELIMITER $$\n{routine}");
    assert_eq!(
        refusal(&after_use, Dialect::Mysql),
        Some((2, "DELIMITER".into()))
    );
    let after_g = format!("SELECT 1\\G\nDELIMITER ;;\n{routine}");
    assert_eq!(
        refusal(&after_g, Dialect::Mysql),
        Some((2, "DELIMITER".into()))
    );
    // One inside a body the tracker swallowed is found too.
    let inside = "CREATE PROCEDURE p() BEGIN SELECT 1;\n  delimiter //\nSELECT 2; END";
    assert_eq!(
        refusal(inside, Dialect::Mysql),
        Some((2, "DELIMITER".into()))
    );
    // Not a line start, a longer word, or text.
    for text in [
        "SELECT delimiter FROM t",
        "SELECT 1,\n  t.delimiter FROM t",
        "SELECT\n  delimiter_x FROM t",
        "SELECT\n  `delimiter` FROM t",
        "SELECT '\nDELIMITER ;;\n'",
        "SELECT 1 /*\nDELIMITER ;;\n*/",
    ] {
        assert_eq!(refusal(text, Dialect::Mysql), None, "{text}");
    }
    // A column named delimiter that starts a line fails closed, and the message says how out.
    let column = "SELECT a,\n  delimiter FROM t";
    let message = client_directive(column, Dialect::Mysql)
        .unwrap()
        .to_string();
    assert!(message.contains("backticks"), "{message}");
}

fn refusal(sql: &str, dialect: Dialect) -> Option<(usize, String)> {
    client_directive(sql, dialect).map(|refusal| (refusal.line, refusal.directive))
}

#[test]
fn a_delimiter_line_is_named_with_its_line() {
    let sql = "SET NAMES utf8mb4;\nDROP TABLE IF EXISTS t;\nDELIMITER ;;\nCREATE PROCEDURE p() BEGIN SELECT 1; END ;;\nDELIMITER ;\n";
    assert_eq!(refusal(sql, Dialect::Mysql), Some((3, "DELIMITER".into())));
    let lower = "select 1;\n  delimiter //\nselect 2//\n";
    assert_eq!(
        refusal(lower, Dialect::Mysql),
        Some((2, "DELIMITER".into()))
    );
    let message = client_directive(sql, Dialect::Mysql).unwrap().to_string();
    assert!(message.starts_with("line 3: `DELIMITER`"), "{message}");
    // Only a line that *starts* with it is the client command; the word elsewhere is not.
    assert_eq!(refusal("SELECT delimiter FROM t", Dialect::Mysql), None);
    assert_eq!(refusal("SELECT 'DELIMITER ;;'", Dialect::Mysql), None);
    assert_eq!(refusal("-- DELIMITER ;;\nSELECT 1", Dialect::Mysql), None);
    // It is mysql's command and no other dialect's.
    assert_eq!(refusal(sql, Dialect::Postgres), None);
}

#[test]
fn pg_dump_restrict_lines_are_named_not_glued_to_a_statement() {
    let sql = "--\n-- PostgreSQL database dump\n--\n\n\\restrict abc123\n\nSET statement_timeout = 0;\nSELECT 1;\n\\unrestrict abc123\n";
    assert_eq!(
        refusal(sql, Dialect::Postgres),
        Some((5, "\\restrict".into()))
    );
    let message = client_directive(sql, Dialect::Postgres)
        .unwrap()
        .to_string();
    assert!(message.contains("psql meta-command"), "{message}");
    // A backslash that is text is not a directive.
    for text in [
        "SELECT E'\\n', 'a\\b', U&'\\0041'",
        "SELECT $q$ \\restrict $q$",
        "SELECT 1 -- \\connect other\n",
        "SELECT 1 /* \\q */",
        "SELECT \"a\\b\"",
    ] {
        assert_eq!(refusal(text, Dialect::Postgres), None, "{text}");
    }
    // MySQL has no psql.
    assert_eq!(refusal(sql, Dialect::Mysql), None);
}

#[test]
fn copy_from_stdin_is_named_and_the_other_copies_are_not() {
    let sql = "CREATE TABLE t (a int);\nCOPY public.t (a) FROM stdin;\n1\n2\n\\.\n";
    assert_eq!(
        refusal(sql, Dialect::Postgres),
        Some((2, "COPY … FROM STDIN".into()))
    );
    for text in [
        "COPY t FROM '/tmp/x.csv' WITH (FORMAT csv)",
        "COPY t TO STDOUT",
        "COPY (SELECT 1) TO '/tmp/x'",
        "SELECT 'COPY t FROM stdin'",
    ] {
        assert_eq!(refusal(text, Dialect::Postgres), None, "{text}");
    }
    // Whichever comes first is the one named.
    let both = "\\restrict k\nCOPY t FROM stdin;\n";
    assert_eq!(
        refusal(both, Dialect::Postgres),
        Some((1, "\\restrict".into()))
    );
}

#[test]
fn plain_scripts_have_no_directive() {
    let sql =
        "SELECT 1;\nINSERT INTO t VALUES ('a;b');\nCREATE PROCEDURE p() BEGIN SELECT 1; END;\n";
    for dialect in [
        Dialect::Generic,
        Dialect::Mysql,
        Dialect::Postgres,
        Dialect::Trino,
    ] {
        assert_eq!(refusal(sql, dialect), None, "{dialect:?}");
    }
}

#[test]
fn mysqldump_session_lines_stay_separate_statements_and_fail_closed_below_full() {
    // B-14a: `/*!40101 SET … */;` is code the server runs, and a `SET` can move `sql_mode` or the
    // character set, which moves how the *next* bytes are read. The classifier does not claim to
    // know it, so every mode below `full` refuses it, deliberately. A restore that needs them
    // runs under `full`. What this splitter owes is that each one is its own statement.
    let sql = "/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;\n\
               /*!40101 SET NAMES utf8mb4 */;\nCREATE TABLE t (a INT);\n";
    // (Under the three readings that skip an executable comment's body these lines are
    // comments, so the readings disagree and the guard refuses them as ambiguous too.)
    assert_eq!(statements_with_lines_dialect(sql, Dialect::Mysql).len(), 3);
    for mode in [SafeMode::NoDdl, SafeMode::Confirm, SafeMode::ReadOnly] {
        assert!(
            check_dialect(mode, sql, Dialect::Mysql).is_err(),
            "{mode:?}"
        );
    }
    assert!(check_dialect(SafeMode::Full, sql, Dialect::Mysql).is_ok());
}
