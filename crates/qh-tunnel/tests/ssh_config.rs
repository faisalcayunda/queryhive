//! The `~/.ssh/config` resolver against OpenSSH itself.
//!
//! The unit tests in `src/ssh_config.rs` pin the rules we wrote down. They cannot say the
//! rules are the ones `ssh` follows, because the same person wrote both. `ssh -G` prints
//! what OpenSSH resolved for a host without connecting, so this file asks the real thing
//! and compares: if "first value wins", `!` negation, `Include` order or `%h` ever drift
//! from OpenSSH, a config that connects somewhere else in `ssh` does so here too, and
//! this is where it shows.
//!
//! It also checks the other direction of the same promise: every directive we refuse by
//! name is one `ssh -G` reports as changing its answer, so the refusal is never fussiness.
//!
//! Needs `ssh` on `PATH` (macOS and every Linux have it) and skips loudly without it, the
//! way the `sshd` tests do. `-F <file>` makes `ssh` ignore `/etc/ssh/ssh_config`, so
//! the result does not depend on the machine's own configuration. Paths in the corpus are
//! absolute because `ssh` anchors `~` and relative `Include`s on the passwd home
//! directory, not on `$HOME`; `Include` against a home of our own is tested at the end
//! through [`ssh_config::load_in`].

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use qh_tunnel::ssh_config::{self, SshConfigError};

/// What `ssh -G` printed, as `(key, value)` in order. `None` when `ssh` cannot be run.
fn ssh_g(config: &Path, alias: &str) -> Option<Vec<(String, String)>> {
    let output = match Command::new("ssh")
        .args(["-G", "-F"])
        .arg(config)
        .arg(alias)
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: ssh is not on PATH");
            return None;
        }
        Err(error) => panic!("cannot run ssh: {error}"),
    };
    assert!(
        output.status.success(),
        "ssh -G refused the config for {alias}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
    )
}

fn values<'a>(seen: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    seen.iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect()
}

/// A temp directory holding a main config and the files it includes, with `{dir}` in
/// every text replaced by the directory's path.
struct Corpus {
    dir: tempfile::TempDir,
}

impl Corpus {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().display().to_string();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text.replace("{dir}", &root)).expect("write");
        }
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// `(name, files, aliases)`: `config` is the main file. Every alias is named by a `Host`
/// line (a lone `*` does not name one, on purpose), and lower case, because `ssh`
/// lower-cases the host it prints.
type Case = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

const CASES: &[Case] = &[
    (
        "first value wins, identity files accumulate, later blocks only fill gaps",
        &[(
            "config",
            "\
Host a b
  HostName a.example
  User alice
  IdentityFile /keys/one
Host !b *
  User star
  Port 2200
  IdentityFile /keys/two
Host *
  Port 9
  User fallback
",
        )],
        &["a", "b"],
    ),
    (
        "keywords are case-insensitive, = and quotes are the same directive",
        &[(
            "config",
            "\
host quoted
  HOSTNAME=\"q.example\"
  user = \"bob\"
  PORT=2022
  identityfile \"/keys/with space\"
",
        )],
        &["quoted"],
    ),
    (
        "cosmetics before and after the block change nothing we resolve",
        &[(
            "config",
            "\
Host *
  ServerAliveInterval 30
  AddKeysToAgent yes
  UseKeychain yes
  Compression yes
  ForwardAgent no
  ConnectTimeout 5
  StrictHostKeyChecking ask
  IdentitiesOnly no
Host db
  HostName db.corp
  User dba
  LogLevel ERROR
",
        )],
        &["db"],
    ),
    (
        "%h in HostName and IdentityFile is the name that was typed",
        &[(
            "config",
            "\
Host h1 h2
  HostName %h.internal
  IdentityFile /keys/%h
  User u
",
        )],
        &["h1", "h2"],
    ),
    (
        "wildcards and ? choose blocks, ! removes them",
        &[(
            "config",
            "\
Host db-? !db-9
  User dba
  Port 5432
Host db-9 web
  User ops
Host web*
  Port 8022
Host db-*
  HostName shared.corp
",
        )],
        &["db-1", "db-9", "web"],
    ),
    (
        "an Include at the top is read where it stands, and its Host lines work",
        &[
            (
                "config",
                "\
Include {dir}/extra.conf
Host last
  User three
",
            ),
            (
                "extra.conf",
                "\
Host included
  User two
  HostName inc.example
Host *
  IdentityFile /keys/from-include
",
            ),
        ],
        &["included", "last"],
    ),
    (
        "an Include inside a Host block is inert for every other alias",
        &[
            (
                "config",
                "\
Host first
  User one
  Include {dir}/extra.conf
  Port 7000
Host last
  User three
",
            ),
            (
                "extra.conf",
                "\
Host included
  User two
Host *
  IdentityFile /keys/from-include
",
            ),
        ],
        &["first", "last"],
    ),
    (
        "lines after an Include are read after the included file's Host blocks",
        &[
            (
                "config",
                "Host a\n  Include {dir}/extra.conf\n  User later\n  Port 1\nHost *\n  Port 2\n",
            ),
            (
                "extra.conf",
                "Host *\n  User from-file\n  Port 3\nHost z\n  Port 4\n",
            ),
        ],
        &["a"],
    ),
    (
        "Include globs are sorted and the first value across files wins",
        &[
            ("config", "Include {dir}/d-*.conf\nHost g\n  User late\n"),
            ("d-b.conf", "Host g\n  User second\n  Port 22022\n"),
            ("d-a.conf", "Host g\n  User first\n  HostName g.example\n"),
        ],
        &["g"],
    ),
];

/// The identity files `ssh` adds when a config names none, which a comparison must not
/// count as the config's.
fn default_identities() -> Vec<String> {
    let blank = Corpus::new(&[("config", "")]);
    ssh_g(&blank.path("config"), "probe")
        .map(|seen| {
            values(&seen, "identityfile")
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn local_user() -> String {
    let output = Command::new("id").arg("-un").output().expect("run id -un");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn resolution_agrees_with_openssh() {
    if ssh_g(&Corpus::new(&[("config", "")]).path("config"), "probe").is_none() {
        return;
    }
    let defaults = default_identities();
    let me = local_user();
    let mut disagreements = Vec::new();

    for (name, files, aliases) in CASES {
        let corpus = Corpus::new(files);
        let config = corpus.path("config");
        let loaded = ssh_config::load_in(&config, None).expect("the corpus loads");
        for alias in *aliases {
            let seen = ssh_g(&config, alias).expect("ssh was found a moment ago");
            let ours = match loaded.resolve(alias) {
                Ok(resolved) => resolved,
                Err(error) => {
                    disagreements.push(format!("{name} / {alias}: we refused: {error}"));
                    continue;
                }
            };
            let one = |key: &str| values(&seen, key).first().map(|v| (*v).to_owned());
            let mut check = |what: &str, ours: String, theirs: Option<String>| {
                if theirs.as_deref() != Some(ours.as_str()) {
                    disagreements.push(format!(
                        "{name} / {alias}: {what}: ours {ours:?}, ssh {theirs:?}"
                    ));
                }
            };
            check("hostname", ours.host_name.clone(), one("hostname"));
            check(
                "port",
                ours.port.map_or_else(|| "22".to_owned(), |p| p.to_string()),
                one("port"),
            );
            check(
                "user",
                ours.user.clone().unwrap_or_else(|| me.clone()),
                one("user"),
            );
            // `-G` prints IdentityFile before its tokens are expanded. At connect time `ssh`
            // replaces %h with the resolved host name (`ssh -vvv` shows `identity file
            // /keys/127.0.0.1` for `IdentityFile /keys/%h` and `HostName 127.0.0.1`), so
            // do the same with the host name `-G` itself reports.
            let host = one("hostname").unwrap_or_default();
            let theirs: Vec<String> = values(&seen, "identityfile")
                .into_iter()
                .filter(|path| !defaults.iter().any(|d| d == path))
                .map(|path| path.replace("%h", &host))
                .collect();
            let ours_files: Vec<String> = ours
                .identity_files
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            if ours_files != theirs {
                disagreements.push(format!(
                    "{name} / {alias}: identity files: ours {ours_files:?}, ssh {theirs:?}"
                ));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "we resolve differently from OpenSSH:\n{}",
        disagreements.join("\n")
    );
}

/// `(directive line, key `ssh -G` prints when it honours it, a marker inside the value)`.
const REFUSED: &[(&str, &str, &str)] = &[
    ("ProxyJump jump-marker-1", "proxyjump", "jump-marker-1"),
    ("ProxyCommand sh -c marker-2", "proxycommand", "marker-2"),
    (
        "HostKeyAlias alias-marker-3",
        "hostkeyalias",
        "alias-marker-3",
    ),
    (
        "UserKnownHostsFile /marker-4/known_hosts",
        "userknownhostsfile",
        "/marker-4/known_hosts",
    ),
    ("StrictHostKeyChecking no", "stricthostkeychecking", "false"),
    (
        "StrictHostKeyChecking accept-new",
        "stricthostkeychecking",
        "accept-new",
    ),
    ("IdentitiesOnly yes", "identitiesonly", "yes"),
    (
        "PreferredAuthentications password",
        "preferredauthentications",
        "password",
    ),
    (
        "IdentityAgent /marker-5/agent.sock",
        "identityagent",
        "/marker-5/agent.sock",
    ),
    (
        "CertificateFile /marker-6/cert.pub",
        "certificatefile",
        "/marker-6/cert.pub",
    ),
    (
        "LocalForward 15432 marker-7:5432",
        "localforward",
        "marker-7",
    ),
    ("RemoteCommand marker-8", "remotecommand", "marker-8"),
    ("VerifyHostKeyDNS yes", "verifyhostkeydns", "true"),
];

#[test]
fn every_directive_we_refuse_is_one_openssh_acts_on_and_the_message_never_repeats_its_value() {
    let have_ssh = ssh_g(&Corpus::new(&[("config", "")]).path("config"), "probe").is_some();
    for (line, key, marker) in REFUSED {
        let corpus = Corpus::new(&[(
            "config",
            &format!("Host bastion\n  HostName bastion.corp\n  {line}\n"),
        )]);
        let config = corpus.path("config");

        let error = ssh_config::load_in(&config, None)
            .expect("loads")
            .resolve("bastion")
            .expect_err(line);
        let directive = line.split_whitespace().next().expect("a directive");
        match &error {
            SshConfigError::Unsupported {
                directive: named,
                line: at,
                ..
            } => {
                assert!(named.eq_ignore_ascii_case(directive), "{line}: {error}");
                assert_eq!(*at, 3, "{line}: the line of the directive");
            }
            other => panic!("{line}: expected a refusal by name, got {other}"),
        }
        let message = error.to_string();
        assert!(message.contains(directive), "{message}");
        assert!(!message.contains(marker), "the value leaked: {message}");

        if have_ssh {
            let seen = ssh_g(&config, "bastion").expect("ssh was found a moment ago");
            assert!(
                values(&seen, key).iter().any(|value| value.contains(marker)),
                "{line}: ssh -G does not report {key} {marker}, so refusing it is not justified: {seen:?}"
            );
        }
    }
}

#[test]
fn match_in_any_included_file_refuses_every_alias() {
    let corpus = Corpus::new(&[
        ("config", "Include {dir}/work.conf\nHost plain\n  User u\n"),
        ("work.conf", "Match host *.corp\n  ProxyJump gate\n"),
    ]);
    let loaded = ssh_config::load_in(&corpus.path("config"), None).expect("loads");
    assert_eq!(loaded.aliases(), ["plain"], "listing never fails on Match");
    assert!(matches!(
        loaded.resolve("plain"),
        Err(SshConfigError::Unsupported { directive, .. }) if directive == "Match"
    ));
}

/// The shape a Docker Desktop, OrbStack or Colima user has: the tool adds an `Include`
/// of its own file to `~/.ssh/config`, and the aliases live in that file.
#[test]
fn a_config_that_includes_a_tool_file_from_the_home_directory_resolves() {
    let home = tempfile::tempdir().expect("temp dir");
    let write = |relative: &str, text: &str| {
        let path = home.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
        path
    };
    write(
        ".orbstack/ssh/config",
        "Host orb\n  HostName 127.0.0.1\n  Port 32222\n  User me\n  IdentityFile ~/.orbstack/ssh/id_ed25519\n",
    );
    write(
        ".ssh/colima.conf",
        "Host colima\n  HostName 127.0.0.1\n  Port 41122\n",
    );
    let config = write(
        ".ssh/config",
        "Include ~/.orbstack/ssh/config\nInclude colima.conf\nHost *\n  ServerAliveInterval 60\n",
    );

    let loaded = ssh_config::load_in(&config, Some(home.path())).expect("loads");
    assert_eq!(loaded.aliases(), ["orb", "colima"]);
    let orb = loaded.resolve("orb").expect("orb");
    assert_eq!(orb.port, Some(32222));
    assert_eq!(
        orb.identity_files,
        [home.path().join(".orbstack/ssh/id_ed25519")]
    );
    assert_eq!(loaded.resolve("colima").expect("colima").port, Some(41122));
}
