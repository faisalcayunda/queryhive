import Foundation
import ApplicationServices
import CoreGraphics
import AppKit

// MARK: - Clock

/// Monotonic nanoseconds on the mach clock, the same clock ScreenCaptureKit stamps frames with.
enum Clock {
    private static let timebase: mach_timebase_info_data_t = {
        var t = mach_timebase_info_data_t()
        mach_timebase_info(&t)
        return t
    }()

    static func nowNs() -> UInt64 {
        mach_absolute_time() * UInt64(timebase.numer) / UInt64(timebase.denom)
    }

    static func ms(from start: UInt64, to end: UInt64) -> Double {
        Double(Int64(bitPattern: end &- start)) / 1e6
    }
}

func sleepMs(_ ms: Double) { usleep(useconds_t(max(0, ms) * 1000)) }

// MARK: - Output

/// Scenario name to plan axis, per `bench_app.py --list-scenarios`.
func axis(for scenario: String) -> Any {
    switch scenario.split(separator: "-").first.map(String.init) ?? "" {
    case "ttfr": return 1
    case "rows": return 2
    case "mem": return 3
    case "scroll": return 4
    case "type": return 5
    case "cancel": return 6
    case "launch": return 7
    default: return scenario
    }
}

enum Status {
    static let permission = "tidak diukur (izin OS)"
    static let notMeasured = "[belum diukur]"
    static let unsupported = "tidak mendukung"
}

struct Out {
    let scenario: String
    let app: String
    var label: String?

    func metrics(_ m: [String: Any], notes: String? = nil) {
        var d: [String: Any] = ["source": "qhbench", "scenario": scenario, "axis": axis(for: scenario),
                                "app": app, "metrics": m]
        if let notes { d["notes"] = notes }
        if let label { d["label"] = label }
        write(d)
    }

    func status(_ s: String, notes: String? = nil) {
        var d: [String: Any] = ["source": "qhbench", "scenario": scenario, "axis": axis(for: scenario),
                                "app": app, "status": s]
        if let notes { d["notes"] = notes }
        if let label { d["label"] = label }
        write(d)
    }

    private func write(_ d: [String: Any]) {
        if let data = try? JSONSerialization.data(withJSONObject: d, options: [.sortedKeys, .withoutEscapingSlashes]),
           let s = String(data: data, encoding: .utf8) {
            print(s)
            fflush(stdout)
        }
    }
}

func logErr(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }

// MARK: - Permissions (query only, never request)

struct PermissionState {
    let screenRecording: Bool
    let accessibility: Bool

    /// `CGPreflightScreenCaptureAccess` and `AXIsProcessTrusted()` only read the TCC state. The
    /// `Request` and prompting variants are deliberately never called.
    static func current() -> PermissionState {
        // Test hook: pretend nothing is granted, to exercise the "tidak diukur (izin OS)" path.
        if ProcessInfo.processInfo.environment["QHBENCH_FORCE_NO_PERMS"] == "1" {
            return PermissionState(screenRecording: false, accessibility: false)
        }
        return PermissionState(screenRecording: CGPreflightScreenCaptureAccess(), accessibility: AXIsProcessTrusted())
    }

    var missingNames: [String] {
        var m: [String] = []
        if !screenRecording { m.append("Screen Recording") }
        if !accessibility { m.append("Accessibility") }
        return m
    }
}

// MARK: - Arguments

struct Args {
    private var flags: [String: String] = [:]
    private var switches: Set<String> = []
    var positional: [String] = []

    init(_ raw: [String]) {
        var i = 0
        while i < raw.count {
            let a = raw[i]
            if a.hasPrefix("--") {
                let key = String(a.dropFirst(2))
                if let eq = key.firstIndex(of: "=") {
                    flags[String(key[..<eq])] = String(key[key.index(after: eq)...])
                } else if i + 1 < raw.count, !raw[i + 1].hasPrefix("--") {
                    flags[key] = raw[i + 1]
                    i += 1
                } else {
                    switches.insert(key)
                }
            } else {
                positional.append(a)
            }
            i += 1
        }
    }

    func string(_ k: String) -> String? { flags[k] }
    func int(_ k: String, _ d: Int) -> Int { flags[k].flatMap(Int.init) ?? d }
    func double(_ k: String, _ d: Double) -> Double { flags[k].flatMap(Double.init) ?? d }
    func has(_ k: String) -> Bool { switches.contains(k) || flags[k] != nil }
}

// MARK: - Profiles

struct KeyChord: Codable {
    let key: String
    let modifiers: [String]
}

struct RegionSpec: Codable {
    let roles: [String]
    let pick: String?
    let identifiers: [String]?
    let fallbackWindowFraction: [Double]?
}

struct Profile: Codable {
    let app: String
    let verified: Bool
    let bundleId: String
    let appName: String
    let processName: String
    let run: KeyChord
    let stop: KeyChord
    let selectAll: KeyChord?
    let editor: RegionSpec
    let grid: RegionSpec
    let notes: String?

    static func load(app: String, dirOverride: String?) -> Profile? {
        var dirs: [String] = []
        if let d = dirOverride { dirs.append(d) }
        if let d = ProcessInfo.processInfo.environment["QHBENCH_PROFILES"] { dirs.append(d) }
        let exe = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        // .build/release/qhbench: the package root is two or three levels up.
        var up = exe.deletingLastPathComponent()
        for _ in 0..<4 {
            dirs.append(up.appendingPathComponent("profiles").path)
            up = up.deletingLastPathComponent()
        }
        dirs.append(FileManager.default.currentDirectoryPath + "/profiles")
        dirs.append(FileManager.default.currentDirectoryPath + "/tools/bench/qhbench/profiles")
        for d in dirs {
            let url = URL(fileURLWithPath: d).appendingPathComponent("\(app).json")
            if let data = try? Data(contentsOf: url), let p = try? JSONDecoder().decode(Profile.self, from: data) {
                return p
            }
        }
        return nil
    }
}

// MARK: - Target app

func findApp(_ p: Profile, pidOverride: Int?) -> NSRunningApplication? {
    if let pid = pidOverride { return NSRunningApplication(processIdentifier: pid_t(pid)) }
    return NSRunningApplication.runningApplications(withBundleIdentifier: p.bundleId).first
        ?? NSWorkspace.shared.runningApplications.first { $0.localizedName == p.processName }
}

// MARK: - Helpers

func percentile(_ v: [Double], _ p: Double) -> Double {
    guard !v.isEmpty else { return 0 }
    let s = v.sorted()
    let idx = min(s.count - 1, Int((p * Double(s.count - 1)).rounded(.up)))
    return s[idx]
}

@discardableResult
func sh(_ launch: String, _ args: [String]) -> (status: Int32, out: String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: launch)
    p.arguments = args
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = FileHandle.nullDevice
    do { try p.run() } catch { return (-1, "") }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(data: data, encoding: .utf8) ?? "")
}
