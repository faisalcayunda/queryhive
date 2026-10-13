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
/// **`verify` belongs to every driver.** The engine sends `DB_INSECURE` whenever it is off, and a
/// mode other than `disable` then runs as "encrypt, do not check", `verify-full` included. So the
/// word alone would sometimes claim a check that does not happen. A stored word with `verify`
/// off is therefore its own case, `sslmodeUnverified`, which says so; it is read as stored and
/// never normalised. (Imports write `verify: false` for every driver, which is how such
/// connections exist.)
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

    /// What the menu shows. Trino's are QueryHive's own wording; the others are the mode word.
    var title: String {
        switch self {
        case .trinoHTTPSVerified: "HTTPS · verified"
        case .trinoHTTPSUnverified: "HTTPS · unverified"
        case .trinoPrefer: "Prefer · unverified"
        case .trinoHTTP: "HTTP"
        case .sslmode(let word): word
        case .sslmodeUnverified(let word): "\(word) · unverified"
        }
    }

    /// The longer line VoiceOver reads as the control's value.
    var spokenDescription: String {
        switch self {
        case .trinoHTTPSVerified: "HTTPS, certificate verified"
        case .trinoHTTPSUnverified: "HTTPS, certificate not verified"
        case .trinoPrefer: "Prefer, HTTPS first and plain HTTP as a fallback, certificate not verified"
        case .trinoHTTP: "HTTP, not encrypted"
        case .sslmode(let word):
            switch word {
            case "disable": "Disable, not encrypted"
            case "prefer": "Prefer, encrypted when the server offers it, certificate not verified"
            case "require": "Require, encrypted, certificate not verified"
            case "verify-ca": "Verify CA, encrypted, certificate chain verified"
            case "verify-full": "Verify full, encrypted, certificate chain and host name verified"
            default: word
            }
        case .sslmodeUnverified(let word):
            "\(word), certificate verification is off, so the certificate is not checked"
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
            "Plain HTTP: nothing to verify. A password still forces HTTPS — pick HTTPS to choose how that is verified."
        case .sslmode(let word):
            switch word {
            case "disable": "No encryption."
            case "prefer": "Encrypts when the server offers it, and connects in clear otherwise. The certificate is not checked."
            case "require": "Always encrypts. The certificate is not checked."
            case "verify-ca": "Always encrypts and checks the certificate chain, not the host name."
            case "verify-full": "Always encrypts, checks the certificate chain and that the host name matches."
            default: ""
            }
        case .sslmodeUnverified:
            "Certificate verification is off for this connection, so it encrypts without checking the certificate. Pick the mode again to turn the check back on."
        }
    }

    /// What the menu offers, in the order it shows them. A CA file only means something to a mode
    /// that checks the certificate, so while one is set the others are not offered (blueprint
    /// 9.1). MySQL has no CA setting and no verifying mode, so a path there changes nothing.
    static func options(kind: ConnectionKind, caFile: String) -> [ConnectionTLSChoice] {
        let hasCA = !caFile.trimmingCharacters(in: .whitespaces).isEmpty
        switch kind {
        case .trino:
            return hasCA ? [.trinoHTTPSVerified]
                         : [.trinoHTTPSVerified, .trinoHTTPSUnverified, .trinoPrefer, .trinoHTTP]
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

    /// Writes a choice into the stored fields and nothing else. Every PostgreSQL and MySQL mode
    /// turns verification on (`verify` is the engine's insecure switch for them too); only the
    /// "unverified" case, which is a stored combination and not a menu option, writes it off.
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
