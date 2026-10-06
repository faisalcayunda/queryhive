//! The kernel sandbox, proved and not argued (blueprint section 14.9).
//!
//! The helper is started exactly as the app starts it: `sandbox-exec` with the fixed profile
//! (`helpers/analytics/profile.sb`), `-D` parameters for the four paths, an empty
//! environment, the spill directory as the working directory. Its hidden command
//! `--self-test-confinement` tries each thing the helper must not be able to do, and one it
//! must, and reports. The same command is first run *without* the sandbox, so a probe that
//! reports "denied" means the sandbox said no, not that the probe is broken.
//!
//! Where the machine cannot apply a sandbox at all (a build running inside another
//! sandbox), the test says so and stops instead of failing, because it would prove
//! nothing either way. The line it prints names the reason.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HELPER: &str = env!("CARGO_BIN_EXE_queryhive-analytics");
const PROFILE: &str = include_str!("../profile.sb");
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// `probe -> "allowed" | "denied ..."`
fn probes(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(' ')?;
            Some((name.to_owned(), rest.to_owned()))
        })
        .collect()
}

fn can_sandbox_at_all() -> Result<(), String> {
    let status = Command::new(SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{SANDBOX_EXEC} cannot run: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&status.stderr).trim().to_owned())
    }
}

struct Layout {
    _root: tempfile::TempDir,
    home: PathBuf,
    spill: PathBuf,
    components: PathBuf,
    helper: PathBuf,
    fixture: PathBuf,
    /// Files the profile must keep the helper from reading: the app's data, the app's
    /// own spill, and a keychain.
    protected: Vec<PathBuf>,
    elsewhere: PathBuf,
}

fn layout() -> Layout {
    // In the target directory's own scratch space, not `$TMPDIR`: the helper binary is
    // copied here (a debug build is hundreds of megabytes), and a copy is what the app
    // does too, so the copy belongs on the disk the build already uses.
    let root = tempfile::Builder::new()
        .prefix("confinement-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    // The path may contain a symlink (`/var` is one on macOS), and the sandbox matches
    // resolved paths.
    let base = root.path().canonicalize().unwrap();
    let home = base.join("home");
    let app_support = home.join("Library/Application Support/QueryHive");
    let components = app_support.join("Components/analytics");
    let spill = home.join("Library/Caches/QueryHive/spill-analytics");
    for dir in [
        &components,
        &spill,
        &home.join("elsewhere"),
        &home.join("data"),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // The helper sits where the app puts it: inside the protected tree, in the component
    // directory.
    let helper = components.join("queryhive-analytics");
    std::fs::copy(HELPER, &helper).unwrap();
    let fixture = home.join("data/people.csv");
    std::fs::write(&fixture, "id,name\n1,alpha\n").unwrap();
    let app_spill = home.join("Library/Caches/QueryHive/spill");
    let keychains = home.join("Library/Keychains");
    for dir in [&app_spill, &keychains] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let protected = vec![
        app_support.join("secrets.db"),
        app_spill.join("qhs-1-1-0123456789abcdef.spill"),
        keychains.join("login.keychain-db"),
    ];
    for path in &protected {
        std::fs::write(path, "not for the helper").unwrap();
    }
    Layout {
        elsewhere: home.join("elsewhere"),
        _root: root,
        home,
        spill,
        components,
        helper,
        fixture,
        protected,
    }
}

fn run(layout: &Layout, sandboxed: bool, listener: &Path) -> HashMap<String, String> {
    let address = std::fs::read_to_string(listener).unwrap();
    let mut probe_args = vec![
        "--self-test-confinement".to_owned(),
        format!("read_ok={}", layout.fixture.display()),
        format!("write_denied={}", layout.elsewhere.display()),
        format!("connect={address}"),
    ];
    probe_args.extend(
        layout
            .protected
            .iter()
            .map(|path| format!("read_denied={}", path.display())),
    );
    let mut command = if sandboxed {
        let mut command = Command::new(SANDBOX_EXEC);
        command.args(["-p", PROFILE]);
        for (key, value) in [
            ("HELPER", layout.helper.as_path()),
            ("SPILL_DIR", layout.spill.as_path()),
            ("HOME", layout.home.as_path()),
            ("COMPONENT_DIR", layout.components.as_path()),
        ] {
            command.arg("-D").arg(format!("{key}={}", value.display()));
        }
        command.arg(&layout.helper);
        command
    } else {
        Command::new(&layout.helper)
    };
    let output = command
        .args(probe_args)
        .env_clear()
        // The temporary directory the probe will try to write to.
        .env("TMPDIR", layout.home.join("elsewhere"))
        .current_dir(&layout.spill)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the helper did not run (sandboxed: {sandboxed}): {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    probes(&String::from_utf8_lossy(&output.stdout))
}

#[test]
fn the_helper_can_read_and_spill_and_nothing_else() {
    if let Err(reason) = can_sandbox_at_all() {
        eprintln!("SKIPPED: this machine cannot apply a sandbox here ({reason}); confinement is not proved");
        return;
    }
    let layout = layout();
    // A real listener, so a refused connection means the sandbox refused it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address_file = layout.home.join("address");
    std::fs::write(&address_file, listener.local_addr().unwrap().to_string()).unwrap();

    // Control: with no sandbox every probe succeeds. If one did not, "denied" below would
    // mean nothing.
    let free = run(&layout, false, &address_file);
    for name in [
        "write_spill",
        "write_tmp",
        "write_elsewhere",
        "read_ok",
        "read_denied_0",
        "read_denied_1",
        "read_denied_2",
        "connect",
        "exec",
    ] {
        assert_eq!(
            free.get(name).map(String::as_str),
            Some("allowed"),
            "unsandboxed {name}: {free:?}"
        );
    }

    let confined = run(&layout, true, &address_file);
    let allowed = |name: &str| confined.get(name).map(|r| r == "allowed");
    assert_eq!(
        allowed("write_spill"),
        Some(true),
        "the spill directory must be writable: {confined:?}"
    );
    assert_eq!(
        allowed("read_ok"),
        Some(true),
        "a user's CSV must be readable: {confined:?}"
    );
    for denied in [
        "write_tmp",
        "write_elsewhere",
        "read_denied_0",
        "read_denied_1",
        "read_denied_2",
        "connect",
        "exec",
    ] {
        assert_eq!(
            allowed(denied),
            Some(false),
            "{denied} must be refused: {confined:?}"
        );
    }
}

#[test]
fn the_profile_names_exactly_the_four_parameters_the_app_passes() {
    let mut params: Vec<&str> = PROFILE
        .match_indices("(param \"")
        .map(|(at, _)| PROFILE[at + 8..].split('"').next().unwrap())
        .collect();
    params.sort_unstable();
    params.dedup();
    assert_eq!(params, ["COMPONENT_DIR", "HELPER", "HOME", "SPILL_DIR"]);
}

#[test]
fn a_profile_without_its_parameters_is_refused_not_run_unconfined() {
    if can_sandbox_at_all().is_err() {
        return;
    }
    let output = Command::new(SANDBOX_EXEC)
        .args(["-p", PROFILE, "/usr/bin/true"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a profile with unset parameters must not run the program"
    );
}

/// The helper as the app runs it: sandboxed, over pipes, asked a question whose answer needs
/// more memory than its lease. It has to read a file (anywhere), spill (to its own
/// directory, the only place it may write), and answer, with nothing left in `TMPDIR` and
/// nothing visible in the spill directory.
#[test]
fn the_sandboxed_helper_serves_a_query_that_spills() {
    use qh_analytics_proto::*;

    if let Err(reason) = can_sandbox_at_all() {
        eprintln!("SKIPPED: this machine cannot apply a sandbox here ({reason}); confinement is not proved");
        return;
    }
    let layout = layout();
    let big = layout.home.join("data/big.csv");
    {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&big).unwrap());
        writeln!(out, "id,name").unwrap();
        for id in (0..400_000u32).rev() {
            writeln!(out, "{id},row-{id:08}-padding-to-make-the-rows-longer").unwrap();
        }
    }
    let tmp = layout.home.join("elsewhere");
    let mut command = Command::new(SANDBOX_EXEC);
    command.args(["-p", PROFILE]);
    for (key, value) in [
        ("HELPER", layout.helper.as_path()),
        ("SPILL_DIR", layout.spill.as_path()),
        ("HOME", layout.home.as_path()),
        ("COMPONENT_DIR", layout.components.as_path()),
    ] {
        command.arg("-D").arg(format!("{key}={}", value.display()));
    }
    let mut child = command
        .arg(&layout.helper)
        .args(["--protocol", "1"])
        .env_clear()
        .env("TMPDIR", &tmp)
        .current_dir(&layout.spill)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to_helper = child.stdin.take().unwrap();
    let mut from_helper = child.stdout.take().unwrap();

    let mut next = || -> Frame {
        read_frame(&mut from_helper)
            .unwrap()
            .expect("the helper closed its output")
    };
    let control = |frame: &Frame| -> HelperMessage { decode_control(&frame.payload).unwrap() };

    let hello = next();
    assert!(matches!(
        control(&hello),
        HelperMessage::Hello { protocol: 1, .. }
    ));
    write_control(&mut to_helper, 1, &AppMessage::OpenSession { session: 1 }).unwrap();
    assert_eq!(control(&next()), HelperMessage::Ack);
    write_control(
        &mut to_helper,
        2,
        &AppMessage::RegisterFile {
            session: 1,
            name: "big".into(),
            path: big.display().to_string(),
            format: FileFormat::Csv {
                has_header: true,
                delimiter: b',',
                quote: b'"',
            },
        },
    )
    .unwrap();
    assert!(matches!(control(&next()), HelperMessage::TableInfo { .. }));

    // 400,000 rows of about 60 bytes sorted in reverse, on a 32 MiB lease: the sort cannot
    // stay in memory.
    write_control(
        &mut to_helper,
        3,
        &AppMessage::RunSql {
            session: 1,
            sql: "SELECT id, name FROM big ORDER BY name".into(),
            lease_bytes: 32 << 20,
            row_cap: 5_000_000,
        },
    )
    .unwrap();
    let mut chunks = 0;
    let done = loop {
        let frame = next();
        match frame.kind {
            Kind::Batch => chunks += 1,
            Kind::Control => {
                if let message @ HelperMessage::Done { .. } = control(&frame) {
                    break message;
                }
            }
        }
    };
    assert_eq!(
        done,
        HelperMessage::Done {
            rows: 400_000,
            truncated: false,
            cancelled: false,
            error: None
        }
    );
    assert!(
        chunks >= 7,
        "400,000 rows come out in chunks of at most 65,536, got {chunks}"
    );

    // Nothing leaked to the disk, and the helper goes when its input does.
    assert_eq!(
        std::fs::read_dir(&layout.spill).unwrap().count(),
        0,
        "the spill directory is not empty"
    );
    assert_eq!(
        std::fs::read_dir(&tmp).unwrap().count(),
        0,
        "TMPDIR was written to"
    );
    drop(to_helper);
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(
        status.success(),
        "the helper exited with {status}: {stderr}"
    );
}
