import XCTest

@testable import QueryHive

/// The connection-URL corpus (DBX-29). Written for this project: the point of each case is a URL a
/// person could really paste and what the form must do with it, and above all that **a TLS setting
/// in the URL is never lowered or dropped without a word**.
final class ConnectionURLTests: XCTestCase {
    private func parsed(_ url: String, file: StaticString = #filePath, line: UInt = #line) throws -> ConnectionURL.Parsed {
        try XCTUnwrap(try ConnectionURL.parse(url), "not read: \(url)", file: file, line: line)
    }

    private func failure(_ url: String, file: StaticString = #filePath, line: UInt = #line) -> String? {
        do {
            _ = try ConnectionURL.parse(url)
            XCTFail("expected a refusal: \(url)", file: file, line: line)
            return nil
        } catch let error as ConnectionURL.Failure {
            return error.message
        } catch {
            XCTFail("\(error)", file: file, line: line)
            return nil
        }
    }

    // MARK: What a URL without TLS words has always meant

    func testTheAuthorityAndPathAreReadAsBefore() throws {
        let pg = try parsed("postgresql://ana:p%40ss@db.internal:6432/sales")
        XCTAssertEqual(pg.kind, .postgres)
        XCTAssertEqual(pg.host, "db.internal")
        XCTAssertEqual(pg.port, 6432)
        XCTAssertEqual(pg.user, "ana")
        XCTAssertEqual(pg.password, "p@ss")
        XCTAssertEqual(pg.database, "sales")
        XCTAssertNil(pg.sslmode, "no word means the form keeps the driver's default")
        XCTAssertNil(pg.verify)

        let trino = try parsed("trino://u@coord:8443/hive/web")
        XCTAssertEqual(trino.kind, .trino)
        XCTAssertEqual(trino.database, "hive")
        XCTAssertEqual(trino.schema, "web")
        XCTAssertEqual(trino.scheme, "http")
        XCTAssertEqual(try parsed("https://coord/hive").scheme, "https")

        XCTAssertEqual(try parsed("mysql://r@my.internal/shop").port, 3306)
        XCTAssertNil(try ConnectionURL.parse(""))
        XCTAssertNil(try ConnectionURL.parse("postgresql://"))
    }

    // MARK: PostgreSQL keeps libpq's words verbatim

    func testEveryLibpqWordSurvivesAsItWasWritten() throws {
        for word in ["disable", "prefer", "require", "verify-ca", "verify-full"] {
            XCTAssertEqual(try parsed("postgresql://u@h/db?sslmode=\(word)").sslmode, word)
        }
    }

    func testVerifyFullIsNeverFoldedIntoRequire() throws {
        // `require` encrypts and does not check the certificate; `verify-full` checks it and the
        // name. Folding one into the other is the downgrade this exists to prevent.
        XCTAssertEqual(try parsed("postgres://u@h/db?sslmode=verify-full").sslmode, "verify-full")
        XCTAssertNotEqual(try parsed("postgres://u@h/db?sslmode=verify-full").sslmode, "require")
        XCTAssertEqual(try parsed("postgres://u@h/db?sslmode=verify-ca").sslmode, "verify-ca")
    }

    func testTheParameterAndItsValueAreCaseInsensitiveAndTrimmed() throws {
        XCTAssertEqual(try parsed("postgresql://u@h/db?SSLMODE=Verify-Full").sslmode, "verify-full")
        XCTAssertEqual(try parsed("postgresql://u@h/db?sslmode=%20require%20").sslmode, "require")
    }

    func testAnUnknownOrUnsupportedPostgresModeIsRefusedByName() throws {
        for word in ["banana", "allow", "", "verify_full"] {
            let message = try XCTUnwrap(failure("postgresql://u@h/db?sslmode=\(word)"))
            XCTAssertTrue(message.contains("sslmode=\(word)"), message)
            XCTAssertTrue(message.contains("verify-full"), "the way out is in the sentence: \(message)")
        }
    }

    // MARK: MySQL

    func testMySQLModesMapToTheThreeTheDriverHas() throws {
        for (given, mapped) in [("DISABLED", "disable"), ("PREFERRED", "prefer"), ("REQUIRED", "require"),
                                ("required", "require")] {
            XCTAssertEqual(try parsed("mysql://u@h/db?ssl-mode=\(given)").sslmode, mapped, given)
        }
        XCTAssertEqual(try parsed("mysql://u@h/db?useSSL=true").sslmode, "require")
        XCTAssertEqual(try parsed("mysql://u@h/db?useSSL=false").sslmode, "disable")
        XCTAssertEqual(try parsed("mysql://u@h/db?sslmode=require").sslmode, "require")
    }

    func testMySQLVerifyModesAreRefusedByNameNotDowngraded() throws {
        for word in ["VERIFY_CA", "VERIFY_IDENTITY"] {
            let message = try XCTUnwrap(failure("mysql://u@h/db?ssl-mode=\(word)"))
            XCTAssertTrue(message.contains("ssl-mode=\(word)"), message)
            XCTAssertTrue(message.contains("verif"), message)
        }
        XCTAssertNotNil(failure("mysql://u@h/db?ssl-mode=SOMETIMES"))
        XCTAssertNotNil(failure("mysql://u@h/db?useSSL=maybe"))
    }

    // MARK: Trino

    func testTrinoReadsTheSameWordsIntoItsTransportAndVerifyFlag() throws {
        let prefer = try parsed("trino://u@c/hive?sslmode=prefer")
        XCTAssertEqual(prefer.scheme, "prefer")
        let require = try parsed("trino://u@c/hive?sslmode=require")
        XCTAssertEqual(require.scheme, "https")
        XCTAssertEqual(require.verify, false, "require encrypts and does not check")
        let verified = try parsed("trino://u@c/hive?sslmode=verify-full")
        XCTAssertEqual(verified.scheme, "https")
        XCTAssertEqual(verified.verify, true)
        XCTAssertEqual(try parsed("trino://u@c/hive?sslmode=disable").scheme, "http")
        XCTAssertNotNil(failure("trino://u@c/hive?sslmode=never"))
    }

    // MARK: Parameters that must not override what the person can see

    func testHostHostaddrAndPortParametersAreIgnoredAndReported() throws {
        let url = try parsed("postgresql://u@db.internal:5432/app?host=evil.example&hostaddr=10.9.9.9&port=1&application_name=x")
        XCTAssertEqual(url.host, "db.internal", "the authority is what the form shows, and what is used")
        XCTAssertEqual(url.port, 5432)
        XCTAssertEqual(Set(url.ignored), ["host", "hostaddr", "port", "application_name"])
    }
}
