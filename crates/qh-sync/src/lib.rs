//! What a stored row has to look like to be syncable later — and nothing that syncs.
//!
//! The cloud-sync feature is not being built (§5 point 7 of the blueprint). What is being
//! built is the *shape*: a row with a stable identity, a revision, and a way to say
//! "deleted" that survives the deletion, so that adding a transport later needs no schema
//! migration and no archaeology. The conflict table in §7.2 settles the tension the other
//! way round: these types are real and `qh-storage` uses them, rather than this crate
//! being a folder of empty functions waiting for a feature.
//!
//! Three rules, and each one is a decision rather than a convention:
//!
//! 1. **Identity is a UUID, kept as text.** Lower case, so one id has one spelling; the
//!    database indexes it as text and a future transport can put it on the wire as is.
//!    (`qh-credentials` folds its accounts to *upper* case instead, and that is not an
//!    inconsistency: the ids here are written only by this engine, while those Keychain
//!    accounts were already written by the app.)
//! 2. **A revision is a number that only goes up.** [`Version`] starts at 1 and each
//!    change takes the next one. `updated_at` is for people to read; the version is what
//!    decides who wins, because two machines' clocks disagree and two machines' counters
//!    do not.
//! 3. **A tombstone is a deletion that is still a row.** Setting `deleted_at` keeps the
//!    identity, the version and the timestamp, so "delete on one machine, edit on
//!    another" resolves the same way any other conflict does.

use uuid::Uuid;

/// Why an identity could not be accepted.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SyncError {
    #[error("{0:?} is not a UUID")]
    NotAUuid(String),
}

/// A row's identity: a UUID, spelled in one way and only one.
///
/// Case is folded rather than rejected, so that an id pasted from anywhere — a log, a
/// JSON file written by the Python engine, a Foundation `uuidString` — names the same row
/// as the one this engine wrote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncId(String);

impl SyncId {
    /// A new identity, time-ordered.
    ///
    /// UUIDv7 rather than v4 because the first 48 bits are a millisecond clock: new rows
    /// sort near the end of an index instead of scattering through it, which is the
    /// difference between an insert and a page fault on a table with history in it.
    pub fn now() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    /// An identity that already exists somewhere: a database row, a JSON file, a wire
    /// message.
    pub fn parse(text: &str) -> Result<Self, SyncError> {
        match Uuid::parse_str(text.trim()) {
            Ok(uuid) => Ok(Self(uuid.to_string())),
            Err(_) => Err(SyncError::NotAUuid(text.to_owned())),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SyncId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A row's revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(i64);

impl Version {
    /// The revision a row is born with.
    pub const FIRST: Self = Self(1);

    pub fn get(self) -> i64 {
        self.0
    }

    /// The next revision.
    ///
    /// Saturating rather than wrapping: a counter that wraps after nine quintillion edits
    /// would make an ancient row win every conflict, and reaching that number is not a
    /// case worth an error.
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl From<i64> for Version {
    fn from(value: i64) -> Self {
        Self(value.max(1))
    }
}

/// Which of two revisions of one row to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The other side is newer — including when "newer" means a deletion.
    TakeTheirs,
    /// This side is newer, or the two are indistinguishable.
    KeepOurs,
}

/// Everything about a row that outlives the row's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncMeta {
    pub id: SyncId,
    /// Unix milliseconds. When the change happened, as the machine that made it saw it.
    pub updated_at: i64,
    /// `None` while the row is alive; the moment it was deleted once it is not.
    pub deleted_at: Option<i64>,
    pub version: Version,
}

impl SyncMeta {
    /// A new, living row at its first revision.
    pub fn new(id: SyncId, at: i64) -> Self {
        Self {
            id,
            updated_at: at,
            deleted_at: None,
            version: Version::FIRST,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Record a change to a living row.
    pub fn touch(&mut self, at: i64) {
        self.updated_at = at;
        self.version = self.version.next();
    }

    /// Delete the row without losing it.
    ///
    /// The row keeps its identity and its history; only `deleted_at` is filled in, so a
    /// machine that was offline when the deletion happened can be told about it in the
    /// same terms as any other change.
    pub fn mark_deleted(&mut self, at: i64) {
        self.deleted_at = Some(at);
        self.touch(at);
    }

    /// Bring a row back, as a change that has to travel like any other.
    pub fn undelete(&mut self, at: i64) {
        self.deleted_at = None;
        self.touch(at);
    }

    /// Decide between this revision and one that arrived from somewhere else.
    ///
    /// The rules, in order:
    ///
    /// 1. A different identity is not a conflict and is not merged: this answers
    ///    [`Resolution::KeepOurs`], because overwriting a row with another row's contents
    ///    is the one outcome nobody can recover from.
    /// 2. The higher [`Version`] wins. It is a counter, so it does not care whose clock
    ///    said what.
    /// 3. On the same version, the later `updated_at` wins. That is the case where a
    ///    change was replayed with its revision number intact, and the timestamp is the
    ///    only evidence left.
    /// 4. Still tied, this side is kept: the two revisions carry the same version and the
    ///    same timestamp, so they claim to be the same edit, and re-reading a row that is
    ///    already stored writes nothing.
    pub fn resolve(&self, incoming: &SyncMeta) -> Resolution {
        if self.id != incoming.id {
            return Resolution::KeepOurs;
        }
        if incoming.version > self.version {
            return Resolution::TakeTheirs;
        }
        if incoming.version < self.version {
            return Resolution::KeepOurs;
        }
        if incoming.updated_at > self.updated_at {
            return Resolution::TakeTheirs;
        }
        Resolution::KeepOurs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(version: i64, updated_at: i64) -> SyncMeta {
        SyncMeta {
            id: SyncId::parse("01924f1e-2f4a-7c3b-9c1e-7b0f3a5d6e80").unwrap(),
            updated_at,
            deleted_at: None,
            version: Version::from(version),
        }
    }

    #[test]
    fn a_new_id_is_time_ordered_lower_case_text() {
        let first = SyncId::now();
        let second = SyncId::now();
        assert!(
            second > first,
            "a later id sorts later: {first} then {second}"
        );
        assert_eq!(first.as_str(), first.as_str().to_lowercase());
        assert!(Uuid::parse_str(first.as_str()).is_ok(), "{first}");
    }

    #[test]
    fn an_id_keeps_one_spelling_whichever_case_it_arrives_in() {
        // The app writes upper case and the Python engine wrote lower case; both name
        // the same row.
        let upper = SyncId::parse("01924F1E-2F4A-7C3B-9C1E-7B0F3A5D6E80").unwrap();
        let lower = SyncId::parse("01924f1e-2f4a-7c3b-9c1e-7b0f3a5d6e80").unwrap();
        assert_eq!(upper, lower);
        assert_eq!(upper.as_str(), "01924f1e-2f4a-7c3b-9c1e-7b0f3a5d6e80");
        // Whitespace from a text field is not part of the identity.
        assert_eq!(
            lower,
            SyncId::parse("  01924f1e-2f4a-7c3b-9c1e-7b0f3a5d6e80\n").unwrap()
        );
    }

    #[test]
    fn something_that_is_not_a_uuid_is_refused_rather_than_guessed() {
        assert_eq!(
            SyncId::parse("connection-1"),
            Err(SyncError::NotAUuid("connection-1".to_owned()))
        );
        assert!(SyncId::parse("").is_err());
        assert!(SyncId::parse("01924f1e-2f4a-7c3b-9c1e-7b0f3a5d6e8").is_err());
    }

    #[test]
    fn a_revision_only_goes_up() {
        let mut row = SyncMeta::new(SyncId::now(), 1_000);
        assert_eq!(row.version, Version::FIRST);
        assert!(!row.is_deleted());
        row.touch(2_000);
        assert_eq!(row.version.get(), 2);
        assert_eq!(row.updated_at, 2_000);
        // A version read off a wire is clamped rather than trusted: 0 and -5 are not
        // revisions a row can be at.
        assert_eq!(Version::from(0).get(), 1);
        assert_eq!(Version::from(-5).get(), 1);
    }

    #[test]
    fn deleting_is_a_change_that_can_travel() {
        let mut row = SyncMeta::new(SyncId::now(), 1_000);
        row.mark_deleted(5_000);
        assert!(row.is_deleted());
        assert_eq!(row.deleted_at, Some(5_000));
        assert_eq!(row.version.get(), 2, "the deletion is a revision");

        // A row deleted on one machine beats an edit made earlier on another.
        let mut edited_elsewhere = SyncMeta::new(row.id.clone(), 3_000);
        edited_elsewhere.touch(4_000);
        assert_eq!(row.resolve(&edited_elsewhere), Resolution::KeepOurs);
        assert_eq!(edited_elsewhere.resolve(&row), Resolution::TakeTheirs);

        // And it can come back, still as a revision.
        row.undelete(6_000);
        assert!(!row.is_deleted());
        assert_eq!(row.version.get(), 3);
    }

    #[test]
    fn the_higher_revision_wins_regardless_of_the_clocks() {
        // Two machines, two clocks that disagree: the counter decides, which is the
        // whole reason it exists.
        let ours = meta(3, 9_000);
        let theirs = meta(4, 1_000);
        assert_eq!(ours.resolve(&theirs), Resolution::TakeTheirs);
        assert_eq!(theirs.resolve(&ours), Resolution::KeepOurs);
    }

    #[test]
    fn the_same_revision_is_settled_by_the_timestamp() {
        let ours = meta(3, 1_000);
        let theirs = meta(3, 2_000);
        assert_eq!(ours.resolve(&theirs), Resolution::TakeTheirs);
        assert_eq!(theirs.resolve(&ours), Resolution::KeepOurs);
        // Same revision and same timestamp: the same edit, so re-reading it changes
        // nothing.
        assert_eq!(ours.resolve(&ours.clone()), Resolution::KeepOurs);
    }

    #[test]
    fn another_rows_payload_is_never_written_over_this_one() {
        // A sync payload that arrives with the wrong identity is a bug somewhere, and the
        // one answer worth having is "do not write".
        let ours = meta(1, 1_000);
        let mut theirs = meta(99, 99_000);
        theirs.id = SyncId::now();
        assert_eq!(ours.resolve(&theirs), Resolution::KeepOurs);
    }
}
