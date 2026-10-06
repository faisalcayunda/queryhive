import XCTest

@testable import QueryHive

/// An engine that records every one of the protocol's seven requirements, and answers `run` and
/// `runIntoStore` from a script, synchronously on the calling thread. `MockEngine` cannot stand in:
/// it inherits the protocol's empty `warmUp`, so it could never show that a wrapper forwarded it.
final class RecordingEngine: DatabaseEngine, @unchecked Sendable {
    struct Call: Equatable {
        var command: String
        var env: [String: String]
    }

    private let lock = NSLock()
    private(set) var runs: [Call] = []
    private(set) var intoStore: [Call] = []
    private(set) var blocking: [Call] = []
    private(set) var warmUps: [[String: String]] = []
    private(set) var terminated = 0
    private(set) var storesMade = 0
    private(set) var storesFromRows = 0

    /// What each command answers with: events, then an exit status.
    var script: [String: (events: [Event], status: Int32)] = [:]

    private func answer(_ command: String, _ onEvent: (Event) -> Void, _ onExit: (Int32, String) -> Void) {
        let scripted = script[command] ?? ([], 0)
        scripted.events.forEach(onEvent)
        onExit(scripted.status, "")
    }

    @discardableResult
    func run(_ command: String, env: [String: String], onEvent: @escaping (Event) -> Void,
             onExit: @escaping (Int32, String) -> Void) -> (any EngineRun)? {
        lock.lock(); runs.append(Call(command: command, env: env)); lock.unlock()
        answer(command, onEvent, onExit)
        return MockRun(command: command)
    }

    func terminateAll() { lock.lock(); terminated += 1; lock.unlock() }

    func runBlocking(_ command: String, env: [String: String]) {
        lock.lock(); blocking.append(Call(command: command, env: env)); lock.unlock()
    }

    func makeResultStore() throws -> StoreRows {
        lock.lock(); storesMade += 1; lock.unlock()
        return StoreRows(handle: FakeResultHandle())
    }

    @discardableResult
    func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                      onEvent: @escaping (Event) -> Void, onExit: @escaping (Int32, String) -> Void) -> (any EngineRun)? {
        lock.lock(); intoStore.append(Call(command: command, env: env)); lock.unlock()
        answer(command, onEvent, onExit)
        return MockRun(command: command)
    }

    func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        lock.lock(); storesFromRows += 1; lock.unlock()
        return try TestStores.makeStore(columns: columns, rows: rows)
    }

    func warmUp(env: [String: String]) {
        lock.lock(); warmUps.append(env); lock.unlock()
    }
}

/// The gate and the center behind it: a refused host key reaches a person exactly once, the only
/// way a key is ever accepted is the one button on the first-use sheet, and no state that means
/// "the key is wrong" has any path to acceptance (blueprint w11 sections 5.4, 5.6 and 9.4).
@MainActor
final class HostKeyGateTests: XCTestCase {
    private let fingerprint = "SHA256:" + String(repeating: "A", count: 43)
    private let other = "SHA256:" + String(repeating: "B", count: 43)

    private func errorEvent(state: String = "unknown", fingerprint: String? = nil, caCovered: Bool = false,
                            pinned: String? = nil, port: Int = 22) throws -> Event {
        let shown = fingerprint ?? self.fingerprint
        let pin = pinned.map { "\"\($0)\"" } ?? "null"
        let line = #"{"event":"error","message":"the bastion's host key is not trusted","host_key":{"state":"\#(state)","host":"bastion.corp","port":\#(port),"alias":"prod","key_type":"ssh-ed25519","fingerprint":"\#(shown)","app_known_hosts":"/Users/x/Library/Application Support/QueryHive/known_hosts","ca_covered":\#(caCovered),"recorded":[{"fingerprint":"SHA256:old","key_type":"ssh-rsa","source":"user","path":"/Users/x/.ssh/known_hosts","line":3}],"pinned":\#(pin)}}"#
        return try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))
    }

    final class ClockBox {
        var now = Date()
    }

    private struct Rig {
        let engine = RecordingEngine()
        let center: HostKeyCenter
        let gate: HostKeyGate
        let clock = ClockBox()

        init() {
            let clock = self.clock
            center = HostKeyCenter(now: { clock.now })
            gate = HostKeyGate(wrapping: engine, center: center)
        }

        func run(_ command: String, _ vars: [String: String]) {
            gate.run(command, env: vars, onEvent: { _ in }, onExit: { _, _ in })
        }

        var pinned: [RecordingEngine.Call] { engine.runs.filter { $0.command == "test" } }
    }

    private let tunnel = ["SSH_HOST": "bastion.corp", "DB_HOST": "db", "DB_PASSWORD": "db-secret"]

    // MARK: Reaching the person

    func testARefusedUnknownKeyOnARunOpensTheSheetAndStillShowsTheErrorToTheCaller() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        var seen: [Event] = []
        rig.gate.run("preview", env: tunnel, onEvent: { seen.append($0) }, onExit: { _, _ in })

        XCTAssertEqual(seen.map(\.event), ["error"], "the failed operation still reports its error")
        let prompt = try XCTUnwrap(rig.center.prompt)
        XCTAssertEqual(prompt.fingerprint, fingerprint)
        XCTAssertEqual(prompt.host, "bastion.corp")
        XCTAssertTrue(prompt.isTrustable)
        XCTAssertEqual(rig.engine.runs.first?.env["SSH_HOST_KEY_DETAIL"], "1", "the gate asks for the detail")
    }

    func testPreviewAndExplainRunThroughRunIntoStoreAndPromptLikeRun() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        let store = try rig.gate.makeResultStore()
        rig.gate.runIntoStore("preview", env: tunnel, store: store, onEvent: { _ in }, onExit: { _, _ in })
        XCTAssertNotNil(rig.center.prompt, "a gate that watched only `run` would never prompt for a preview")
        XCTAssertEqual(rig.engine.intoStore.first?.env["SSH_HOST_KEY_DETAIL"], "1")
    }

    func testARunWithoutABastionIsLeftExactlyAsItWas() {
        let rig = Rig()
        rig.run("preview", ["DB_HOST": "db"])
        XCTAssertEqual(rig.engine.runs.first?.env, ["DB_HOST": "db"])
    }

    func testAllSevenRequirementsReachTheWrappedEngine() throws {
        let rig = Rig()
        rig.run("tables", [:])
        let store = try rig.gate.makeResultStore()
        rig.gate.runIntoStore("preview", env: [:], store: store, onEvent: { _ in }, onExit: { _, _ in })
        rig.gate.terminateAll()
        rig.gate.runBlocking("session", env: ["A": "1"])
        _ = try rig.gate.storeFromRows(columns: [Event.Column(name: "a", type: "text")], rows: [["x"]])
        rig.gate.warmUp(env: ["DB_HOST": "db"])

        XCTAssertEqual(rig.engine.runs.map(\.command), ["tables"])
        XCTAssertEqual(rig.engine.intoStore.map(\.command), ["preview"])
        XCTAssertEqual(rig.engine.terminated, 1)
        XCTAssertEqual(rig.engine.blocking, [.init(command: "session", env: ["A": "1"])])
        XCTAssertEqual(rig.engine.storesMade, 1)
        XCTAssertEqual(rig.engine.storesFromRows, 1)
        XCTAssertEqual(rig.engine.warmUps, [["DB_HOST": "db"]],
                       "warmUp comes from a protocol extension; a wrapper that forgot it would switch warm-up off for the whole app")
    }

    func testAWarmUpNeverPromptsAndNeverAsksForTheDetail() {
        let rig = Rig()
        rig.gate.warmUp(env: tunnel)
        XCTAssertNil(rig.center.prompt)
        XCTAssertNil(rig.engine.warmUps.first?["SSH_HOST_KEY_DETAIL"])
    }

    func testTwoEventsForTheSameKeyOpenOneSheet() throws {
        let rig = Rig()
        rig.engine.script["objects"] = ([try errorEvent()], 1)
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        rig.run("objects", tunnel)
        let first = try XCTUnwrap(rig.center.prompt)
        rig.run("preview", tunnel)
        XCTAssertEqual(rig.center.prompt?.id, first.id, "the second event joins the sheet that is up")
    }

    // MARK: The one way a key is accepted

    func testTrustPinsTheFingerprintOfTheEventForOneTestRunAndKeepsTheRunsOwnEnvironment() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        rig.engine.script["test"] = ([], 0)
        rig.run("preview", tunnel)
        let prompt = try XCTUnwrap(rig.center.prompt)
        XCTAssertTrue(rig.center.holdsEnvironment)

        rig.center.trust(prompt)

        let call = try XCTUnwrap(rig.engine.runs.last)
        XCTAssertEqual(call.command, "test")
        XCTAssertEqual(call.env["SSH_HOST_KEY_ACCEPT"], fingerprint, "the event's own fingerprint")
        XCTAssertEqual(call.env["DB_PASSWORD"], "db-secret", "the failed run's environment, so the same connection")
        XCTAssertEqual(rig.center.trustedCount, 1)
        XCTAssertEqual(rig.center.lastTrusted?.host, "bastion.corp")
        XCTAssertNil(rig.center.prompt, "the sheet closes")
        XCTAssertFalse(rig.center.holdsEnvironment, "the secrets are dropped with it")
        XCTAssertEqual(rig.engine.runs.filter { $0.env["SSH_HOST_KEY_ACCEPT"] != nil }.count, 1,
                       "no other run ever carries a pin")
    }

    func testAPromptForAnotherKeyCannotBeTrustedWithAnOldOne() throws {
        let rig = Rig()
        rig.engine.script["objects"] = ([try errorEvent()], 1)
        rig.run("objects", tunnel)
        let stale = try XCTUnwrap(rig.center.prompt)
        rig.center.dismiss()

        rig.engine.script["objects"] = ([try errorEvent(fingerprint: other)], 1)
        rig.run("objects", tunnel)
        XCTAssertEqual(rig.center.prompt?.fingerprint, other)

        rig.center.trust(stale)
        XCTAssertTrue(rig.pinned.isEmpty, "the fingerprint the person was shown is the one that gets pinned, and no other")
    }

    func testNoStateThatMeansTheKeyIsWrongHasAWayToAcceptIt() throws {
        for state in ["changed", "revoked", "certificate", "certificate_expected", "pin_mismatch",
                      "record_failed", "store_unsafe"] {
            let rig = Rig()
            rig.engine.script["preview"] = ([try errorEvent(state: state)], 1)
            rig.run("preview", tunnel)
            let prompt = try XCTUnwrap(rig.center.prompt, state)
            XCTAssertFalse(prompt.isTrustable, state)
            rig.center.trust(prompt)
            XCTAssertTrue(rig.pinned.isEmpty, "\(state) was trusted")
            XCTAssertNil(rig.engine.runs.first(where: { $0.env["SSH_HOST_KEY_ACCEPT"] != nil }), state)
            XCTAssertFalse(prompt.explanation.lowercased().contains("accept"), state)
        }
    }

    func testAHostThatShouldPresentACertificateIsNeverOfferedEvenIfItsStateSaysUnknown() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent(state: "unknown", caCovered: true)], 1)
        rig.run("preview", tunnel)
        let prompt = try XCTUnwrap(rig.center.prompt)
        XCTAssertFalse(prompt.isTrustable)
        rig.center.trust(prompt)
        XCTAssertTrue(rig.pinned.isEmpty)
    }

    func testAnEventWithNoFingerprintHasNothingToTrust() throws {
        let line = #"{"event":"error","message":"m","host_key":{"state":"unknown","host":"b"}}"#
        let rig = Rig()
        rig.engine.script["preview"] = ([try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))], 1)
        rig.run("preview", tunnel)
        let prompt = try XCTUnwrap(rig.center.prompt)
        XCTAssertFalse(prompt.isTrustable)
    }

    func testAnEnvironmentOlderThanFiveMinutesIsNotReused() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        rig.run("preview", tunnel)
        let prompt = try XCTUnwrap(rig.center.prompt)
        rig.clock.now = rig.clock.now.addingTimeInterval(HostKeyCenter.environmentLifetime + 1)
        rig.center.trust(prompt)
        XCTAssertTrue(rig.pinned.isEmpty, "secrets that sat in memory past their time are not sent again")
    }

    func testDismissingForgetsTheEnvironment() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        rig.run("preview", tunnel)
        XCTAssertTrue(rig.center.holdsEnvironment)
        rig.center.dismiss()
        XCTAssertFalse(rig.center.holdsEnvironment)
        XCTAssertNil(rig.center.prompt)
    }

    // MARK: Outcomes of the pinned run

    func testAPinnedRunThatIsRefusedAgainShowsWhyAndTrustsNothing() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        rig.engine.script["test"] = ([try errorEvent(state: "pin_mismatch", fingerprint: other, pinned: fingerprint)], 1)
        rig.run("preview", tunnel)
        let prompt = try XCTUnwrap(rig.center.prompt)
        rig.center.trust(prompt)
        guard case .refused(let detail) = rig.center.status else { return XCTFail("\(rig.center.status)") }
        XCTAssertEqual(detail.state, "pin_mismatch")
        XCTAssertEqual(rig.center.trustedCount, 0)
        XCTAssertNotNil(rig.center.prompt, "the sheet stays, now explaining; it does not open a second one")
        // And there is no second try on the same sheet.
        rig.center.trust(prompt)
        XCTAssertEqual(rig.pinned.count, 1)
    }

    func testAKeyRecordedButAConnectionThatStillFailsIsSaidInThoseWords() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent()], 1)
        let failure = #"{"event":"error","message":"password authentication failed"}"#
        rig.engine.script["test"] = ([try XCTUnwrap(EngineWire.event(in: Data(failure.utf8)))], 1)
        rig.run("preview", tunnel)
        rig.center.trust(try XCTUnwrap(rig.center.prompt))
        XCTAssertEqual(rig.center.status, .trustedButFailed("password authentication failed"))
        XCTAssertEqual(rig.center.trustedCount, 1, "the key is recorded and that is not undone")
    }

    // MARK: Words the sheet shows

    func testTheRemovalCommandIsQuotedAndNamesTheFileThatHoldsTheRecord() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent(state: "changed", port: 2222)], 1)
        rig.run("preview", tunnel)
        let advice = try XCTUnwrap(rig.center.prompt).removalAdvice
        XCTAssertEqual(advice.first, "ssh-keygen -R '[bastion.corp]:2222' -f '/Users/x/.ssh/known_hosts'")
        XCTAssertTrue(advice.last?.contains("known_hosts.old") == true, "the copy it leaves is mentioned")
        XCTAssertFalse(advice.joined().lowercased().contains("trust"), "no word that invites acceptance")
    }

    func testShellQuotingSurvivesAShellForEveryAwkwardValue() throws {
        for value in ["plain", "o'brien", "a b", "x;rm -rf ~", "$(id)", "`id`", "-flag", "line\nbreak", "*", "'"] {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/sh")
            process.arguments = ["-c", "printf %s \(HostKeyPrompt.shellQuote(value))"]
            let out = Pipe()
            process.standardOutput = out
            try process.run()
            process.waitUntilExit()
            let text = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            XCTAssertEqual(text, value, "quoting \(value.debugDescription)")
        }
    }

    func testTheFingerprintIsGroupedInFoursForReadingAndStaysWholeForThePin() throws {
        let rig = Rig()
        rig.engine.script["preview"] = ([try errorEvent(fingerprint: "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG")], 1)
        rig.run("preview", tunnel)
        let prompt = try XCTUnwrap(rig.center.prompt)
        XCTAssertEqual(prompt.groupedFingerprint, "SHA256: abcd efgh ijkl mnop qrst uvwx yz01 2345 6789 ABCD EFG")
        XCTAssertEqual(prompt.fingerprint, "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG")
        XCTAssertEqual(prompt.hostSpelling, "bastion.corp")
    }
}
