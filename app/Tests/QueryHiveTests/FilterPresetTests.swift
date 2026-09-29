import XCTest

@testable import QueryHive

/// Filter presets: what a saved set becomes, and where it is filed.
///
/// The decision under test is the one that makes a preset still mean something next time: it is
/// keyed by **column name**, not by the position the column happened to have, and it is filed per
/// connection + table — or on the tab when the query is hand-written and there is no table.
final class FilterPresetTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any QueryTab or AppModel is built: both read the stored output directory.
        isolateConnectionStore()
    }

    private let columns = [Event.Column(name: "a", type: "varchar"),
                           Event.Column(name: "b", type: "bigint"),
                           Event.Column(name: "c", type: "varchar")]

    // MARK: Encoding the filters on screen

    func testAPresetCarriesBothShapesOfFilterKeyedByColumnName() throws {
        let preset = try XCTUnwrap(FilterPreset.from(name: "Bandung",
                                                     filters: [0: .text("32.01"),
                                                               2: .values(["x", "a"])],
                                                     columns: columns))

        XCTAssertEqual(preset.filters, [
            FilterPreset.Entry(column: "a", values: nil, text: "32.01"),
            // Values are sorted, so two presets with the same selection compare equal.
            FilterPreset.Entry(column: "c", values: ["a", "x"], text: nil),
        ])
    }

    func testNoFiltersMeansNoPreset() {
        XCTAssertNil(FilterPreset.from(name: "empty", filters: [:], columns: columns))
    }

    // MARK: Applying it again

    func testApplyingMapsNamesBackToTheResultsOwnColumnOrder() {
        // A preset made when the columns came back a, b, c, applied to a result that came back
        // c, b, a: the names are what tie the two together.
        let preset = FilterPreset(name: "p", filters: [
            FilterPreset.Entry(column: "c", values: ["x"], text: nil),
            FilterPreset.Entry(column: "a", values: nil, text: "32"),
        ])

        let reordered = [Event.Column(name: "c", type: "varchar"),
                         Event.Column(name: "b", type: "bigint"),
                         Event.Column(name: "a", type: "varchar")]
        let (filters, missing) = preset.resolve(columns: reordered)

        XCTAssertTrue(missing.isEmpty)
        XCTAssertEqual(filters[0], .values(["x"]), "c is first here")
        XCTAssertEqual(filters[2], .text("32"), "a is last here")
    }

    func testAColumnTheResultDoesNotHaveIsReportedNotDroppedSilently() {
        let preset = FilterPreset(name: "p", filters: [
            FilterPreset.Entry(column: "nik", values: nil, text: "32"),
            FilterPreset.Entry(column: "a", values: nil, text: "1"),
        ])

        let (filters, missing) = preset.resolve(columns: columns)

        XCTAssertEqual(missing, ["nik"])
        XCTAssertEqual(filters[0], .text("1"))
    }

    func testARepeatedColumnNameResolvesToTheFirstColumn() {
        let preset = FilterPreset(name: "p", filters: [
            FilterPreset.Entry(column: "a", values: nil, text: "1"),
        ])

        let (filters, _) = preset.resolve(columns: columns)
        XCTAssertEqual(filters.count, 1)
        XCTAssertEqual(filters[0], .text("1"))
    }

    func testApplyingToATabSetsItsFiltersAndReturnsTheMissingNames() {
        let tab = QueryTab(title: "Q")
        tab.preview = PreviewResult(columns: columns,
                                    rows: [["1", "2", "x"]], truncated: false,
                                    queryID: nil, elapsedMS: 0)
        let preset = FilterPreset(name: "p", filters: [
            FilterPreset.Entry(column: "b", values: nil, text: "2"),
            FilterPreset.Entry(column: "ghost", values: nil, text: "?"),
        ])

        let missing = tab.applyFilterPreset(preset)

        XCTAssertEqual(missing, ["ghost"])
        XCTAssertEqual(tab.columnFilters[1], .text("2"))
    }

    // MARK: Where presets live

    func testStoreRoundTripsThroughAFileBackedDefaults() throws {
        let name = "qh-filterpreset-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }

        let identity = try XCTUnwrap(FilterPresetStore.identity(connection: UUID(),
                                                                table: "hive.analytics.t"))
        let preset = FilterPreset(name: "p", filters: [
            FilterPreset.Entry(column: "a", values: nil, text: "32"),
        ])

        FilterPresetStore.save([identity: [preset]], in: defaults)
        XCTAssertEqual(FilterPresetStore.load(in: defaults)[identity], [preset])

        // Saving nothing clears the key rather than leaving an empty object behind.
        FilterPresetStore.save([:], in: defaults)
        XCTAssertNil(defaults.object(forKey: FilterPresetStore.defaultsKey))
    }

    func testIdentityNeedsAConnectionAndATable() {
        XCTAssertNil(FilterPresetStore.identity(connection: nil, table: "t"))
        XCTAssertNil(FilterPresetStore.identity(connection: UUID(), table: nil))
        XCTAssertNil(FilterPresetStore.identity(connection: UUID(), table: ""))
        XCTAssertNotNil(FilterPresetStore.identity(connection: UUID(), table: "t"))
    }

    func testAHandWrittenQueryKeepsItsPresetsOnTheTabAndWritesNothing() {
        // `sourceTable` is nil for a hand-written query, so there is no durable identity and the
        // preset stays with the tab. This also means the test never touches the user's defaults.
        let model = AppModel()
        let tab = QueryTab(title: "Q")
        tab.preview = PreviewResult(columns: columns,
                                    rows: [["1", "2", "x"]], truncated: false,
                                    queryID: nil, elapsedMS: 0)
        tab.columnFilters[1] = .text("2")
        XCTAssertNil(model.presetIdentity(for: tab))
        let durable = model.filterPresets

        XCTAssertTrue(model.saveFilterPreset(named: "  Dua  ", in: tab))

        XCTAssertEqual(model.filterPresets(for: tab).map(\.name), ["Dua"])
        XCTAssertEqual(model.filterPresets, durable, "nothing was written to the durable store")

        model.deleteFilterPreset(named: "Dua", in: tab)
        XCTAssertTrue(model.filterPresets(for: tab).isEmpty)
    }

    func testSavingWithNoFiltersOrABlankNameIsRefused() {
        let model = AppModel()
        let tab = QueryTab(title: "Q")
        tab.preview = PreviewResult(columns: columns, rows: [], truncated: false,
                                    queryID: nil, elapsedMS: 0)

        XCTAssertFalse(model.saveFilterPreset(named: "x", in: tab), "nothing filtered")
        tab.columnFilters[0] = .text("1")
        XCTAssertFalse(model.saveFilterPreset(named: "   ", in: tab), "blank name")
    }
}
