import Foundation

/// One answer to "how is this connection encrypted", named once for all three drivers and read
/// from, and written to, the fields a connection already stores: `scheme`, `verify` and `sslmode`.
/// Nothing is stored under this type, so a saved connection means exactly what it did before it
/// existed.
///
/// Trino has four outcomes, the ones `TrinoTransport` documents. PostgreSQL and MySQL keep their
/// own SSL mode words, which already carry the verification answer (libpq's `verify-ca` and
/// `verify-full`); MySQL has no verifying mode, and that is `ConnectionKind.sslModes`' business.
///
/// **How the engine reads the stored fields**, which is what the titles say (`crates/qh-ffi/src/config.rs`,
/// `AppModel.connectionEnvironment`):
///
/// - PostgreSQL and MySQL: `disable` is clear. `prefer` encrypts when the server offers it, never
///   checks the certificate, and may fall back to clear when the server answers "no TLS". `require`
///   always encrypts and never checks. `verify-ca` and `verify-full` always encrypt and check the
///   certificate chain; the PostgreSQL driver checks the host name in both. MySQL has none of the
///   verifying words.
/// - **`verify` belongs to every driver.** The engine sends `DB_INSECURE` whenever it is off, and any
///   mode other than `disable` then runs as "always encrypt, never check", `verify-full` included,
///   and `prefer` loses its fallback to clear. A stored word with `verify` off is therefore its own
///   case, `sslmodeUnverified`. It is read as stored and never normalised. (Imports made before
///   `AppModel.applyTLS` set `verify` with the mode wrote `verify: false` for every driver, which is
///   how such connections exist; they are shown honestly and not rewritten.)
/// - So "· unverified" is shown exactly when the engine will not check the certificate: `prefer` and
///   `require` always, and `verify-ca` or `verify-full` stored with `verify` off. Trino follows the
///   same rule: HTTPS with `verify` off and Prefer say so; HTTP has nothing to verify, but is not
///   always clear, because a password or port 443 or 8443 raises it to HTTPS (with the stored
///   `verify`), which its description says.
enum ConnectionTLSChoice: Hashable, Identifiable {
    case trinoHTTPSVerified
    case trinoHTTPSUnverified
    case trinoPrefer
    case trinoHTTP
    /// PostgreSQL and MySQL: the mode word as stored, with the certificate check as it is stored.
    case sslmode(String)
    /// PostgreSQL and MySQL: a mode word other than `disable` stored with `verify` off, so the
    /// connection encrypts without checking the certificate whatever the word suggests.
    case sslmodeUnverified(String)

    var id: Self { self }

    /// The choice a connection's stored fields amount to. A blank Trino scheme is plain HTTP and
    /// a blank `sslmode` is the driver's default, as the engine reads them
    /// (`TrinoTransport(stored:)`, `AppModel.connectionEnvironment`). Nothing is normalised.
    init(kind: ConnectionKind, scheme: String, verify: Bool, sslmode: String) {
        switch kind {
        case .trino:
            switch TrinoTransport(stored: scheme) {
            case .https: self = verify ? .trinoHTTPSVerified : .trinoHTTPSUnverified
            case .prefer: self = .trinoPrefer
            case .http: self = .trinoHTTP
            }
        case .postgres, .mysql:
            let word = sslmode.isEmpty ? kind.defaultSSLMode : sslmode
            self = verify || word == "disable" ? .sslmode(word) : .sslmodeUnverified(word)
        }
    }

    init(_ draft: ConnectionDraft) {
        self.init(kind: draft.kind, scheme: draft.scheme, verify: draft.verifyTLS, sslmode: draft.sslmode)
    }

    /// What the menu shows. Trino's are QueryHive's own wording; the others are the mode word, with
    /// "· unverified" wherever the engine will not check the certificate.
    var title: String {
        switch self {
        case .trinoHTTPSVerified: "HTTPS · verified"
        case .trinoHTTPSUnverified: "HTTPS · unverified"
        case .trinoPrefer: "Prefer · unverified"
        case .trinoHTTP: "HTTP"
        case .sslmode(let word):
            switch word {
            case "prefer", "require": "\(word) · unverified"
            default: word
            }
        case .sslmodeUnverified(let word):
            // `prefer` with verification off is not the `prefer` above: the engine runs it as
            // `require` (it must encrypt, never falls back to clear, and does not check the
            // certificate), and the title says what runs. The help says what is stored.
            word == "prefer" ? "require · unverified" : "\(word) · unverified"
        }
    }

    /// The mode word a PostgreSQL or MySQL choice stands for, as the engine is sent it.
    var modeWord: String? {
        switch self {
        case .sslmode(let word), .sslmodeUnverified(let word): word
        default: nil
        }
    }

    /// The longer line VoiceOver reads as the control's value.
    var spokenDescription: String {
        switch self {
        case .trinoHTTPSVerified: "HTTPS, certificate verified"
        case .trinoHTTPSUnverified: "HTTPS, certificate not verified"
        case .trinoPrefer: "Prefer, HTTPS first and plain HTTP as a fallback, certificate not verified"
        case .trinoHTTP:
            "HTTP, not encrypted unless a password is set or the port is 443 or 8443, which raise it to HTTPS"
        case .sslmode(let word):
            switch word {
            case "disable": "Disable, not encrypted"
            case "prefer": "Prefer, encrypted when the server offers it, certificate not verified"
            case "require": "Require, encrypted, certificate not verified"
            case "verify-ca": "Verify CA, encrypted, certificate chain and host name verified"
            case "verify-full": "Verify full, encrypted, certificate chain and host name verified"
            default: word
            }
        case .sslmodeUnverified(let word):
            switch word {
            case "prefer": "Prefer with certificate verification off, always encrypted, never falls back to clear, certificate not verified"
            case "require": "Require, encrypted, certificate not verified"
            default: "\(word) with certificate verification off, encrypted, certificate not verified"
            }
        }
    }

    /// An SF Symbol (1 and 2 only: the binary is built with the macOS 14 SDK). Decorative; the
    /// title and the spoken description carry the meaning.
    var glyph: String {
        switch self {
        case .trinoHTTPSVerified: "lock.fill"
        case .trinoHTTPSUnverified, .sslmodeUnverified: "lock"
        case .trinoPrefer, .trinoHTTP: "lock.open"
        case .sslmode(let word):
            switch word {
            case "verify-ca", "verify-full": "lock.fill"
            case "require": "lock"
            default: "lock.open"
            }
        }
    }

    /// The tooltip: what the choice does, in the sentences the form already used.
    var help: String {
        switch self {
        case .trinoHTTPSVerified: "HTTPS encrypts and checks the certificate."
        case .trinoHTTPSUnverified: "HTTPS encrypts, and the certificate is not checked."
        case .trinoPrefer:
            "Prefer: HTTPS first, plain HTTP only when the coordinator has no TLS. The certificate is not checked."
        case .trinoHTTP:
            "Plain HTTP: nothing to verify. A password, or port 443 or 8443, still raises it to HTTPS — pick HTTPS to choose how that is verified."
        case .sslmode(let word):
            switch word {
            case "disable": "No encryption."
            case "prefer": "Encrypts when the server offers it, and connects in clear otherwise. The certificate is not checked."
            case "require": "Always encrypts. The certificate is not checked."
            case "verify-ca": "Always encrypts and checks the certificate chain and the host name."
            case "verify-full": "Always encrypts, checks the certificate chain and that the host name matches."
            default: ""
            }
        case .sslmodeUnverified(let word):
            switch word {
            case "prefer":
                "Stored as prefer with certificate verification off, which the engine runs as require: it always encrypts, never falls back to clear, and does not check the certificate. Pick \"prefer · unverified\" to allow the fallback."
            case "require":
                "Always encrypts. The certificate is not checked."
            default:
                "Certificate verification is off for this connection, so it encrypts without checking the certificate. Pick \(word) again to turn the check on."
            }
        }
    }

    /// What the menu offers, in the order it shows them. A CA file only means something to a mode
    /// that checks the certificate, so while one is set the others are not offered (blueprint
    /// 9.1). MySQL has no CA setting and no verifying mode, so a path there changes nothing. A
    /// Trino JWT is sent only over HTTPS (the engine refuses it over Prefer or HTTP), so with one
    /// only the two HTTPS choices are offered.
    static func options(kind: ConnectionKind, caFile: String, dbAuth: DatabaseAuth = .password) -> [ConnectionTLSChoice] {
        let hasCA = !caFile.trimmingCharacters(in: .whitespaces).isEmpty  // as `ConnectionDraft.hasCA`
        switch kind {
        case .trino:
            if hasCA { return [.trinoHTTPSVerified] }
            if dbAuth == .jwt { return [.trinoHTTPSVerified, .trinoHTTPSUnverified] }
            return [.trinoHTTPSVerified, .trinoHTTPSUnverified, .trinoPrefer, .trinoHTTP]
        case .postgres:
            let modes = hasCA ? ["verify-ca", "verify-full"] : kind.sslModes
            return modes.map { .sslmode($0) }
        case .mysql:
            return kind.sslModes.map { .sslmode($0) }
        }
    }
}

extension ConnectionDraft {
    /// How the form currently encrypts, read from its stored fields.
    var tls: ConnectionTLSChoice { ConnectionTLSChoice(self) }

    /// Writes a choice into the stored fields and nothing else. A PostgreSQL or MySQL mode picked
    /// from the menu also writes `verify = true`, because `verify` is the engine's insecure switch
    /// for them too and a pick is the person's whole answer. What that changes depends on the mode:
    /// for `prefer` it allows the fallback to clear again, for `require` nothing, and for
    /// `verify-ca` and `verify-full` it is what makes the verification take effect. Only the
    /// "unverified" case, a stored combination and not a menu option, writes it off.
    ///
    /// The Trino transports go through `setScheme`, so the standard port follows as it does when
    /// the transport is picked today. `verify` is the choice's own for HTTPS and untouched for
    /// Prefer and HTTP, which cannot verify but should offer the last answer again when HTTPS is
    /// picked next. The caller offers only `options(kind:caFile:)`, so a choice always matches the
    /// draft's driver.
    mutating func setTLS(_ choice: ConnectionTLSChoice) {
        switch choice {
        case .trinoHTTPSVerified:
            setScheme(TrinoTransport.https.rawValue); verifyTLS = true
        case .trinoHTTPSUnverified:
            setScheme(TrinoTransport.https.rawValue); verifyTLS = false
        case .trinoPrefer:
            setScheme(TrinoTransport.prefer.rawValue)
        case .trinoHTTP:
            setScheme(TrinoTransport.http.rawValue)
        case .sslmode(let word):
            sslmode = word; verifyTLS = true
        case .sslmodeUnverified(let word):
            sslmode = word; verifyTLS = false
        }
    }
}
