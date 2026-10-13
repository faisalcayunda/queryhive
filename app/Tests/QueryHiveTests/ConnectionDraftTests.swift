import XCTest

@testable import QueryHive

/// The connection sheet's one value: what it makes of a saved connection and what Save stores
/// back, and the rules that tie its fields together. The sheet's own Save/Test paths are driven
/// from these same members, so a rule proven here is the rule the sheet applies.
final class ConnectionDraftTests: XCTestCase {
    private let id = UUID()

    /// A connection as the form would save it: nothing the form would have blanked or defaulted.
    private func saved(_ kind: ConnectionKind, tunnel: Bool = false, auth: SSHAuthMethod = .agent,
                       jwt: Bool = false, ca: Bool = false, timeoutMS: Int? = nil) -> Connection {
        Connection(id: id, name: "wh", color: .violet, kind: kind, host: "db.internal", port: kind.defaultPort,
                   scheme: kind == .trino ? "https" : "", sslmode: kind.defaultSSLMode,
                   user: "u", database: "app", schema: kind.hasSchemaLevel ? "public" : "", verify: false,
                   showAllSchemas: true, showAllDatabases: kind == .postgres,
                   safeMode: .confirm, environment: .staging,
                   sshHost: tunnel ? "bastion" : "", sshUseConfig: tunnel && auth == .agent,
                   sshPort: tunnel ? 2222 : 0, sshUser: tunnel ? "deploy" : "",
                   sshAuth: tunnel ? auth : .agent,
                   sshKeyPath: tunnel && auth == .key ? "/k/id" : "",
                   dbAuth: jwt && kind == .trino ? .jwt : .password,
                   caFile: ca && kind != .mysql ? "/ca.pem" : "", statementTimeoutMS: timeoutMS)
    }

    func testEveryKindRoundTripsThroughTheDraft() {
        for kind in ConnectionKind.allCases {
            for tunnel in [false, true] {
                for auth in tunnel ? SSHAuthMethod.allCases : [.agent] {
                    for jwt in [false, true] {
                        for ca in [false, true] {
                            for timeout in [nil, 0, 45_000] as [Int?] {
                                let c = saved(kind, tunnel: tunnel, auth: auth, jwt: jwt, ca: ca, timeoutMS: timeout)
                                let back = ConnectionDraft(c).connection(id: c.id, group: c.group)
                                XCTAssertEqual(back, c, "\(kind) tunnel=\(tunnel) \(auth) jwt=\(jwt) ca=\(ca) timeout=\(String(describing: timeout))")
                            }
                        }
                    }
                }
            }
        }
    }

    func testTheGroupIsTheSavedOnesAndNotTheDrafts() {
        let group = UUID()
        var c = saved(.postgres)
        c.group = group
        XCTAssertEqual(ConnectionDraft(c).connection(id: c.id, group: group), c)
        XCTAssertNil(ConnectionDraft(c).connection(id: c.id, group: nil).group)
    }

    func testTextFieldsAreTrimmedOnSaveButNotInTheDraft() {
        var draft = ConnectionDraft.new()
        draft.name = "  wh  "; draft.host = " db "; draft.user = " u "; draft.database = " app "; draft.schema = " s "
        draft.caFile = " /ca.pem "
        let c = draft.connection(id: id, group: nil)
        XCTAssertEqual([c.name, c.host, c.user, c.database, c.schema, c.caFile], ["wh", "db", "u", "app", "s", "/ca.pem"])
        XCTAssertEqual(draft.name, "  wh  ")
    }

    func testTurningTheTunnelOffClearsItsFieldsOnSaveButKeepsThemInTheDraft() {
        var draft = ConnectionDraft(saved(.postgres, tunnel: true, auth: .key))
        XCTAssertTrue(draft.sshEnabled)
        draft.sshEnabled = false
        let off = draft.connection(id: id, group: nil)
        XCTAssertEqual([off.sshHost, off.sshUser, off.sshKeyPath], ["", "", ""])
        XCTAssertEqual(off.sshPort, 0)
        XCTAssertEqual(off.sshAuth, .agent)
        XCTAssertFalse(off.sshUseConfig)
        XCTAssertFalse(off.usesTunnel)
        // Still there, so turning it back on finds what was typed.
        XCTAssertEqual([draft.sshHost, draft.sshUser, draft.sshKeyPath], ["bastion", "deploy", "/k/id"])
        draft.sshEnabled = true
        XCTAssertEqual(draft.connection(id: id, group: nil), saved(.postgres, tunnel: true, auth: .key))
    }

    func testOnlyTheKeyMethodStoresAKeyPath() {
        var draft = ConnectionDraft(saved(.postgres, tunnel: true, auth: .key))
        draft.sshAuth = .password
        XCTAssertEqual(draft.connection(id: id, group: nil).sshKeyPath, "")
    }

    func testWhatOnlyBelongsToAnotherDriverIsLeftOutOnSave() {
        var draft = ConnectionDraft(saved(.trino, jwt: true, ca: true))
        draft.kind = .postgres
        XCTAssertEqual(draft.connection(id: id, group: nil).dbAuth, .password, "JWT is Trino's")
        draft.kind = .mysql
        XCTAssertEqual(draft.connection(id: id, group: nil).caFile, "", "MySQL has no CA setting")
        XCTAssertEqual(draft.formState.caFile, "", "and validation does not read one")
    }

    func testAStatementTimeoutIsSecondsOnTheFormAndMillisecondsInTheConnection() {
        var draft = ConnectionDraft.new()
        XCTAssertNil(draft.timeoutMS, "blank inherits")
        draft.timeoutText = " 30 "
        XCTAssertEqual(draft.timeoutMS, 30_000)
        draft.timeoutText = "0"
        XCTAssertEqual(draft.timeoutMS, 0, "0 is 'no bound', which is not the same as inheriting")
        for bad in ["601", "-1", "x", "1.5"] {
            draft.timeoutText = bad
            XCTAssertNil(draft.timeoutMS, bad)
        }
    }

    func testASavedBlankSSLModeStaysBlankAndOpeningItIsNotAChange() {
        // T-1: the sheet used to load a blank mode as the driver's default, which Save would then
        // write, so a connection nobody touched opened as "Unsaved changes".
        for kind in ConnectionKind.allCases {
            var c = saved(kind)
            c.sslmode = ""
            let draft = ConnectionDraft(c)
            XCTAssertEqual(draft.sslmode, "", "\(kind)")
            XCTAssertFalse(draft.isDirty(against: c), "\(kind)")
            XCTAssertEqual(draft.connection(id: c.id, group: c.group), c, "and what Save would write is what is saved")
        }
    }

    func testOpeningAConnectionAnOlderBuildOrAnImportSavedIsNotAChange() {
        // What Save tidies is not an edit: each of these opened as "Unsaved changes" before.
        func opens(_ name: String, _ kind: ConnectionKind = .postgres, _ tweak: (inout Connection) -> Void) {
            var c = saved(kind)
            tweak(&c)
            XCTAssertFalse(ConnectionDraft(c).isDirty(against: c), name)
        }
        opens("a timeout that is not whole seconds") { $0.statementTimeoutMS = 1500 }
        opens("spaces an import left in the name") { $0.name = "  Warehouse " }
        opens("spaces an import left in the host") { $0.host = " pg.internal  " }
        opens("a MySQL connection with a CA file", .mysql) { $0.caFile = "/ca.pem" }
        opens("a PostgreSQL connection with a JWT login") { $0.dbAuth = .jwt }
        opens("an upper-case Trino scheme", .trino) { $0.scheme = "HTTPS" }
        opens("a Trino connection with no scheme", .trino) { $0.scheme = "" }
    }

    func testARealEditToASavedConnectionIsStillAChange() {
        var c = saved(.postgres)
        c.sslmode = ""
        var draft = ConnectionDraft(c)
        draft.sslmode = "require"
        XCTAssertTrue(draft.isDirty(against: c))
        draft = ConnectionDraft(c)
        draft.timeoutText = "30"
        XCTAssertTrue(draft.isDirty(against: c))
        draft = ConnectionDraft(c)
        draft.host += "x"
        XCTAssertTrue(draft.isDirty(against: c))
    }

    // MARK: Rules

    func testChoosingADriverMovesTheDefaultPortOnlyWhileItIsStillTheDefault() {
        var draft = ConnectionDraft.new()
        draft.choose(.postgres)
        XCTAssertEqual(draft.kind, .postgres)
        XCTAssertEqual(draft.port, 5432)
        draft.port = 6543
        draft.choose(.mysql)
        XCTAssertEqual(draft.port, 6543, "a port the person typed stays")
    }

    func testChoosingADriverFillsABlankSSLModeForDriversThatHaveOne() {
        var draft = ConnectionDraft.new()
        draft.sslmode = ""
        draft.choose(.mysql)
        XCTAssertEqual(draft.sslmode, "disable")
        draft.sslmode = ""
        draft.choose(.trino)
        XCTAssertEqual(draft.sslmode, "", "Trino has no SSL modes")
    }

    func testChoosingADriverResetsTheJWTAndTheCAThatDoNotApply() {
        var draft = ConnectionDraft.new()
        draft.dbAuth = .jwt
        draft.secrets.jwt = "token"
        draft.caFile = "/ca.pem"
        draft.choose(.postgres)
        XCTAssertEqual(draft.dbAuth, .password)
        XCTAssertEqual(draft.secrets.jwt, "")
        XCTAssertEqual(draft.caFile, "/ca.pem", "PostgreSQL can use a CA")
        draft.choose(.mysql)
        XCTAssertEqual(draft.caFile, "", "MySQL cannot")
    }

    func testChoosingTrinoKeepsItsOwnJWT() {
        var draft = ConnectionDraft.new()
        draft.dbAuth = .jwt
        draft.secrets.jwt = "token"
        draft.choose(.trino)
        XCTAssertEqual(draft.dbAuth, .jwt)
        XCTAssertEqual(draft.secrets.jwt, "token")
    }

    func testApplyingAURLFillsTheFieldsItSpeaksAbout() throws {
        var draft = ConnectionDraft.new()
        draft.color = .red
        draft.safeMode = .readOnly
        let parsed = try XCTUnwrap(try ConnectionURL.parse("postgresql://ann:p%40ss@db.corp:6543/sales?sslmode=verify-full"))
        draft.apply(parsed)
        XCTAssertEqual(draft.kind, .postgres)
        XCTAssertEqual([draft.host, draft.user, draft.database], ["db.corp", "ann", "sales"])
        XCTAssertEqual(draft.port, 6543)
        XCTAssertEqual(draft.sslmode, "verify-full")
        XCTAssertEqual(draft.scheme, "https", "a driver with no scheme keeps the form's default")
        XCTAssertEqual(draft.secrets.password, "p@ss")
        XCTAssertEqual(draft.name, "db.corp/sales", "a blank name is filled from the URL")
        XCTAssertEqual(draft.color, .red)
        XCTAssertEqual(draft.safeMode, .readOnly, "what the URL does not say is left alone")
    }

    func testAnAppliedURLKeepsItsPortWhateverTheSchemeWasBefore() throws {
        var draft = ConnectionDraft.new()
        draft.scheme = "https"
        draft.apply(try XCTUnwrap(try ConnectionURL.parse("http://h:8443/hive")))
        XCTAssertEqual(draft.scheme, "http")
        XCTAssertEqual(draft.port, 8443, "the URL's port is what was typed; the scheme does not rewrite it")
    }

    func testACAInTheDraftOverridesAURLThatAsksForNoEncryption() throws {
        // The safe direction, shown on the form before anything is saved: a CA means nothing to
        // `disable` or `http`, so the CA the person chose earlier makes the mode verify.
        var draft = ConnectionDraft.new()
        draft.setCAFile("/ca.pem")
        draft.apply(try XCTUnwrap(try ConnectionURL.parse("postgresql://u@h/db?sslmode=disable")))
        XCTAssertEqual(draft.sslmode, "verify-full")
        var trino = ConnectionDraft.new()
        trino.setCAFile("/ca.pem")
        trino.apply(try XCTUnwrap(try ConnectionURL.parse("http://h:8080/hive")))
        XCTAssertEqual(trino.scheme, "https")
        XCTAssertTrue(trino.verifyTLS)
    }

    func testApplyingAURLKeepsANameAlreadyTyped() throws {
        var draft = ConnectionDraft.new()
        draft.name = "mine"
        draft.apply(try XCTUnwrap(try ConnectionURL.parse("mysql://u@h/db")))
        XCTAssertEqual(draft.name, "mine")
        XCTAssertEqual(draft.kind, .mysql)
        XCTAssertEqual(draft.sslmode, "disable", "no mode in the URL is the driver's default")
        XCTAssertEqual(draft.secrets.password, "", "no password in the URL is blank")
    }

    func testApplyingATrinoURLReadsItsTransportAndVerification() throws {
        var draft = ConnectionDraft.new()
        draft.apply(try XCTUnwrap(try ConnectionURL.parse("trino://u@h:8443/hive?sslmode=require")))
        XCTAssertEqual(draft.kind, .trino)
        XCTAssertEqual(draft.scheme, "https")
        XCTAssertFalse(draft.verifyTLS, "require encrypts and does not verify")
    }

    func testAPickedCAFileForcesAModeThatChecksTheCertificate() {
        var pg = ConnectionDraft.new()
        pg.choose(.postgres)
        pg.setCAFile("/ca.pem")
        XCTAssertEqual(pg.sslmode, "verify-full")
        pg.sslmode = "verify-ca"
        pg.setCAFile("/other.pem")
        XCTAssertEqual(pg.sslmode, "verify-ca", "a mode that already verifies stays")

        var trino = ConnectionDraft.new()
        trino.scheme = "http"
        trino.verifyTLS = false
        trino.setCAFile("/ca.pem")
        XCTAssertEqual(trino.scheme, "https")
        XCTAssertTrue(trino.verifyTLS)
    }

    func testAPickedCAFileOnTrinoMovesTheStandardHTTPPortWithTheScheme() {
        var trino = ConnectionDraft.new()
        trino.scheme = "http"
        trino.port = 8080
        trino.setCAFile("/ca.pem")
        XCTAssertEqual(trino.port, 8443)
    }

    func testABlankOrNoCAAsksForNothing() {
        var pg = ConnectionDraft.new()
        pg.choose(.postgres)
        pg.setCAFile("   ")
        XCTAssertEqual(pg.sslmode, "prefer")
        var mysql = ConnectionDraft.new()
        mysql.choose(.mysql)
        mysql.sslmode = "require"
        mysql.setCAFile("/ca.pem")
        XCTAssertEqual(mysql.sslmode, "require", "MySQL's mode is not tied to a CA")
    }

    func testTheSchemeMovesTheStandardPortAndNothingElse() {
        var draft = ConnectionDraft.new()
        draft.scheme = "http"
        draft.port = 8080
        draft.setScheme("https")
        XCTAssertEqual(draft.port, 8443)
        draft.setScheme("http")
        XCTAssertEqual(draft.port, 8080)
        draft.port = 9000
        draft.setScheme("https")
        XCTAssertEqual(draft.port, 9000, "a port the person chose stays")
        draft.setScheme("http")
        XCTAssertEqual(draft.port, 9000)
    }

    func testABlankSchemeIsHTTPAndFollowsThePortToo() {
        var draft = ConnectionDraft.new()
        draft.scheme = ""
        draft.port = 8080
        draft.setScheme("https")
        XCTAssertEqual(draft.port, 8443, "the engine reads a blank scheme as plain HTTP")
        draft.port = 8443
        draft.scheme = "ftp"
        draft.setScheme("https")
        XCTAssertEqual(draft.port, 8443, "an unknown word is read as HTTP, and HTTP to HTTPS from 8443 moves nothing")
        draft.setScheme("")
        XCTAssertEqual(draft.port, 8080, "and a blank one is HTTP going back")
    }

    func testPreferMovesNoPort() {
        var draft = ConnectionDraft.new()
        draft.scheme = "http"
        draft.port = 8080
        draft.setScheme("prefer")
        XCTAssertEqual(draft.port, 8080)
        draft.scheme = "https"
        draft.port = 8443
        draft.setScheme("prefer")
        XCTAssertEqual(draft.port, 8443)
    }

    // MARK: The CA constraint holds wherever the draft can change

    func testAPostgreSQLModeThatDoesNotVerifyGetsVerificationBackWithACA() {
        var draft = ConnectionDraft.new()
        draft.choose(.postgres)
        draft.verifyTLS = false
        draft.setCAFile("/ca.pem")
        XCTAssertEqual(draft.sslmode, "verify-full")
        XCTAssertTrue(draft.verifyTLS)
        draft = .new()
        draft.choose(.postgres)
        draft.sslmode = "verify-ca"
        draft.verifyTLS = false
        draft.setCAFile("/ca.pem")
        XCTAssertEqual(draft.sslmode, "verify-ca", "a verifying word stays")
        XCTAssertTrue(draft.verifyTLS, "but the flag that switches the check off does not")
    }

    func testLeavingAnUnverifiedTrinoStartsTheNewDriverVerified() {
        var draft = ConnectionDraft.new()
        draft.verifyTLS = false
        draft.choose(.postgres)
        XCTAssertTrue(draft.verifyTLS, "the flag meant unverified HTTPS, not anything about PostgreSQL")
        draft = .new()
        draft.verifyTLS = false
        draft.choose(.mysql)
        XCTAssertTrue(draft.verifyTLS)
        draft = .new()
        draft.verifyTLS = false
        draft.choose(.trino)
        XCTAssertFalse(draft.verifyTLS, "choosing Trino again keeps Trino's own answer")
    }

    /// What the engine makes of a PostgreSQL or MySQL draft (`EngineTLSMode`).
    private func engineMode(_ draft: ConnectionDraft) -> EngineTLSMode {
        EngineTLSMode(draft.connection(id: id, group: nil))
    }

    func testPickingTheSameDriverAgainChangesNothingAboutTheModeTheEngineRuns() {
        // A Navicat import: `prefer` with verify off, which the engine runs as "must encrypt".
        var draft = ConnectionDraft(Connection(id: id, name: "imp", color: .blue, kind: .postgres, host: "h", port: 5432,
                                               sslmode: "", user: "u", database: "d", schema: "", verify: false))
        XCTAssertEqual(engineMode(draft), .requireNoVerify)
        let before = draft
        draft.choose(.postgres)
        XCTAssertEqual(draft, before, "Change Type then the same tile changes nothing")
        XCTAssertEqual(engineMode(draft), .requireNoVerify, "so it may not become the mode that can connect in clear")
        XCTAssertNotEqual(engineMode(draft), .prefer)
    }

    func testMovingBetweenPostgreSQLAndMySQLKeepsTheModeTheEngineRuns() {
        var draft = ConnectionDraft(Connection(id: id, name: "imp", color: .blue, kind: .postgres, host: "h", port: 5432,
                                               sslmode: "require", user: "u", database: "d", schema: "", verify: false))
        draft.choose(.mysql)
        XCTAssertFalse(draft.verifyTLS)
        XCTAssertEqual(engineMode(draft), .requireNoVerify)
    }

    func testLeavingTrinoMovesTheModeToTheNewDriversDefaultNotToASilentlyWeakerOne() {
        var draft = ConnectionDraft.new()
        draft.verifyTLS = false
        draft.choose(.postgres)
        XCTAssertEqual(draft.verifyTLS, true)
        XCTAssertEqual(draft.sslmode, "prefer")
        XCTAssertEqual(engineMode(draft), .prefer, "the documented reset: PostgreSQL's own default, as a new connection has it")
    }

    func testATrinoDraftWithACAThenChangeTypeToPostgreSQLIsNotPostgreSQLWithACAAndDisable() {
        var draft = ConnectionDraft(Connection(id: id, name: "t", color: .blue, kind: .trino, host: "h", port: 8443,
                                               scheme: "https", sslmode: "", user: "u", database: "", schema: "",
                                               verify: true, caFile: "/ca.pem"))
        XCTAssertEqual(draft.sslmode, "", "a Trino connection stores no SSL mode, and the form shows it as stored")
        draft.choose(.postgres)
        XCTAssertEqual(draft.caFile, "/ca.pem")
        XCTAssertEqual(draft.sslmode, "verify-full")
        XCTAssertTrue(draft.verifyTLS)
    }

    func testAPostgreSQLDraftWithACAThenChangeTypeToTrinoIsHTTPSAndVerified() {
        var draft = ConnectionDraft(Connection(id: id, name: "p", color: .blue, kind: .postgres, host: "h", port: 5432,
                                               scheme: "", sslmode: "verify-full", user: "u", database: "d", schema: "",
                                               verify: true, caFile: "/ca.pem"))
        XCTAssertEqual(draft.scheme, "")
        draft.choose(.trino)
        XCTAssertEqual(draft.scheme, "https", "a blank scheme would read as plain HTTP")
        XCTAssertTrue(draft.verifyTLS)
        XCTAssertEqual(draft.caFile, "/ca.pem")
    }

    func testAURLAppliedOverADraftThatHoldsACAKeepsTheCARule() throws {
        var draft = ConnectionDraft.new()
        draft.setCAFile("/ca.pem")
        draft.apply(try XCTUnwrap(try ConnectionURL.parse("postgresql://u@h/db?sslmode=disable")))
        XCTAssertEqual(draft.kind, .postgres)
        XCTAssertEqual(draft.sslmode, "verify-full")
        XCTAssertTrue(draft.verifyTLS)
        draft.apply(try XCTUnwrap(try ConnectionURL.parse("http://h:8080/hive")))
        XCTAssertEqual(draft.kind, .trino)
        XCTAssertEqual(draft.scheme, "https")
        XCTAssertEqual(draft.port, 8443, "the standard port follows the scheme the CA forced")
    }

    func testALoadedConnectionThatAlreadyBreaksTheRuleIsShownAsSaved() {
        let broken = Connection(id: id, name: "p", color: .blue, kind: .postgres, host: "h", port: 5432,
                                sslmode: "disable", user: "u", database: "d", schema: "", verify: false,
                                caFile: "/ca.pem")
        let draft = ConnectionDraft(broken)
        XCTAssertEqual(draft.sslmode, "disable")
        XCTAssertFalse(draft.verifyTLS)
        XCTAssertFalse(draft.isDirty(against: broken), "and opening it does not make it dirty")
    }

    func testTypingAMySQLCAChangesNothing() {
        var draft = ConnectionDraft.new()
        draft.choose(.mysql)
        draft.verifyTLS = false
        draft.setCAFile("/ca.pem")
        XCTAssertEqual(draft.sslmode, "prefer")
        XCTAssertFalse(draft.verifyTLS)
        XCTAssertEqual(draft.connection(id: id, group: nil).caFile, "", "and it is not saved")
    }

    // MARK: Dirty

    func testANewDraftIsDirtyOnceAnythingIsInIt() {
        var draft = ConnectionDraft.new()
        XCTAssertFalse(draft.isDirty(against: nil))
        draft.secrets.password = "x"
        XCTAssertTrue(draft.isDirty(against: nil))
        draft = .new()
        draft.sshEnabled = true
        XCTAssertTrue(draft.isDirty(against: nil))
        draft = .new()
        draft.color = .red
        XCTAssertFalse(draft.isDirty(against: nil), "colour and the driver alone do not make a new connection worth saving")
    }

    func testASavedConnectionIsDirtyForATypedSecretAStagedRemovalOrAChangedField() {
        let c = saved(.postgres)
        var draft = ConnectionDraft(c)
        XCTAssertFalse(draft.isDirty(against: c))
        draft.secrets.sshPassword = "s"
        XCTAssertTrue(draft.isDirty(against: c))
        draft = ConnectionDraft(c)
        draft.secrets.removals = [.database]
        XCTAssertTrue(draft.isDirty(against: c))
        draft = ConnectionDraft(c)
        draft.host = c.host + " "
        XCTAssertFalse(draft.isDirty(against: c), "trailing space is trimmed before the comparison")
        draft.host = "other"
        XCTAssertTrue(draft.isDirty(against: c))
    }

    func testTheDraftsSecretsAreTheSavePlansInput() {
        var draft = ConnectionDraft(saved(.postgres, tunnel: true, auth: .password))
        draft.secrets.password = "db"
        draft.secrets.sshPassword = "ssh"
        draft.secrets.sshPassphrase = "unused"
        let c = draft.connection(id: id, group: nil)
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: draft.secrets, old: saved(.postgres, tunnel: true, auth: .password), new: c),
                       [.set(.database, "db"), .set(.sshPassword, "ssh")])
    }
}
