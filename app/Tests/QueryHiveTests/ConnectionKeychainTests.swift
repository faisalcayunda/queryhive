import XCTest

@testable import QueryHive

/// The Keychain items a connection owns (blueprint w11 section 8).
///
/// The account strings are a contract with the engine: `qh_credentials::account_for` must produce
/// the same text, and `crates/qh-credentials/src/lib.rs` pins the same literals from its side. If
/// either side changes a prefix, one of the two tests fails, instead of a saved secret quietly
/// becoming unreadable to the MCP server or the app.
final class ConnectionKeychainTests: XCTestCase {
    private let id = UUID(uuidString: "E621E1F8-C36C-495A-93FC-0C247A3E6E5F")!

    func testEachSlotHasTheAccountTheEngineBuilds() {
        XCTAssertEqual(ConnectionKeychain.account(.database, for: id), "E621E1F8-C36C-495A-93FC-0C247A3E6E5F",
                       "the password keeps the bare UUID it has always had")
        XCTAssertEqual(ConnectionKeychain.account(.sshPassword, for: id), "ssh-password:E621E1F8-C36C-495A-93FC-0C247A3E6E5F")
        XCTAssertEqual(ConnectionKeychain.account(.sshPassphrase, for: id), "ssh-passphrase:E621E1F8-C36C-495A-93FC-0C247A3E6E5F")
        XCTAssertEqual(ConnectionKeychain.account(.jwt, for: id), "jwt:E621E1F8-C36C-495A-93FC-0C247A3E6E5F")
    }

    func testNoPrefixedAccountCanBeReadAsAUUIDOrAProfile() {
        for slot in ConnectionKeychain.Slot.allCases where slot != .database {
            let account = ConnectionKeychain.account(slot, for: id)
            XCTAssertNil(UUID(uuidString: account), "\(slot)")
            XCTAssertFalse(account.hasPrefix("profile:"), "\(slot)")
        }
    }

    func testTheFourAccountsOfOneConnectionAreFourDifferentItems() {
        let accounts = ConnectionKeychain.Slot.allCases.map { ConnectionKeychain.account($0, for: id) }
        XCTAssertEqual(Set(accounts).count, 4)
    }

    /// The real login keychain, opt-in like the Rust side (`QH_TEST_KEYCHAIN=1`): writing to a user's
    /// keychain during an ordinary `swift test` is not something a suite does unasked.
    func testTheFourSlotsRoundTripAndDeleteAllTakesOnlyThem() throws {
        guard ProcessInfo.processInfo.environment["QH_TEST_KEYCHAIN"] == "1" else {
            throw XCTSkip("set QH_TEST_KEYCHAIN=1 to exercise the real login keychain")
        }
        let mine = UUID()
        addTeardownBlock { try? ConnectionKeychain.deleteAll(for: mine) }
        XCTAssertFalse(ConnectionKeychain.contains(slot: .jwt, for: mine))
        for slot in ConnectionKeychain.Slot.allCases {
            try ConnectionKeychain.set("v-\(slot)", slot: slot, for: mine)
        }
        for slot in ConnectionKeychain.Slot.allCases {
            XCTAssertTrue(ConnectionKeychain.contains(slot: slot, for: mine), "\(slot)")
            XCTAssertEqual(try ConnectionKeychain.get(slot: slot, for: mine), "v-\(slot)")
        }
        // The database-slot spellings every earlier caller uses still mean the database slot.
        XCTAssertEqual(try ConnectionKeychain.get(for: mine), "v-database")
        try ConnectionKeychain.set("second", slot: .jwt, for: mine)
        XCTAssertEqual(try ConnectionKeychain.get(slot: .jwt, for: mine), "second", "an item is replaced, not duplicated")

        try ConnectionKeychain.deleteAll(for: mine)
        for slot in ConnectionKeychain.Slot.allCases {
            XCTAssertFalse(ConnectionKeychain.contains(slot: slot, for: mine), "\(slot)")
        }
        // Nothing to renew once it is gone, and an item this build wrote never prompts.
        XCTAssertFalse(try ConnectionKeychain.renewIfPrompting(slot: .database, for: mine))
    }

    /// A store that keeps one value and fails the first `failingSets` writes.
    private final class FlakyStore: SecretStoring {
        var value: String?
        var failingSets: Int
        var failingDelete = false
        struct Boom: Error {}
        init(value: String?, failingSets: Int) { self.value = value; self.failingSets = failingSets }
        func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool { value != nil }
        func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? { value }
        func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws {
            if failingSets > 0 { failingSets -= 1; throw Boom() }
            value = secret
        }
        func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws {
            if failingDelete { throw Boom() }
            value = nil
        }
    }

    func testRenewKeepsTheSecretWhenTheSecondWriteWorks() throws {
        let store = FlakyStore(value: "pw", failingSets: 1)
        XCTAssertTrue(try ConnectionKeychain.renew(slot: .database, for: id, in: store))
        XCTAssertEqual(store.value, "pw")
    }

    /// The silent credential-loss path: delete went through, both writes failed.
    func testRenewReportsASecretLostWhenBothWritesFail() {
        let store = FlakyStore(value: "pw", failingSets: 2)
        XCTAssertThrowsError(try ConnectionKeychain.renew(slot: .sshPassword, for: id, in: store)) { error in
            guard let lost = error as? ConnectionKeychain.SecretLost else { return XCTFail("\(error)") }
            XCTAssertEqual(lost.slot, .sshPassword)
        }
        XCTAssertNil(store.value, "the secret really is gone, which is why the error must reach the person")
    }

    func testAFailureBeforeTheDeleteIsNotASecretLost() {
        let store = FlakyStore(value: "pw", failingSets: 0)
        store.failingDelete = true
        XCTAssertThrowsError(try ConnectionKeychain.renew(slot: .database, for: id, in: store)) { error in
            XCTAssertFalse(error is ConnectionKeychain.SecretLost)
        }
        XCTAssertEqual(store.value, "pw")
    }

    func testTheLostMessageNamesTheConnectionAndTheSecrets() {
        let text = ConnectionKeychain.lostMessage(connection: "Prod", secrets: ["password", "SSH passphrase"])
        XCTAssertTrue(text.contains("Prod") && text.contains("password, SSH passphrase"), text)
    }
}
