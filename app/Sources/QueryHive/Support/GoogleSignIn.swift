import AppKit
import CryptoKit
import Foundation
import Network

/// Signing in with Google, the way a desktop application is allowed to do it.
///
/// The authorization-code flow with PKCE, and the browser the user already has. Google's own
/// documentation is the reason for each of the two choices that look unusual:
///
/// - **A loopback redirect, not a custom URL scheme.** A `Desktop app` OAuth client accepts
///   `http://127.0.0.1:<port>`; the loopback flow is deprecated for the iOS, Android and Chrome
///   client types and explicitly still supported for desktop. So the app listens on an ephemeral
///   loopback port, sends the browser there, and reads the `code` out of the one request it gets.
/// - **PKCE, with no client secret.** A desktop application cannot keep a secret: anything shipped
///   in the bundle is readable. The code verifier is what proves the callback is ours.
///
/// # What is verified, and what is not
///
/// The parts that decide something are pure and tested: the PKCE pair, the authorization URL, the
/// callback's `code`/`state`/`error`, and the identity inside the `id_token`. The two ends that
/// need the network, the token exchange and the listener, are not exercised by a test: they need a
/// real client id and a person at a browser. That split is deliberate and is why the pieces are
/// separate functions rather than one opaque `signIn`.
enum GoogleSignIn {
    /// What a sign-in needs and this build does not ship.
    ///
    /// The client id belongs to whoever owns the Google Cloud project, not to the repository, so
    /// it is read from the bundle's `QHGoogleClientID` or from the environment. Absent, the pane
    /// says so rather than offering a button that cannot work.
    static var clientID: String? {
        if let value = ProcessInfo.processInfo.environment["QH_GOOGLE_CLIENT_ID"],
           !value.trimmingCharacters(in: .whitespaces).isEmpty {
            return value
        }
        if let value = Bundle.main.object(forInfoDictionaryKey: "QHGoogleClientID") as? String,
           !value.trimmingCharacters(in: .whitespaces).isEmpty {
            return value
        }
        return nil
    }

    enum Failure: LocalizedError {
        case noClientID
        case refused(String)
        case malformedCallback
        case noCode
        case stateMismatch
        case tokenExchange(String)
        case noIdentity

        var errorDescription: String? {
            switch self {
            case .noClientID:
                "No Google client id is configured. Add QHGoogleClientID to the app's Info.plist, "
                    + "or set QH_GOOGLE_CLIENT_ID, using a Desktop app client from Google Cloud."
            case .refused(let reason):
                "Google refused the sign-in: \(reason)."
            case .malformedCallback:
                "The sign-in redirect was not a URL this app can read."
            case .noCode:
                "The sign-in redirect carried no authorization code."
            case .stateMismatch:
                "The sign-in redirect did not carry the state this app sent."
            case .tokenExchange(let reason):
                "Google would not exchange the authorization code: \(reason)."
            case .noIdentity:
                "The token Google returned named no subject."
            }
        }
    }

    /// What a completed sign-in tells the account row.
    struct Identity: Equatable {
        let subject: String
        let email: String?
        let displayName: String?
    }

    // MARK: PKCE

    struct PKCE: Equatable {
        let verifier: String
        let challenge: String
    }

    /// A fresh verifier and its S256 challenge.
    ///
    /// The verifier is 32 random bytes in base64url, which is 43 characters: the specification
    /// allows 43 to 128, and 256 bits of randomness is the whole of its strength.
    static func pkce() -> PKCE {
        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        let verifier = base64URL(Data(bytes))
        let digest = SHA256.hash(data: Data(verifier.utf8))
        return PKCE(verifier: verifier, challenge: base64URL(Data(digest)))
    }

    // MARK: The two URLs

    static func authorizationURL(clientID: String, redirectURI: String,
                                 challenge: String, state: String) -> URL {
        var components = URLComponents(string: "https://accounts.google.com/o/oauth2/v2/auth")!
        components.queryItems = [
            URLQueryItem(name: "client_id", value: clientID),
            URLQueryItem(name: "redirect_uri", value: redirectURI),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "scope", value: "openid email profile"),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            URLQueryItem(name: "state", value: state),
            // No `access_type=offline`: this asks for an identity, not for a refresh token to keep
            // calling Google's APIs after the user has gone. Nothing here stores a token.
            URLQueryItem(name: "prompt", value: "select_account"),
        ]
        return components.url!
    }

    /// The `code` out of the loopback callback, or the failure it carries.
    ///
    /// `state` is checked here rather than by the caller: it is the one defence against a
    /// callback that is not the one this app asked for, and a caller that forgot to check it
    /// would be a caller that cannot be made correct from outside.
    static func code(fromCallback url: URL, expectedState: String) throws -> String {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let items = components.queryItems else {
            throw Failure.malformedCallback
        }
        func value(_ name: String) -> String? {
            items.first { $0.name == name }?.value
        }
        if let error = value("error") {
            throw Failure.refused(error)
        }
        guard let state = value("state"), state == expectedState else {
            throw Failure.stateMismatch
        }
        guard let code = value("code"), !code.isEmpty else {
            throw Failure.noCode
        }
        return code
    }

    /// The identity inside an `id_token`.
    ///
    /// The signature is not checked here, and that is the specification's own rule rather than a
    /// shortcut: a token that arrived in the response to this app's own token exchange, over TLS
    /// from the token endpoint, is trustworthy without a second verification. Verifying it would
    /// mean fetching Google's keys and pinning an issuer and an audience, which buys nothing for a
    /// token that has not travelled through a browser.
    static func identity(fromIDToken token: String) throws -> Identity {
        let parts = token.split(separator: ".")
        guard parts.count == 3,
              let payload = base64URLDecode(String(parts[1])),
              let claims = try? JSONSerialization.jsonObject(with: payload) as? [String: Any],
              let subject = claims["sub"] as? String, !subject.isEmpty else {
            throw Failure.noIdentity
        }
        return Identity(subject: subject,
                        email: claims["email"] as? String,
                        displayName: claims["name"] as? String)
    }

    // MARK: base64url

    static func base64URL(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func base64URLDecode(_ text: String) -> Data? {
        var padded = text
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        while padded.count % 4 != 0 { padded += "=" }
        return Data(base64Encoded: padded)
    }
}
