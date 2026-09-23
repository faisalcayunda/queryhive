import Foundation

/// The engine's event wire format: one compact JSON object per line, named by its `event` field.
///
/// It has its own type because two engines speak it. `PythonEngine` decodes the child process's
/// stdout, `RustEngine` decodes the lines the FFI call handed back, and the two have to agree on
/// what a line means or the swap in blueprint §1.6 is a behaviour change rather than a swap. A
/// decoder inside each engine would let them drift, and the one that drifted would be the new one:
/// the old engine is the one with a frozen golden record behind it
/// (`tests/golden/**`, `lib.rs`'s "the protocol is the deliverable").
enum EngineWire {
    /// A decoder per call, not one shared instance. `JSONDecoder` is a class, and both engines
    /// decode on a background queue while the app runs; sharing one would be sharing its caches
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
    /// `nil` rather than a thrown error, because neither engine treats a line it cannot read as a
    /// run failure: the Python engine appends it to the run's stderr so the user sees what the
    /// engine said instead of losing it, and the FFI has no other place to put it. Keeping that
    /// decision here is what stops the two engines from disagreeing about it.
    static func event(in line: Data) -> Event? {
        try? decoder().decode(Event.self, from: line)
    }
}
