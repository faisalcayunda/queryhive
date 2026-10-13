import XCTest

@testable import QueryHive

/// What the engine makes of a PostgreSQL or MySQL connection's stored TLS fields.
///
/// A Swift copy of `crates/qh-ffi/src/config.rs`: the mode word maps to a TLS mode, and
/// `DB_INSECURE` turns any mode but `disable` into one that encrypts and never checks. Only
/// `prefer` can fall back to clear (`qh-driver-postgres/src/tls.rs`, `qh-driver-mysql/src/tls.rs`).
/// The copy is read from the environment the app really sends (`AppModel.connectionEnvironment`),
/// so a test cannot pass by asserting on the fields alone.
enum EngineTLSMode: Equatable {
    case disable
    /// Encrypts when the server offers it, never checks, may fall back to clear.
    case prefer
    /// Always encrypts and checks the certificate chain.
    case require
    /// Always encrypts, never checks.
    case requireNoVerify

    init(environment env: [String: String]) {
        let required: EngineTLSMode
        switch env["DB_SSLMODE"] ?? "" {
        case "disable": required = .disable
        case "require": required = .requireNoVerify
        case "verify-ca", "verify-full": required = .require
        default: required = .prefer
        }
        self = env["DB_INSECURE"] == "1" && required != .disable ? .requireNoVerify : required
    }

    init(_ connection: Connection) {
        self.init(environment: AppModel.connectionEnvironment(connection, secrets: ConnectionSecrets()))
    }
}
