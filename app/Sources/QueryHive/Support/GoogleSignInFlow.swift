import AppKit
import Foundation
import Network

/// The half of Google sign-in that needs a browser and a network.
///
/// Kept apart from `GoogleSignIn` on purpose: everything that decides something (the PKCE pair,
/// the URLs, the callback, the identity) lives there and is tested, and this file holds only the
/// two ends that need the outside world.
enum GoogleSignInFlow {
    /// The whole round trip: listen, send the browser, take the callback, exchange, read.
    static func signIn(clientID: String) async throws -> GoogleSignIn.Identity {
        let listener = try LoopbackCallback()
        let port = try await listener.ready()
        let redirectURI = "http://127.0.0.1:\(port)"
        let pkce = GoogleSignIn.pkce()
        let state = UUID().uuidString
        let url = GoogleSignIn.authorizationURL(clientID: clientID, redirectURI: redirectURI,
                                                challenge: pkce.challenge, state: state)
        await MainActor.run { _ = NSWorkspace.shared.open(url) }
        let callback = try await listener.callback()
        listener.stop()
        let code = try GoogleSignIn.code(fromCallback: callback, expectedState: state)
        let idToken = try await exchange(code: code, verifier: pkce.verifier,
                                         clientID: clientID, redirectURI: redirectURI)
        return try GoogleSignIn.identity(fromIDToken: idToken)
    }

    /// The authorization code for a token. No secret is sent: a desktop application has none to
    /// keep, which is what the verifier is for.
    private static func exchange(code: String, verifier: String, clientID: String,
                                 redirectURI: String) async throws -> String {
        var request = URLRequest(url: URL(string: "https://oauth2.googleapis.com/token")!)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        var form = URLComponents()
        form.queryItems = [
            URLQueryItem(name: "code", value: code),
            URLQueryItem(name: "client_id", value: clientID),
            URLQueryItem(name: "redirect_uri", value: redirectURI),
            URLQueryItem(name: "code_verifier", value: verifier),
            URLQueryItem(name: "grant_type", value: "authorization_code"),
        ]
        request.httpBody = Data((form.percentEncodedQuery ?? "").utf8)

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw GoogleSignIn.Failure.tokenExchange("the request got no HTTP response")
        }
        guard http.statusCode == 200 else {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw GoogleSignIn.Failure.tokenExchange("HTTP \(http.statusCode) \(body)")
        }
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let idToken = object["id_token"] as? String else {
            throw GoogleSignIn.Failure.tokenExchange("the response carried no id_token")
        }
        return idToken
    }
}

/// Resumes a continuation at most once, from whichever thread arrives first.
///
/// A `var` captured by the two closures would be a data race, and Swift 6 makes it a compile error
/// rather than a warning. The lock is what makes "only the first answer counts" true across the
/// listener's queue and the caller's.
private final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if done { return false }
        done = true
        return true
    }
}

/// A one-request HTTP listener on the loopback interface.
///
/// Ephemeral port, loopback only, and it answers exactly one request before it is stopped: the
/// redirect comes back here once, and a listener that stayed open would be a port a user's machine
/// keeps answering on for no reason.
final class LoopbackCallback {
    private let listener: NWListener
    private let queue = DispatchQueue(label: "id.data-ecosystem.queryhive.oauth")

    init() throws {
        let parameters = NWParameters.tcp
        parameters.requiredInterfaceType = .loopback
        parameters.allowLocalEndpointReuse = true
        listener = try NWListener(using: parameters, on: .any)
    }

    /// The port the listener got, once it can accept.
    func ready() async throws -> UInt16 {
        let once = Once()
        let listener = self.listener
        return try await withCheckedThrowingContinuation { continuation in
            listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if once.claim() {
                        continuation.resume(returning: listener.port?.rawValue ?? 0)
                    }
                case .failed(let error):
                    if once.claim() { continuation.resume(throwing: error) }
                default:
                    break
                }
            }
            listener.start(queue: queue)
        }
    }

    /// The one request's URL.
    func callback() async throws -> URL {
        let once = Once()
        return try await withCheckedThrowingContinuation { continuation in
            listener.newConnectionHandler = { [weak self] connection in
                guard let self else { return }
                self.listener.newConnectionHandler = nil
                self.receive(connection) { result in
                    if once.claim() { continuation.resume(with: result) }
                }
            }
        }
    }

    private func receive(_ connection: NWConnection,
                         completion: @escaping (Result<URL, Error>) -> Void) {
        connection.start(queue: queue)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16_384) { data, _, _, error in
            defer { connection.cancel() }
            if let error {
                completion(.failure(error))
                return
            }
            guard let data, let request = String(data: data, encoding: .utf8),
                  let requestLine = request.split(separator: "\r\n", maxSplits: 1).first else {
                completion(.failure(GoogleSignIn.Failure.malformedCallback))
                return
            }
            // `GET /?code=…&state=… HTTP/1.1`
            let fields = requestLine.split(separator: " ")
            guard fields.count >= 2,
                  let url = URL(string: "http://127.0.0.1\(fields[1])") else {
                completion(.failure(GoogleSignIn.Failure.malformedCallback))
                return
            }
            // A page a person can close, rather than a blank tab or a raw JSON body.
            let body = "<!doctype html><meta charset=\"utf-8\"><title>QueryHive</title>"
                + "<body style=\"font:14px -apple-system;padding:2rem\">"
                + "You can close this window and go back to QueryHive.</body>"
            let head = "HTTP/1.1 200 OK\r\n"
                + "Content-Type: text/html; charset=utf-8\r\n"
                + "Content-Length: \(body.utf8.count)\r\n"
                + "Connection: close\r\n\r\n"
            connection.send(content: Data((head + body).utf8),
                            completion: .contentProcessed { _ in
                completion(.success(url))
            })
        }
    }

    func stop() {
        listener.newConnectionHandler = nil
        listener.cancel()
    }
}
