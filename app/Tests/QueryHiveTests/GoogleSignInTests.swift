import CryptoKit
import XCTest

@testable import QueryHive

/// The parts of Google sign-in that decide something.
///
/// The two ends that need the outside world — the loopback listener and the token exchange — are
/// not here, and cannot be: they need a real client id and a person at a browser. What is here is
/// everything a bug would hide in: the PKCE pair, the authorization URL, the callback's code and
/// state, and the identity inside an `id_token`.
final class GoogleSignInTests: XCTestCase {
    // MARK: PKCE

    func testTheVerifierIsLongEnoughAndTheChallengeIsItsDigest() {
        let pkce = GoogleSignIn.pkce()
        // The specification allows 43 to 128 characters, and base64url of 32 bytes is 43.
        XCTAssertEqual(pkce.verifier.count, 43)
        XCTAssertFalse(pkce.verifier.contains("="), "base64url is unpadded")
        XCTAssertFalse(pkce.verifier.contains("+"))
        XCTAssertFalse(pkce.verifier.contains("/"))

        // S256: the challenge is base64url(SHA-256(verifier)). Recomputed here rather than compared
        // against a constant, so the test says what the relation is.
        let expected = GoogleSignIn.base64URL(Data(SHA256.hash(data: Data(pkce.verifier.utf8))))
        XCTAssertEqual(pkce.challenge, expected)
    }

    func testTwoSignInsGetDifferentPairs() {
        XCTAssertNotEqual(GoogleSignIn.pkce().verifier, GoogleSignIn.pkce().verifier)
    }

    // MARK: The authorization URL

    func testTheAuthorizationURLAsksForWhatTheFlowNeeds() throws {
        let url = GoogleSignIn.authorizationURL(clientID: "client-123",
                                                redirectURI: "http://127.0.0.1:49152",
                                                challenge: "chal", state: "st")
        let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertEqual(components.host, "accounts.google.com")
        XCTAssertEqual(components.path, "/o/oauth2/v2/auth")

        let items = Dictionary(uniqueKeysWithValues: (components.queryItems ?? [])
            .map { ($0.name, $0.value ?? "") })
        XCTAssertEqual(items["client_id"], "client-123")
        XCTAssertEqual(items["redirect_uri"], "http://127.0.0.1:49152")
        XCTAssertEqual(items["response_type"], "code")
        XCTAssertEqual(items["code_challenge"], "chal")
        XCTAssertEqual(items["code_challenge_method"], "S256")
        XCTAssertEqual(items["state"], "st")
        XCTAssertEqual(items["scope"], "openid email profile")
        // No refresh token is asked for: this app keeps no token afterwards.
        XCTAssertNil(items["access_type"])
    }

    // MARK: The callback

    func testTheCallbackYieldsTheCode() throws {
        let url = try XCTUnwrap(URL(string: "http://127.0.0.1:49152/?code=abc123&state=st"))
        XCTAssertEqual(try GoogleSignIn.code(fromCallback: url, expectedState: "st"), "abc123")
    }

    func testACallbackWithTheWrongStateIsRefused() throws {
        // The one defence against a callback that is not the one this app asked for.
        let url = try XCTUnwrap(URL(string: "http://127.0.0.1:49152/?code=abc123&state=other"))
        XCTAssertThrowsError(try GoogleSignIn.code(fromCallback: url, expectedState: "st")) { error in
            guard case GoogleSignIn.Failure.stateMismatch = error else {
                return XCTFail("expected a state mismatch, got \(error)")
            }
        }
    }

    func testAProviderRefusalIsCarriedThrough() throws {
        let url = try XCTUnwrap(URL(string: "http://127.0.0.1:49152/?error=access_denied&state=st"))
        XCTAssertThrowsError(try GoogleSignIn.code(fromCallback: url, expectedState: "st")) { error in
            guard case GoogleSignIn.Failure.refused(let reason) = error else {
                return XCTFail("expected a refusal, got \(error)")
            }
            XCTAssertEqual(reason, "access_denied")
        }
    }

    func testACallbackWithNoCodeSaysSo() throws {
        let url = try XCTUnwrap(URL(string: "http://127.0.0.1:49152/?state=st"))
        XCTAssertThrowsError(try GoogleSignIn.code(fromCallback: url, expectedState: "st")) { error in
            guard case GoogleSignIn.Failure.noCode = error else {
                return XCTFail("expected noCode, got \(error)")
            }
        }
    }

    // MARK: The identity

    func testTheIdentityComesOutOfTheIDToken() throws {
        let token = Self.idToken(["sub": "10769150350006150715113082367",
                                  "email": "faisal@example.com",
                                  "name": "Faisal Nugraha Cayunda"])
        let identity = try GoogleSignIn.identity(fromIDToken: token)
        XCTAssertEqual(identity.subject, "10769150350006150715113082367")
        XCTAssertEqual(identity.email, "faisal@example.com")
        XCTAssertEqual(identity.displayName, "Faisal Nugraha Cayunda")
    }

    func testAnIDTokenWithNoSubjectIsRefused() {
        let token = Self.idToken(["email": "faisal@example.com"])
        XCTAssertThrowsError(try GoogleSignIn.identity(fromIDToken: token)) { error in
            guard case GoogleSignIn.Failure.noIdentity = error else {
                return XCTFail("expected noIdentity, got \(error)")
            }
        }
    }

    func testSomethingThatIsNotATokenIsRefused() {
        XCTAssertThrowsError(try GoogleSignIn.identity(fromIDToken: "not-a-jwt"))
    }

    // MARK: helpers

    /// A JWT-shaped string with `claims` as its payload. The signature is irrelevant here: this
    /// parses what Google would have sent, and nothing verifies a signature.
    private static func idToken(_ claims: [String: String]) -> String {
        let header = GoogleSignIn.base64URL(Data(#"{"alg":"RS256","typ":"JWT"}"#.utf8))
        let payload = GoogleSignIn.base64URL(try! JSONSerialization.data(withJSONObject: claims))
        return "\(header).\(payload).signature"
    }
}
