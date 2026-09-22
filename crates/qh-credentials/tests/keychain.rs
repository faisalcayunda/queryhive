//! The Keychain itself, against the real login keychain.
//!
//! ```bash
//! QH_TEST_KEYCHAIN=1 cargo test -p qh-credentials --test keychain
//! ```
//!
//! Opt-in, unlike every other integration test in this workspace, and the reason is the
//! resource rather than the server: writing to a user's login keychain unasked is not
//! something a test suite does. Without the variable each test prints why it is skipping
//! and returns.
//!
//! What this proves that the unit tests cannot: that the item this crate writes is the
//! item the app's own code wrote — same class, same service, same account spelling — and
//! that "no password saved yet" arrives as `None` rather than as an error. The items it
//! creates are named after the process id and are deleted at the end, including when an
//! assertion fails.
//!
//! # What this deliberately does not do
//!
//! It does not ask a second program to read the item. `/usr/bin/security` reading an
//! item another program created raises the Keychain's own permission dialog, and a test
//! that waits for a human to click it is a test that hangs in CI — which is how that was
//! found. The item's shape is checked against the app's real item instead, by hand and
//! once, with the result recorded in `PROGRESS.md`: the service is the same string and
//! the account is an upper-case UUID, which is what `account_key` produces.

use qh_credentials::{account_key, KeychainStore, SecretStore};
use secrecy::{ExposeSecret, SecretString};

const SKIP_HINT: &str = "skipped: set QH_TEST_KEYCHAIN=1 to exercise the real login keychain";

/// The account this run owns, so two concurrent runs and a crashed run cannot collide.
///
/// Shaped like a UUID because that is what the app uses, and normalised by the crate the
/// same way — so this test also exercises the spelling rule rather than side-stepping it.
fn account() -> String {
    // The last four hex digits of the process id, so the account keeps the 8-4-4-4-12
    // shape whatever the pid is: a longer group would make the string a name rather
    // than a UUID, and this test is partly about the UUID spelling rule.
    let tail = std::process::id() & 0xffff;
    account_key(&format!("7f3d2a10-{tail:04x}-4c8e-9f01-2b7c5d6e8a90"))
}

/// Deletes the item when it goes out of scope, so a failed assertion does not leave a
/// password behind.
struct Cleanup {
    account: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = KeychainStore.delete(&self.account);
    }
}

fn secret(text: &str) -> SecretString {
    SecretString::new(text.to_owned().into_boxed_str())
}

fn reveal(secret: &SecretString) -> String {
    secret.expose_secret().to_owned()
}

#[test]
fn a_secret_survives_the_real_keychain() {
    if std::env::var("QH_TEST_KEYCHAIN").as_deref() != Ok("1") {
        eprintln!("{SKIP_HINT}");
        return;
    }

    let account = account();
    let _cleanup = Cleanup {
        account: account.clone(),
    };
    let store = KeychainStore;

    // Nothing stored is the ordinary state of a connection with no password, and it has
    // to arrive as `None` rather than as a failure — an error here would show a working
    // password-less connection as broken.
    store.delete(&account).expect("delete before the test");
    assert!(
        store.get(&account).expect("get when empty").is_none(),
        "nothing is stored yet"
    );

    store.set(&account, &secret("first")).expect("set");
    assert_eq!(reveal(&store.get(&account).expect("get").unwrap()), "first");

    // Editing a connection replaces its password, so this must not fail with a
    // duplicate-item error.
    store.set(&account, &secret("second")).expect("overwrite");
    assert_eq!(
        reveal(&store.get(&account).expect("get").unwrap()),
        "second"
    );

    // A password with a space and non-ASCII characters: the item holds bytes and the
    // round trip must not transform them.
    //
    // (If this file is ever interrupted mid-test, the item it was holding is named
    // `7F3D2A10-<four hex digits of the test process's pid>-4C8E-9F01-2B7C5D6E8A90`,
    // upper-cased, holding one of these strings — never a real credential, and never the
    // app's own item, whose account is that connection's UUID.
    // `security delete-generic-password -s id.data-ecosystem.queryhive -a <that account>`
    // removes it.)
    store
        .set(&account, &secret("pässword with space"))
        .expect("set");
    assert_eq!(
        reveal(&store.get(&account).expect("get").unwrap()),
        "pässword with space"
    );

    store.delete(&account).expect("delete");
    assert!(store.get(&account).expect("get after delete").is_none());
    // Deleting again is success: the caller asked for it gone, and it is gone.
    store.delete(&account).expect("delete twice");
}

#[test]
fn the_account_is_found_whichever_way_it_is_spelled() {
    if std::env::var("QH_TEST_KEYCHAIN").as_deref() != Ok("1") {
        eprintln!("{SKIP_HINT}");
        return;
    }

    let account = account();
    let _cleanup = Cleanup {
        account: account.clone(),
    };
    let store = KeychainStore;

    // Foundation's `uuidString` is upper case and Rust's is lower: one connection, two
    // spellings. If the crate did not fold them, this would write a second item and a
    // password saved by the app would look missing here.
    store
        .set(&account.to_ascii_lowercase(), &secret("saved lowercase"))
        .expect("set");
    assert_eq!(
        reveal(
            &store
                .get(&account.to_ascii_uppercase())
                .expect("get")
                .unwrap()
        ),
        "saved lowercase"
    );
}
