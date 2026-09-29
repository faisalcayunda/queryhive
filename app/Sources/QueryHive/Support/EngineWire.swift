import Foundation

/// The engine's event wire format: one compact JSON object per line, named by its `event` field.
///
/// It has its own type because the wire outlives the code that produces it. `RustEngine` decodes
/// the lines the FFI hands it, and the frozen record in `tests/golden/**` is the same lines written
/// by the engine that came before it. A decoder inside the engine would let the app and that record
/// drift, and the one that drifted would be the app's: `lib.rs`'s "the protocol is the deliverable".
enum EngineWire {
    /// A decoder per call, not one shared instance. `JSONDecoder` is a class, and the engine decodes
    /// on the queue the call came in on while the app runs; sharing one would be sharing its caches
    /// across queues for no gain, since there is one JSON object per line.
    static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        // Load-bearing, and the reason this can't be a bare `JSONDecoder()` in two places: the
        // wire is snake_case and `Event`'s properties are its camelCase — `query_id` → `queryId`,
        // `object_columns` → `objectColumns`, `elapsed_ms` → `elapsedMs`, `catalog_count` →
        // `catalogCount`. Every one of those four has been an event that failed to decode at some
        // point in this migration, and a case that decoded to `nil` would read as "the engine sent
        // nothing" rather than as a decoder bug.
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }

    /// One line as an event, or `nil` when the line is not one.
    ///
    /// `nil` rather than a thrown error, because a line the app cannot read is not a run failure:
    /// it is kept in the run's stderr so the user sees what the engine said instead of losing it.
    /// Keeping that decision here is what stops the app and the frozen record from disagreeing
    /// about it.
    static func event(in line: Data) -> Event? {
        try? decoder().decode(Event.self, from: line)
    }
}
