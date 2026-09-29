import Foundation

/// One query tab, as it is written into the session store and read back.
///
/// A plain value rather than a `QueryTab`, because what survives a relaunch is the *work*, not
/// the run: the SQL, the connection it was written against, and where a Run would send it. The
/// results, the log and the run state are deliberately left out. A result set belongs to
/// `qh-result-store`, and restoring a grid of yesterday's rows would promise something about
/// data that may since have changed.
///
/// One thing this does not carry is an object-listing tab's scope. A tab opened from the tree
/// comes back as an ordinary empty query tab rather than one that immediately re-runs the
/// browse. Re-running a listing on launch is a network call the user did not ask for, and the
/// tree is one click away.
///
/// Keys carry no underscores on purpose. The engine hands this blob back inside a `session`
/// event, and `EngineWire` decodes every event with `convertFromSnakeCase`; a key such as
/// `active_tab_id` would come back as `activeTabId` and miss an `activeTabID` property. No
/// underscores means the strategy is the identity and the two shapes cannot drift.
struct SessionTab: Codable, Equatable {
    var id: String
    var title: String
    var sql: String
    /// The connection's UUID, or `nil` for a tab that was never pointed at one.
    var connection: String?
    var destination: String
    var format: String
    var outputName: String
    /// The output folder's path, or `nil` when the tab never picked one.
    var outputDirectory: String?
    var rowLimit: Int
    var contextDatabase: String
    var contextSchema: String
    var targetCatalog: String
    var targetSchema: String
    var targetTable: String
    var writeMode: String
}

extension SessionTab {
    /// The snapshot of a tab, taken when the session is saved.
    init(_ tab: QueryTab) {
        id = tab.id.uuidString
        title = tab.title
        sql = tab.sql
        connection = tab.connectionID?.uuidString
        destination = tab.destination.rawValue
        format = tab.format.rawValue
        outputName = tab.outputName
        outputDirectory = tab.outputDirectory?.path
        rowLimit = tab.rowLimit
        contextDatabase = tab.contextDatabase
        contextSchema = tab.contextSchema
        targetCatalog = tab.targetCatalog
        targetSchema = tab.targetSchema
        targetTable = tab.targetTable
        writeMode = tab.writeMode.rawValue
    }

    /// The tab this snapshot describes.
    ///
    /// A value the engine no longer knows — a format or a write mode that was removed — falls back
    /// to the same default a new tab starts with rather than failing the restore. One unknown word
    /// in one field is not a reason to lose every tab.
    ///
    /// The identity is restored too, because `active_tab_id` names it: a tab that came back under a
    /// fresh UUID could never be the one the session says was in front.
    func tab() -> QueryTab {
        let tab = QueryTab(title: title, id: UUID(uuidString: id) ?? UUID())
        tab.sql = sql
        tab.connectionID = connection.flatMap(UUID.init(uuidString:))
        tab.destination = Destination(rawValue: destination) ?? .file
        tab.format = ExportFormat(rawValue: format) ?? .csv
        tab.outputName = outputName
        if let outputDirectory, !outputDirectory.isEmpty {
            tab.outputDirectory = URL(fileURLWithPath: outputDirectory)
        }
        tab.rowLimit = rowLimit
        tab.contextDatabase = contextDatabase
        tab.contextSchema = contextSchema
        tab.targetCatalog = targetCatalog
        tab.targetSchema = targetSchema
        tab.targetTable = targetTable
        tab.writeMode = WriteMode(rawValue: writeMode) ?? .create
        return tab
    }
}
