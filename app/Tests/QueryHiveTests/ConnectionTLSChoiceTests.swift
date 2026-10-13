import XCTest

@testable import QueryHive

/// The encryption choices of the three drivers, read from and written to the fields a connection
/// already stores, and what each one sends the engine.
final class ConnectionTLSChoiceTests: XCTestCase {
    private func draft(_ kind: ConnectionKind, _ tweak: (inout ConnectionDraft) -> Void = { _ in }) -> ConnectionDraft {
        var d = ConnectionDraft.new()
        d.choose(kind)
        d.name = "wh"; d.host = "h"; d.user = "u"; d.database = "d"
        tweak(&d)
        return d
    }

    private func environment(_ d: ConnectionDraft) -> [String: String] {
        AppModel.connectionEnvironment(d.connection(id: UUID(), group: nil), secrets: ConnectionSecrets())
    }

    // MARK: Options

    func testEachDriverOffersItsOwnChoices() {
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .trino, caFile: ""),
                       [.trinoHTTPSVerified, .trinoHTTPSUnverified, .trinoPrefer, .trinoHTTP])
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .postgres, caFile: ""),
                       ["prefer", "disable", "require", "verify-ca", "verify-full"].map { .sslmode($0) })
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .mysql, caFile: ""),
                       ["disable", "prefer", "require"].map { .sslmode($0) })
    }

    func testAnOptionListMatchesTheDriversOwnModeList() {
        for kind in [ConnectionKind.postgres, .mysql] {
            XCTAssertEqual(ConnectionTLSChoice.options(kind: kind, caFile: "").compactMap(\.modeWord), kind.sslModes)
        }
    }

    func testACAFileLeavesOnlyTheChoicesThatCheckTheCertificate() {
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .trino, caFile: "/ca.pem"), [.trinoHTTPSVerified])
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .postgres, caFile: "/ca.pem"),
                       [.sslmode("verify-ca"), .sslmode("verify-full")])
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .trino, caFile: "  "),
                       ConnectionTLSChoice.options(kind: .trino, caFile: ""), "a blank path is no CA")
    }

    func testATrinoJWTIsOnlyOfferedOverHTTPS() {
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .trino, caFile: "", dbAuth: .jwt),
                       [.trinoHTTPSVerified, .trinoHTTPSUnverified])
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .trino, caFile: "/ca.pem", dbAuth: .jwt), [.trinoHTTPSVerified])
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .trino, caFile: "", dbAuth: .password).count, 4)
        // The other drivers have no JWT, so it changes nothing there.
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .postgres, caFile: "", dbAuth: .jwt),
                       ConnectionTLSChoice.options(kind: .postgres, caFile: ""))
    }

    func testTrinoHTTPKeepsItsVerifyFlagAndSaysAPasswordOrAHTTPSPortRaisesIt() {
        var d = draft(.trino)
        d.setTLS(.trinoHTTPSUnverified)
        d.setTLS(.trinoHTTP)
        XCTAssertFalse(d.verifyTLS, "choosing HTTP leaves the flag for when HTTPS comes back")
        // With a password the engine runs it over HTTPS, and the stored flag then decides the check.
        let withPassword = AppModel.connectionEnvironment(d.connection(id: UUID(), group: nil),
                                                         secrets: ConnectionSecrets(password: "pw"))
        XCTAssertEqual(withPassword["DB_SCHEME"], "http", "the app sends what is stored; the engine raises it")
        XCTAssertEqual(withPassword["DB_INSECURE"], "1")
        for text in [d.tls.help, d.tls.spokenDescription] {
            XCTAssertTrue(text.contains("HTTPS"), text)
            XCTAssertTrue(text.contains("443"), text)
        }
    }

    func testMySQLHasNoCASoAPathChangesNothing() {
        XCTAssertEqual(ConnectionTLSChoice.options(kind: .mysql, caFile: "/ca.pem"),
                       ConnectionTLSChoice.options(kind: .mysql, caFile: ""))
    }

    func testTheCAChoicesAreAlwaysAmongTheOptions() {
        // Setting a CA moves the draft to a verifying mode (`ConnectionDraft.setCAFile`), and that
        // mode is one the menu then still offers. Started from modes that do not verify, so the
        // move is something that happened.
        var trino = draft(.trino)
        trino.setTLS(.trinoPrefer)
        XCTAssertEqual(trino.tls, .trinoPrefer)
        trino.setCAFile("/ca.pem")
        XCTAssertEqual(trino.tls, .trinoHTTPSVerified)
        XCTAssertTrue(ConnectionTLSChoice.options(kind: .trino, caFile: trino.caFile).contains(trino.tls))
        var http = draft(.trino)
        http.setTLS(.trinoHTTP)
        http.verifyTLS = false
        http.setCAFile("/ca.pem")
        XCTAssertEqual(http.tls, .trinoHTTPSVerified)
        var pg = draft(.postgres)
        pg.setTLS(.sslmode("disable"))
        pg.verifyTLS = false
        pg.setCAFile("/ca.pem")
        XCTAssertEqual(pg.tls, .sslmode("verify-full"))
        XCTAssertTrue(ConnectionTLSChoice.options(kind: .postgres, caFile: pg.caFile).contains(pg.tls))
    }

    // MARK: Reading what is stored

    func testTrinoReadsTheSchemeAndTheVerifyFlag() {
        func read(_ scheme: String, _ verify: Bool) -> ConnectionTLSChoice {
            ConnectionTLSChoice(kind: .trino, scheme: scheme, verify: verify, sslmode: "")
        }
        XCTAssertEqual(read("https", true), .trinoHTTPSVerified)
        XCTAssertEqual(read("https", false), .trinoHTTPSUnverified)
        XCTAssertEqual(read("prefer", true), .trinoPrefer)
        XCTAssertEqual(read("prefer", false), .trinoPrefer, "prefer never verifies, whatever the flag says")
        XCTAssertEqual(read("http", true), .trinoHTTP)
        XCTAssertEqual(read("http", false), .trinoHTTP)
    }

    func testAnOldOrUnknownTrinoSchemeIsPlainHTTP() {
        for scheme in ["", "ftp", "HTTP"] {
            XCTAssertEqual(ConnectionTLSChoice(kind: .trino, scheme: scheme, verify: true, sslmode: ""), .trinoHTTP, scheme)
        }
        XCTAssertEqual(ConnectionTLSChoice(kind: .trino, scheme: "HTTPS", verify: true, sslmode: ""), .trinoHTTPSVerified,
                       "the engine reads the word without regard to case")
    }

    func testOtherDriversReadTheirModeWordAndABlankIsTheDriversDefault() {
        XCTAssertEqual(ConnectionTLSChoice(kind: .postgres, scheme: "", verify: true, sslmode: "verify-full"), .sslmode("verify-full"))
        XCTAssertEqual(ConnectionTLSChoice(kind: .postgres, scheme: "", verify: true, sslmode: ""), .sslmode("prefer"))
        XCTAssertEqual(ConnectionTLSChoice(kind: .mysql, scheme: "", verify: true, sslmode: ""), .sslmode("disable"))
        XCTAssertEqual(ConnectionTLSChoice(kind: .mysql, scheme: "https", verify: true, sslmode: "require"), .sslmode("require"),
                       "a MySQL connection ignores the Trino scheme")
    }

    func testAModeStoredWithVerifyOffIsShownAsUnverifiedNeverAsVerified() {
        let postgres = ConnectionTLSChoice(kind: .postgres, scheme: "", verify: false, sslmode: "verify-full")
        XCTAssertEqual(postgres, .sslmodeUnverified("verify-full"))
        XCTAssertEqual(postgres.title, "verify-full · unverified")
        XCTAssertEqual(postgres.glyph, "lock")
        XCTAssertTrue(postgres.spokenDescription.contains("not verified"))
        // A blank mode is the driver's default and reads the same way: nothing is normalised.
        XCTAssertEqual(ConnectionTLSChoice(kind: .postgres, scheme: "", verify: false, sslmode: "").title,
                       "require · unverified")
        // `disable` has nothing to verify, and the engine ignores the flag for it.
        XCTAssertEqual(ConnectionTLSChoice(kind: .mysql, scheme: "", verify: false, sslmode: "disable"), .sslmode("disable"))
    }

    func testReadingNeverChangesWhatIsStored() {
        let stored = Connection(id: UUID(), name: "wh", color: .blue, kind: .postgres, host: "h", port: 5432,
                                sslmode: "verify-full", user: "u", database: "d", schema: "", verify: false)
        var draft = ConnectionDraft(stored)
        XCTAssertEqual(draft.tls, .sslmodeUnverified("verify-full"))
        XCTAssertEqual(draft.verifyTLS, false)
        XCTAssertFalse(draft.isDirty(against: stored))
        // Picking the same word again is the one way to turn the check on.
        draft.setTLS(.sslmode("verify-full"))
        XCTAssertEqual(draft.tls, .sslmode("verify-full"))
        XCTAssertTrue(draft.isDirty(against: stored))
    }

    func testTheDraftReadsItsOwnFields() {
        XCTAssertEqual(draft(.trino).tls, .trinoHTTPSVerified, "a new connection is HTTPS and verified")
        XCTAssertEqual(draft(.postgres).tls, .sslmode("prefer"))
        XCTAssertEqual(draft(.mysql) { $0.sslmode = "" }.tls, .sslmode("disable"))
    }

    // MARK: Writing

    func testEveryChoiceReadsBackAsItself() {
        for kind in ConnectionKind.allCases {
            for choice in ConnectionTLSChoice.options(kind: kind, caFile: "") {
                var d = draft(kind)
                d.setTLS(choice)
                XCTAssertEqual(d.tls, choice, "\(kind) \(choice)")
            }
        }
    }

    func testHTTPSWritesTheVerifyFlagAndPreferAndHTTPLeaveItAlone() {
        var d = draft(.trino)
        d.setTLS(.trinoHTTPSUnverified)
        XCTAssertEqual([d.scheme, "\(d.verifyTLS)"], ["https", "false"])
        d.setTLS(.trinoHTTP)
        XCTAssertEqual([d.scheme, "\(d.verifyTLS)"], ["http", "false"], "the last answer is kept for when HTTPS comes back")
        d.setTLS(.trinoPrefer)
        XCTAssertEqual([d.scheme, "\(d.verifyTLS)"], ["prefer", "false"])
        d.setTLS(.trinoHTTPSVerified)
        XCTAssertEqual([d.scheme, "\(d.verifyTLS)"], ["https", "true"])
        d.setTLS(.trinoPrefer)
        XCTAssertEqual([d.scheme, "\(d.verifyTLS)"], ["prefer", "true"])
    }

    func testChoosingATrinoTransportMovesTheStandardPortLikeThePickerDoesToday() {
        var d = draft(.trino)
        d.setTLS(.trinoHTTP)
        d.port = 8080
        d.setTLS(.trinoHTTPSVerified)
        XCTAssertEqual(d.port, 8443)
        d.setTLS(.trinoHTTP)
        XCTAssertEqual(d.port, 8080)
        d.setTLS(.trinoPrefer)
        XCTAssertEqual(d.port, 8080, "prefer moves no port")
    }

    func testAModeWordWritesTheSSLModeAndTurnsVerifyOnAndNothingElse() {
        var d = draft(.postgres) { $0.verifyTLS = false }
        let before = d
        d.setTLS(.sslmode("verify-ca"))
        XCTAssertEqual(d.sslmode, "verify-ca")
        XCTAssertTrue(d.verifyTLS)
        d.sslmode = before.sslmode
        d.verifyTLS = before.verifyTLS
        XCTAssertEqual(d, before)
    }

    // MARK: What the engine is told

    func testEveryTrinoChoiceSendsWhatTheTransportTableSays() {
        // The table in Connections.swift above `TrinoTransport`: scheme, sslmode and insecure.
        let table: [(ConnectionTLSChoice, scheme: String, sslmode: String, insecure: String)] = [
            (.trinoHTTPSVerified, "https", "", ""),
            (.trinoHTTPSUnverified, "https", "", "1"),
            (.trinoPrefer, "https", "prefer", ""),
            (.trinoHTTP, "http", "", ""),
        ]
        for (choice, scheme, sslmode, insecure) in table {
            var d = draft(.trino)
            d.setTLS(choice)
            let env = environment(d)
            XCTAssertEqual(env["DB_SCHEME"], scheme, "\(choice)")
            XCTAssertEqual(env["DB_SSLMODE"], sslmode, "\(choice)")
            XCTAssertEqual(env["DB_INSECURE"], insecure, "\(choice)")
        }
    }

    func testPlainHTTPKeepsTheStoredVerifyFlagInTheEnvironmentAndPreferNeverSendsIt() {
        for verify in [true, false] {
            var http = draft(.trino) { $0.verifyTLS = verify }
            http.setTLS(.trinoHTTP)
            XCTAssertEqual(environment(http)["DB_INSECURE"], verify ? "" : "1", "http, verify \(verify)")
            var prefer = draft(.trino) { $0.verifyTLS = verify }
            prefer.setTLS(.trinoPrefer)
            XCTAssertEqual(environment(prefer)["DB_INSECURE"], "", "prefer, verify \(verify)")
        }
    }

    func testEveryModeWordIsSentAsTheSSLMode() {
        for kind in [ConnectionKind.postgres, .mysql] {
            for choice in ConnectionTLSChoice.options(kind: kind, caFile: "") {
                var d = draft(kind)
                d.setTLS(choice)
                let env = environment(d)
                XCTAssertEqual(env["DB_SSLMODE"], choice.modeWord, "\(kind) \(choice)")
                XCTAssertEqual(env["DB_SCHEME"], "", "\(kind) has no scheme")
                XCTAssertEqual(env["DB_INSECURE"], "", "\(kind) never sends the Trino verify flag unless it is off")
            }
        }
    }

    func testEveryModeWordWithVerifyOnAndOffSendsTheInsecureFlagAndShowsTheTitleItDeserves() {
        let rows: [(ConnectionKind, [String])] = [(.postgres, ["prefer", "require", "verify-full"]),
                                                  (.mysql, ["prefer", "require"])]
        for (kind, words) in rows {
            for word in words {
                for verify in [true, false] {
                    var d = draft(kind) { $0.sslmode = word; $0.verifyTLS = verify }
                    let env = environment(d)
                    XCTAssertEqual(env["DB_SSLMODE"], word, "\(kind) \(word) \(verify)")
                    XCTAssertEqual(env["DB_INSECURE"], verify ? "" : "1", "\(kind) \(word) verify \(verify)")
                    // The engine never checks `prefer` or `require`, so they always say so; the verify
                    // words say it only when `verify` is off.
                    let checks = word.hasPrefix("verify") && verify
                    // `prefer` with verification off is what the engine runs as `require`, and says so.
                    let unchecked = !verify && word == "prefer" ? "require · unverified" : "\(word) · unverified"
                    XCTAssertEqual(d.tls.title, checks ? word : unchecked, "\(kind) \(word) \(verify)")
                    // Choosing the word from the menu always leaves the connection verified.
                    d.setTLS(.sslmode(word))
                    XCTAssertEqual(environment(d)["DB_INSECURE"], "", "\(kind) \(word) re-picked")
                }
            }
        }
    }

    func testTheTitlesSayUnverifiedExactlyWhereTheEngineWillNotCheck() {
        let titles = ["disable", "prefer", "require", "verify-ca", "verify-full"].map { ConnectionTLSChoice.sslmode($0).title }
        XCTAssertEqual(titles, ["disable", "prefer · unverified", "require · unverified", "verify-ca", "verify-full"])
        XCTAssertEqual(ConnectionTLSChoice.sslmodeUnverified("verify-ca").title, "verify-ca · unverified")
        XCTAssertEqual(ConnectionTLSChoice.trinoHTTPSVerified.title, "HTTPS · verified")
        XCTAssertEqual(ConnectionTLSChoice.trinoPrefer.title, "Prefer · unverified")
    }

    func testPreferThatMayFallBackAndPreferThatMustEncryptAreTwoDifferentTitles() {
        let mayFallBack = ConnectionTLSChoice(kind: .postgres, scheme: "", verify: true, sslmode: "prefer")
        let mustEncrypt = ConnectionTLSChoice(kind: .postgres, scheme: "", verify: false, sslmode: "prefer")
        XCTAssertNotEqual(mayFallBack.title, mustEncrypt.title)
        XCTAssertNotEqual(mayFallBack.help, mustEncrypt.help)
        XCTAssertNotEqual(mayFallBack.spokenDescription, mustEncrypt.spokenDescription)
        XCTAssertEqual(mayFallBack.title, "prefer · unverified")
        XCTAssertEqual(mustEncrypt.title, "require · unverified", "what the engine runs: TLS required, certificate not checked")
        XCTAssertTrue(mustEncrypt.help.contains("Stored as prefer"))
        XCTAssertTrue(mustEncrypt.help.contains("runs as require"))
        XCTAssertEqual(mayFallBack.glyph, "lock.open")
        XCTAssertEqual(mustEncrypt.glyph, "lock")
    }

    func testTheHelpDoesNotPromiseACheckThatCannotComeBack() {
        XCTAssertFalse(ConnectionTLSChoice.sslmodeUnverified("require").help.contains("turn the check"))
        XCTAssertTrue(ConnectionTLSChoice.sslmodeUnverified("prefer").help.contains("fallback"))
        XCTAssertTrue(ConnectionTLSChoice.sslmodeUnverified("verify-full").help.contains("Pick verify-full again to turn the check on"))
        // The note names the exact entry the menu shows for the variant that may fall back.
        let entry = ConnectionTLSChoice.sslmode("prefer").title
        XCTAssertTrue(ConnectionTLSChoice.sslmodeUnverified("prefer").help.contains("Pick \"\(entry)\" to allow the fallback"))
        XCTAssertTrue(ConnectionTLSChoice.options(kind: .postgres, caFile: "").map(\.title).contains(entry))
        XCTAssertFalse(ConnectionTLSChoice.sslmode("verify-ca").help.contains("not the host name"))
        XCTAssertTrue(ConnectionTLSChoice.trinoHTTP.help.contains("443"))
        XCTAssertTrue(ConnectionTLSChoice.trinoHTTP.spokenDescription.contains("HTTPS"))
    }

    func testDisableWithVerifyOffStillSendsTheFlagTheEngineIgnoresForIt() {
        let d = draft(.postgres) { $0.sslmode = "disable"; $0.verifyTLS = false }
        XCTAssertEqual(environment(d)["DB_INSECURE"], "1")
        XCTAssertEqual(d.tls.title, "disable")
    }

    func testABlankSavedModeIsSentAsTheDriversDefaultJustLikeItsChoiceReadsAsIt() {
        for kind in [ConnectionKind.postgres, .mysql] {
            let stored = Connection(id: UUID(), name: "wh", color: .blue, kind: kind, host: "h", port: kind.defaultPort,
                                    sslmode: "", user: "u", database: "d", schema: "", verify: true)
            let sent = AppModel.connectionEnvironment(stored, secrets: ConnectionSecrets())["DB_SSLMODE"]
            let read = ConnectionTLSChoice(kind: kind, scheme: stored.scheme, verify: stored.verify, sslmode: stored.sslmode)
            XCTAssertEqual(sent, read.modeWord, "\(kind)")
        }
    }

    func testAStoredTrinoConnectionIsSentTheSameWhateverItsSSLModeHoldsExceptPrefer() {
        // An earlier editor left Postgres's default in a Trino connection's `sslmode`; only the
        // transport decides.
        for sslmode in ["", "prefer", "disable", "verify-full"] {
            let stored = Connection(id: UUID(), name: "wh", color: .blue, kind: .trino, host: "h", port: 8443,
                                    scheme: "https", sslmode: sslmode, user: "u", database: "d", schema: "", verify: true)
            XCTAssertEqual(AppModel.connectionEnvironment(stored, secrets: ConnectionSecrets())["DB_SSLMODE"], "", sslmode)
        }
    }

    // MARK: Presentation

    func testEveryChoiceHasATitleAGlyphAndASpokenValue() {
        for kind in ConnectionKind.allCases {
            for choice in ConnectionTLSChoice.options(kind: kind, caFile: "") {
                XCTAssertFalse(choice.title.isEmpty, "\(choice)")
                XCTAssertFalse(choice.spokenDescription.isEmpty, "\(choice)")
                XCTAssertFalse(choice.help.isEmpty, "\(choice)")
                XCTAssertTrue(["lock.fill", "lock", "lock.open"].contains(choice.glyph), "\(choice)")
            }
        }
    }

    func testTheGlyphSaysWhetherTheCertificateIsCheckedAndWhetherItMayRunInClear() {
        XCTAssertEqual(ConnectionTLSChoice.trinoHTTPSVerified.glyph, "lock.fill")
        XCTAssertEqual(ConnectionTLSChoice.sslmode("verify-full").glyph, "lock.fill")
        XCTAssertEqual(ConnectionTLSChoice.trinoHTTPSUnverified.glyph, "lock")
        XCTAssertEqual(ConnectionTLSChoice.sslmode("require").glyph, "lock")
        XCTAssertEqual(ConnectionTLSChoice.trinoPrefer.glyph, "lock.open")
        XCTAssertEqual(ConnectionTLSChoice.trinoHTTP.glyph, "lock.open")
        XCTAssertEqual(ConnectionTLSChoice.sslmode("prefer").glyph, "lock.open")
        XCTAssertEqual(ConnectionTLSChoice.sslmode("disable").glyph, "lock.open")
    }
}
