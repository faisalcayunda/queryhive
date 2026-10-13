import XCTest

@testable import QueryHive

/// Named validation (FR-CON-09) and what saving the form does to the Keychain (PF-5).
final class ConnectionFormIssuesTests: XCTestCase {
    private func state(_ kind: ConnectionKind = .postgres, _ tweak: (inout ConnectionFormState) -> Void = { _ in }) -> ConnectionFormState {
        var state = ConnectionFormState(kind: kind, name: "wh", host: "db", port: 5432, user: "u", database: "app")
        state.fileExists = { _ in true }
        tweak(&state)
        return state
    }

    func testAFilledFormHasNothingMissing() {
        XCTAssertEqual(ConnectionFormIssues.missing(for: state(), resolved: nil), [])
    }

    func testTheSentenceNamesWhatIsMissingInSheetOrder() {
        let missing = ConnectionFormIssues.missing(for: state { $0.host = ""; $0.user = " " }, resolved: nil)
        XCTAssertEqual(missing, ["Host", "User"])
        XCTAssertEqual(ConnectionFormIssues.message(missing), "Host and User are required.")
        let tunnelled = ConnectionFormIssues.missing(for: state {
            $0.database = ""; $0.sshEnabled = true; $0.sshHost = "bastion"
        }, resolved: nil)
        XCTAssertEqual(tunnelled, ["Database", "SSH user"])
        XCTAssertEqual(ConnectionFormIssues.message(["Host", "Database", "SSH user"]),
                       "Host, Database and SSH user are required.")
        XCTAssertEqual(ConnectionFormIssues.message(["Name"]), "Name is required.")
        XCTAssertEqual(ConnectionFormIssues.message([]), "")
    }

    func testAnAliasThatNamesTheUserOrTheKeyFileSatisfiesThoseFields() {
        let alias = SSHAliasInfo(hostName: "bastion.corp", user: "deploy", port: 22, identityFiles: ["/k"])
        let tunnel = { (s: inout ConnectionFormState) in
            s.sshEnabled = true; s.sshHost = "prod"; s.sshUseConfig = true; s.sshAuth = .key
        }
        XCTAssertEqual(ConnectionFormIssues.missing(for: state(.postgres, tunnel), resolved: nil),
                       ["SSH user", "SSH key file"], "without the alias both are missing")
        XCTAssertEqual(ConnectionFormIssues.missing(for: state(.postgres, tunnel), resolved: alias), [])
        // An alias only counts when the host is one: a typed host name has no config behind it.
        XCTAssertEqual(ConnectionFormIssues.missing(for: state { tunnel(&$0); $0.sshUseConfig = false }, resolved: alias),
                       ["SSH user", "SSH key file"])
    }

    func testTheKeyFileIsOnlyNeededForTheKeyMethod() {
        let off = state { $0.sshEnabled = true; $0.sshHost = "b"; $0.sshUser = "u"; $0.sshAuth = .agent }
        XCTAssertEqual(ConnectionFormIssues.missing(for: off, resolved: nil), [])
        let key = state { $0.sshEnabled = true; $0.sshHost = "b"; $0.sshUser = "u"; $0.sshAuth = .key }
        XCTAssertEqual(ConnectionFormIssues.missing(for: key, resolved: nil), ["SSH key file"])
    }

    func testAnOffTunnelNeverComplains() {
        let off = state { $0.sshEnabled = false; $0.sshHost = ""; $0.sshAuth = .key }
        XCTAssertEqual(ConnectionFormIssues.missing(for: off, resolved: nil), [])
    }

    func testValuesThatArePresentButWrongAreNotCalledRequired() {
        let wrong = ConnectionFormIssues.missing(for: state {
            $0.host = ""; $0.port = 70_000; $0.caFile = "/no/such.pem"; $0.fileExists = { _ in false }
            $0.sshEnabled = true; $0.sshHost = "b"; $0.sshUser = "u"; $0.sshPort = 99_999
            $0.timeoutText = "ninety"
        }, resolved: nil)
        XCTAssertEqual(wrong, ["Host", "Port (1-65535)", "SSH port (1-65535)", "CA file (not found)",
                               "Statement timeout (0-600 seconds)"])
        XCTAssertEqual(ConnectionFormIssues.message(wrong),
                       "Host is required. Check Port (1-65535), SSH port (1-65535), CA file (not found) and Statement timeout (0-600 seconds).")
    }

    func testATrinoJWTOverPreferOrHTTPIsNamedBeforeSaveBecauseTheEngineWouldRefuseIt() {
        func fields(_ scheme: String, auth: DatabaseAuth = .jwt, kind: ConnectionKind = .trino) -> [ConnectionField] {
            ConnectionFormIssues.fields(for: state(kind) { $0.scheme = scheme; $0.dbAuth = auth }, resolved: nil)
        }
        XCTAssertEqual(fields("prefer"), [.encryption])
        XCTAssertEqual(fields("http"), [.encryption])
        XCTAssertEqual(fields("HTTP"), [.encryption], "read as http, like TrinoTransport")
        XCTAssertEqual(fields("https"), [])
        XCTAssertEqual(fields(""), [], "with a token the engine reads a blank scheme as TLS")
        XCTAssertEqual(fields("prefer", auth: .password), [], "a password login has no such rule")
        XCTAssertEqual(fields("http", kind: .postgres), [], "JWT is Trino's alone")
        let missing = fields("prefer")
        XCTAssertEqual(ConnectionFormIssues.message(missing.map(\.label)), "Check Encryption (JWT needs HTTPS).")
        XCTAssertEqual(ConnectionFormIssues.summary(missing, visible: .general), "Check Encryption (JWT needs HTTPS).")
        XCTAssertEqual(ConnectionField.encryption.section, .general)
    }

    func testTheTimeoutAcceptsABlankAndZeroToSixHundred() {
        for ok in ["", " ", "0", "60", "600"] {
            XCTAssertEqual(ConnectionFormIssues.missing(for: state { $0.timeoutText = ok }, resolved: nil), [], ok)
        }
        for bad in ["-1", "601", "1.5", "x"] {
            XCTAssertEqual(ConnectionFormIssues.missing(for: state { $0.timeoutText = bad }, resolved: nil),
                           ["Statement timeout (0-600 seconds)"], bad)
        }
    }

    // MARK: Panes

    func testEveryFieldBelongsToExactlyOnePane() {
        for field in ConnectionField.allCases {
            let owners = ConnectionFormSection.allCases.filter { section in field.section == section }
            XCTAssertEqual(owners.count, 1, field.rawValue)
        }
        XCTAssertEqual(ConnectionField.allCases.filter { $0.section == .general }.map(\.rawValue),
                       ["name", "host", "encryption", "port", "user", "database", "caFile"])
        XCTAssertEqual(ConnectionField.allCases.filter { $0.section == .sshTunnel }.map(\.rawValue),
                       ["sshHost", "sshPort", "sshUser", "sshKeyFile"])
        XCTAssertEqual(ConnectionField.allCases.filter { $0.section == .limits }.map(\.rawValue), ["timeout"])
        XCTAssertTrue(ConnectionField.allCases.allSatisfy { $0.section != .appearance },
                      "nothing in Appearance can be wrong")
    }

    func testThePanesHaveNamesAndSymbolsInSidebarOrder() {
        XCTAssertEqual(ConnectionFormSection.allCases.map(\.title), ["General", "SSH Tunnel", "Limits", "Appearance"])
        XCTAssertEqual(Set(ConnectionFormSection.allCases.map(\.symbol)).count, 4)
        XCTAssertEqual(ConnectionFormSection.allCases.map(\.rawValue), ["general", "sshTunnel", "limits", "appearance"])
    }

    func testIssuesGroupByThePaneThatEditsThem() {
        let fields = ConnectionFormIssues.fields(for: state {
            $0.host = ""; $0.timeoutText = "x"; $0.sshEnabled = true; $0.sshHost = ""
        }, resolved: nil)
        let grouped = ConnectionFormIssues.bySection(fields)
        XCTAssertEqual(grouped[.general], [.host])
        XCTAssertEqual(grouped[.sshTunnel], [.sshHost, .sshUser])
        XCTAssertEqual(grouped[.limits], [.timeout])
        XCTAssertNil(grouped[.appearance])
        XCTAssertEqual(ConnectionFormIssues.bySection([]), [:])
    }

    func testTheSummaryTellsThePaneInViewFirstAndPointsToTheOthers() {
        let all: [ConnectionField] = [.host, .sshHost, .timeout]
        XCTAssertEqual(ConnectionFormIssues.summary(all, visible: .general),
                       "Host is required. Also see SSH Tunnel and Limits.")
        XCTAssertEqual(ConnectionFormIssues.summary(all, visible: .sshTunnel),
                       "SSH host is required. Also see General and Limits.")
        XCTAssertEqual(ConnectionFormIssues.summary(all, visible: .limits),
                       "Check Statement timeout (0-600 seconds). Also see General and SSH Tunnel.")
    }

    func testAnotherPanesProblemsLeadWithItsName() {
        XCTAssertEqual(ConnectionFormIssues.summary([.sshHost, .sshUser], visible: .general),
                       "SSH Tunnel: SSH host and SSH user are required.")
        XCTAssertEqual(ConnectionFormIssues.summary([.timeout], visible: .appearance),
                       "Limits: Check Statement timeout (0-600 seconds).")
        XCTAssertEqual(ConnectionFormIssues.summary([.sshHost, .timeout], visible: .appearance),
                       "SSH Tunnel: SSH host is required. Also see Limits.")
    }

    func testTheSummaryIsEmptyWhenNothingIsWrong() {
        XCTAssertEqual(ConnectionFormIssues.summary([], visible: .general), "")
    }

    func testSeveralIssuesInOnePaneShareOneSentence() {
        XCTAssertEqual(ConnectionFormIssues.summary([.name, .host, .caFile], visible: .general),
                       "Name and Host are required. Check CA file (not found).")
    }

    // MARK: PF-5, "an empty field means keep"

    private func connection(_ tweak: (inout Connection) -> Void = { _ in }) -> Connection {
        var c = Connection(id: UUID(), name: "c", color: .blue, kind: .postgres, host: "h", port: 5432,
                           user: "u", database: "d", schema: "", verify: true,
                           sshHost: "bastion", sshUser: "deploy", sshAuth: .password)
        tweak(&c)
        return c
    }

    func testSavingWithEveryFieldEmptyTouchesNothing() {
        let saved = connection()
        let plan = ConnectionSecretPlan.operations(draft: .init(), old: saved, new: saved)
        XCTAssertEqual(plan, [], "an empty field is 'I did not retype it', never 'delete it'")
    }

    @MainActor
    func testEditThenSaveWithEmptyFieldsLeavesAllFourSlotsIntact() throws {
        let store = useMemorySecretStore()
        let id = UUID()
        for slot in ConnectionKeychain.Slot.allCases { try store.set("keep-\(slot)", slot: slot, for: id) }
        var saved = connection { $0.id = id }
        saved.sshAuth = .password
        // Every auth setting unchanged, every field blank: the whole plan is empty and the store is
        // exactly as it was.
        let plan = ConnectionSecretPlan.operations(draft: .init(), old: saved, new: saved)
        try AppModel.apply(plan, for: id)
        for slot in ConnectionKeychain.Slot.allCases {
            XCTAssertEqual(try store.get(slot: slot, for: id), "keep-\(slot)", "\(slot)")
        }
    }

    func testATypedSecretIsWrittenOnlyToTheSlotTheConnectionUses() {
        let saved = connection()
        let typed = ConnectionSecretPlan.Draft(password: "db", sshPassword: "ssh", sshPassphrase: "phrase", jwt: "tok")
        let plan = ConnectionSecretPlan.operations(draft: typed, old: saved, new: saved)
        XCTAssertEqual(plan, [.set(.database, "db"), .set(.sshPassword, "ssh")],
                       "a passphrase for a password tunnel and a JWT on PostgreSQL are not stored")
    }

    func testRemovalIsExplicitAndTypingAfterwardsWins() {
        let saved = connection()
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(removals: [.sshPassword]), old: saved, new: saved),
                       [.remove(.sshPassword)])
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(sshPassword: "new", removals: [.sshPassword]),
                                                       old: saved, new: saved),
                       [.set(.sshPassword, "new")])
    }

    func testChangingTheAuthMethodRemovesTheSecretTheConnectionStoppedUsing() {
        let old = connection { $0.sshAuth = .password }
        var key = old
        key.sshAuth = .key
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(), old: old, new: key), [.remove(.sshPassword)])
        var agent = old
        agent.sshAuth = .agent
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(), old: old, new: agent), [.remove(.sshPassword)])
        var off = old
        off.sshHost = ""
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(), old: old, new: off), [.remove(.sshPassword)],
                       "turning the tunnel off drops its secrets")
    }

    func testSwitchingTrinoBetweenPasswordAndJWTSwapsWhichSlotIsKept() {
        let password = connection { $0.kind = .trino; $0.sshHost = ""; $0.dbAuth = .password }
        var jwt = password
        jwt.dbAuth = .jwt
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(jwt: "t"), old: password, new: jwt),
                       [.remove(.database), .set(.jwt, "t")])
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(password: "p"), old: jwt, new: password),
                       [.set(.database, "p"), .remove(.jwt)])
    }

    func testANewConnectionHasNothingToRemove() {
        let created = connection()
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(sshPassword: "s"), old: nil, new: created),
                       [.set(.sshPassword, "s")])
        XCTAssertEqual(ConnectionSecretPlan.operations(draft: .init(), old: nil, new: created), [])
    }
}
