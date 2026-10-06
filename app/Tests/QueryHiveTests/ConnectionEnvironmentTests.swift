import XCTest

@testable import QueryHive

final class ConnectionEnvironmentTests: XCTestCase {
    private func connection(environment: ConnectionEnvironment? = nil,
                            safeMode: ConnectionSafeMode = .full) -> Connection {
        Connection(id: UUID(), name: "wh", color: .blue, kind: .postgres, host: "h", port: 5432,
                   user: "u", database: "d", schema: "", verify: true,
                   safeMode: safeMode, environment: environment)
    }

    private func decode(_ json: String) throws -> Connection {
        try JSONDecoder().decode(Connection.self, from: Data(json.utf8))
    }

    private let base = #""id":"00000000-0000-0000-0000-000000000001","name":"wh""#

    func testAFileWrittenBeforeTheTagLoadsWithoutOne() throws {
        XCTAssertNil(try decode("{\(base)}").environment)
    }

    func testTheTagRoundTrips() throws {
        for environment in ConnectionEnvironment.allCases {
            let original = connection(environment: environment)
            let back = try JSONDecoder().decode(Connection.self, from: JSONEncoder().encode(original))
            XCTAssertEqual(back.environment, environment)
            XCTAssertEqual(back, original)
        }
        let none = connection()
        XCTAssertNil(try JSONDecoder().decode(Connection.self, from: JSONEncoder().encode(none)).environment)
    }

    func testAnUnknownTagIsNilAndDoesNotFailTheFile() throws {
        XCTAssertNil(try decode(#"{\#(base),"environment":"qa"}"#).environment)
        XCTAssertNil(try decode(#"{\#(base),"environment":7}"#).environment)
        XCTAssertEqual(try decode(#"{\#(base),"environment":"prod"}"#).environment, .prod)
    }

    func testEqualityCountsTheTag() {
        let a = connection(environment: .prod)
        var b = a
        XCTAssertEqual(a, b)
        b.environment = .dev
        XCTAssertNotEqual(a, b)
    }

    func testTheTagNeverReachesTheEngine() {
        let plain = connection()
        var tagged = plain
        tagged.environment = .prod
        XCTAssertEqual(AppModel.connectionEnvironment(plain, password: nil),
                       AppModel.connectionEnvironment(tagged, password: nil))
    }
}

final class BadgeSpecTests: XCTestCase {
    private func connection(_ environment: ConnectionEnvironment?, _ mode: ConnectionSafeMode) -> Connection {
        Connection(id: UUID(), name: "wh", color: .blue, kind: .postgres, host: "h", port: 5432,
                   user: "u", database: "d", schema: "", verify: true,
                   safeMode: mode, environment: environment)
    }

    func testEveryLevelAndEnvironmentAtEveryPlace() {
        let environments: [ConnectionEnvironment?] = [nil] + ConnectionEnvironment.allCases.map { $0 }
        for environment in environments {
            for mode in ConnectionSafeMode.allCases {
                let c = connection(environment, mode)
                let status = BadgeSpec.badges(for: c, in: .statusBar)
                XCTAssertEqual(status.count, environment == nil ? 1 : 2, "status bar always has Safe Mode")
                XCTAssertEqual(status.last?.label, mode.title)
                XCTAssertEqual(status.last?.accessibility, "Safe Mode: \(mode.title)")
                XCTAssertEqual(status.last?.detail, mode.detail)
                for place in [BadgeSpec.Place.breadcrumb, .tabChip] {
                    let quiet = BadgeSpec.badges(for: c, in: place)
                    let expected = (environment == nil ? 0 : 1) + (mode == .full ? 0 : 1)
                    XCTAssertEqual(quiet.count, expected)
                    XCTAssertEqual(quiet.contains { $0.label == "Full" }, false)
                }
            }
        }
        XCTAssertTrue(BadgeSpec.badges(for: nil, in: .statusBar).isEmpty)
    }

    func testTheSpecsSayWhatTheyShow() {
        XCTAssertEqual(BadgeSpec.environment(.prod).label, "PROD")
        XCTAssertEqual(BadgeSpec.environment(.prod).accessibility, "Environment: production")
        XCTAssertEqual(BadgeSpec.environment(.staging).label, "STAGING")
        XCTAssertEqual(BadgeSpec.environment(.dev).accessibility, "Environment: development")
        XCTAssertEqual(BadgeSpec.safeMode(.confirm).glyph, "shield.lefthalf.filled")
        XCTAssertEqual(BadgeSpec.safeMode(.readOnly).label, "Read only")
        // Every glyph exists on this OS: a missing symbol would draw nothing at all.
        for spec in ConnectionEnvironment.allCases.map(BadgeSpec.environment)
            + ConnectionSafeMode.allCases.map(BadgeSpec.safeMode) {
            XCTAssertNotNil(NSImage(systemSymbolName: spec.glyph, accessibilityDescription: nil), spec.glyph)
        }
    }

    func testTheTabChipReadsTheBadgesAfterTheStage() {
        let badges = BadgeSpec.badges(for: connection(.prod, .confirm), in: .tabChip)
        XCTAssertEqual(TabChipSpec(title: "t", stage: .running, isSelected: false, badges: badges).value,
                       "running. Environment: production. Safe Mode: Confirm.")
        XCTAssertEqual(TabChipSpec(title: "t", stage: .idle, isSelected: false, badges: badges).value,
                       "Environment: production. Safe Mode: Confirm.")
        XCTAssertEqual(TabChipSpec(title: "t", stage: .done, isSelected: false).value, "finished")
    }

    func testTheStatusBarCarriesBothBadgesForAProdConfirmConnection() {
        let model = AppModel()
        let c = connection(.prod, .confirm)
        model.connections = [c]
        model.rebuildTree()
        model.selectedTab?.connectionID = c.id
        let specs = BadgeSpec.badges(for: model.statusConnectionTarget, in: .statusBar)
        XCTAssertEqual(specs.map(\.label), ["PROD", "Confirm"])
    }
}
