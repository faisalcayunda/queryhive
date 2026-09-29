//! The application's own identity, and the profiles that belong to it.
//!
//! Local first, and small on purpose. Signing in (currently with Google) yields a
//! [`AppAccount`] row: the provider, the provider's stable subject, the email, and when
//! the sign-in and any sign-out happened. A [`ProfileRecord`] is one thing saved for that
//! account -- a connection worth reusing, a saved query, a stored preference -- and
//! `owner_id` is what makes it theirs.
//!
//! # What this module is not
//!
//! It never touches a database connection. `connection` keeps its own `secret_ref` and its
//! own `SSH_*` settings, and no function here is called on the path that opens one. This is
//! the identity the application runs under; whether that identity is ever presented to a
//! database is a separate decision that has not been taken.
//!
//! It is also not access control. The rows live in the same SQLite file as everything
//! else, readable by whoever runs the application on this machine. An owner column decides
//! which profile the interface shows for which account; it does not hide a profile from
//! someone who can open the file.
//!
//! # Why the owner is a row and not a string
//!
//! `owner_id` is a [`SyncId`] pointing at `app_account.id`, so that two spellings of one
//! email cannot become two owners, and so a future account change is a revision like every
//! other change in this database rather than a rewrite of every profile row.

use qh_sync::{Resolution, SyncId, SyncMeta, Version};
use rusqlite::{params, OptionalExtension};

use crate::connections::{to_sql_error, to_sync_id};
use crate::{Storage, StorageError};

/// The identity a profile can belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Google,
    Apple,
    Github,
    Microsoft,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Apple => "apple",
            Self::Github => "github",
            Self::Microsoft => "microsoft",
        }
    }

    pub fn parse(text: &str) -> Result<Self, StorageError> {
        match text {
            "google" => Ok(Self::Google),
            "apple" => Ok(Self::Apple),
            "github" => Ok(Self::Github),
            "microsoft" => Ok(Self::Microsoft),
            other => Err(StorageError::UnknownProvider {
                text: other.to_owned(),
            }),
        }
    }
}

/// The one account this application runs under.
///
/// `provider` and `subject` are `None` for a row that exists but has never been signed in
/// with: a row is written on first launch so that profiles have an owner immediately, and
/// the sign-in fills these in. Both `None` and an absent row mean "nobody has signed in",
/// and the difference is that the row is a stable owner id either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAccount {
    pub meta: SyncMeta,
    pub provider: Option<Provider>,
    /// The provider's own user id, stored as text. Never the email: an email is a claim
    /// that can change hands, and an identity keyed on it would follow the address.
    pub subject: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub signed_in_at: Option<i64>,
    pub signed_out_at: Option<i64>,
}

/// What a profile holds.
///
/// Text in the column rather than a SQL CHECK, so a new kind is a code change and not a
/// migration. Kept to three for now because three is what the plan has a use for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    SavedQuery,
    Connection,
    Preference,
}

impl ProfileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SavedQuery => "saved_query",
            Self::Connection => "connection",
            Self::Preference => "preference",
        }
    }

    pub fn parse(text: &str) -> Result<Self, StorageError> {
        match text {
            "saved_query" => Ok(Self::SavedQuery),
            "connection" => Ok(Self::Connection),
            "preference" => Ok(Self::Preference),
            other => Err(StorageError::UnknownProfileKind {
                text: other.to_owned(),
            }),
        }
    }
}

/// One thing saved for one account.
///
/// `payload_json` is the body and is opaque here: the shape belongs to whichever feature
/// owns that `kind`, and a new field in it is not a schema change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRecord {
    pub meta: SyncMeta,
    pub owner_id: SyncId,
    pub kind: ProfileKind,
    pub name: String,
    pub payload_json: String,
}

/// The id the single account row is pinned to.
///
/// A constant because the table's CHECK pins it: one row, one identity, stated in the
/// schema rather than hoped for in code.
pub fn app_account_id() -> SyncId {
    SyncId::parse("00000000-0000-7000-8000-000000000000").expect("the pinned account id is a UUID")
}

const ACCOUNT_COLUMNS: &str = "id, provider, subject, email, display_name, \
     signed_in_at, signed_out_at, updated_at, deleted_at, version";

const PROFILE_COLUMNS: &str =
    "id, owner_id, kind, name, payload_json, updated_at, deleted_at, version";

impl Storage {
    /// Write the account row, replacing it.
    pub fn save_app_account(&self, account: &AppAccount) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO app_account (id, provider, subject, email, display_name, \
              signed_in_at, signed_out_at, updated_at, deleted_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(id) DO UPDATE SET provider = excluded.provider, \
              subject = excluded.subject, email = excluded.email, \
              display_name = excluded.display_name, signed_in_at = excluded.signed_in_at, \
              signed_out_at = excluded.signed_out_at, updated_at = excluded.updated_at, \
              deleted_at = excluded.deleted_at, version = excluded.version",
            params![
                account.meta.id.as_str(),
                account.provider.map(Provider::as_str),
                account.subject,
                account.email,
                account.display_name,
                account.signed_in_at,
                account.signed_out_at,
                account.meta.updated_at,
                account.meta.deleted_at,
                account.meta.version.get(),
            ],
        )?;
        Ok(())
    }

    /// The account row, tombstone included.
    pub fn app_account(&self) -> Result<Option<AppAccount>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ACCOUNT_COLUMNS} FROM app_account WHERE id = ?1"),
                params![app_account_id().as_str()],
                read_account,
            )
            .optional()?)
    }

    /// The account, creating the empty row if there is none.
    ///
    /// The launch path wants an owner id before anyone has signed in, and an identity that
    /// arrives only at sign-in would mean a profile saved before it belongs to nobody. The
    /// row is written once and never again by this method.
    pub fn app_account_or_create(&self, at: i64) -> Result<AppAccount, StorageError> {
        if let Some(account) = self.app_account()? {
            return Ok(account);
        }
        let account = AppAccount {
            meta: SyncMeta::new(app_account_id(), at),
            provider: None,
            subject: None,
            email: None,
            display_name: None,
            signed_in_at: None,
            signed_out_at: None,
        };
        self.save_app_account(&account)?;
        Ok(account)
    }

    /// Write a profile, replacing the row with that identity.
    pub fn save_profile(&self, record: &ProfileRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO profile (id, owner_id, kind, name, payload_json, \
              updated_at, deleted_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(id) DO UPDATE SET owner_id = excluded.owner_id, \
              kind = excluded.kind, name = excluded.name, \
              payload_json = excluded.payload_json, updated_at = excluded.updated_at, \
              deleted_at = excluded.deleted_at, version = excluded.version",
            params![
                record.meta.id.as_str(),
                record.owner_id.as_str(),
                record.kind.as_str(),
                record.name,
                record.payload_json,
                record.meta.updated_at,
                record.meta.deleted_at,
                record.meta.version.get(),
            ],
        )?;
        Ok(())
    }

    /// One profile by identity, tombstone included.
    pub fn profile(&self, id: &SyncId) -> Result<Option<ProfileRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PROFILE_COLUMNS} FROM profile WHERE id = ?1"),
                params![id.as_str()],
                read_profile,
            )
            .optional()?)
    }

    /// Every living profile belonging to one owner, in name order.
    pub fn profiles(&self, owner: &SyncId) -> Result<Vec<ProfileRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM profile \
             WHERE owner_id = ?1 AND deleted_at IS NULL ORDER BY kind ASC, name ASC"
        ))?;
        let rows = statement.query_map(params![owner.as_str()], read_profile)?;
        let mut profiles = Vec::new();
        for row in rows {
            profiles.push(row?);
        }
        Ok(profiles)
    }

    /// Every profile the table holds, tombstones included.
    ///
    /// What a sync would send, ordered by id so two runs agree.
    pub fn profiles_including_deleted(&self) -> Result<Vec<ProfileRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM profile ORDER BY id ASC"
        ))?;
        let rows = statement.query_map([], read_profile)?;
        let mut profiles = Vec::new();
        for row in rows {
            profiles.push(row?);
        }
        Ok(profiles)
    }

    /// Delete a profile, keeping the row.
    pub fn soft_delete_profile(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(mut record) = self.profile(id)? else {
            return Ok(false);
        };
        if record.meta.is_deleted() {
            return Ok(true);
        }
        record.meta.mark_deleted(at);
        self.save_profile(&record)?;
        Ok(true)
    }

    /// Bring a deleted profile back, as a revision like any other.
    pub fn restore_profile(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(mut record) = self.profile(id)? else {
            return Ok(false);
        };
        if !record.meta.is_deleted() {
            return Ok(true);
        }
        record.meta.undelete(at);
        self.save_profile(&record)?;
        Ok(true)
    }

    /// Merge a profile that arrived from elsewhere.
    pub fn merge_profile(&self, incoming: &ProfileRecord) -> Result<Resolution, StorageError> {
        let resolution = match self.profile(&incoming.meta.id)? {
            Some(ours) => ours.meta.resolve(&incoming.meta),
            None => Resolution::TakeTheirs,
        };
        if resolution == Resolution::TakeTheirs {
            self.save_profile(incoming)?;
        }
        Ok(resolution)
    }
}

impl AppAccount {
    /// A row at its first revision, with nobody signed in.
    pub fn new(at: i64) -> Self {
        Self {
            meta: SyncMeta::new(app_account_id(), at),
            provider: None,
            subject: None,
            email: None,
            display_name: None,
            signed_in_at: None,
            signed_out_at: None,
        }
    }
}

impl ProfileRecord {
    /// A living profile at its first revision.
    pub fn new(
        owner: &SyncId,
        kind: ProfileKind,
        name: impl Into<String>,
        payload_json: impl Into<String>,
        at: i64,
    ) -> Self {
        Self {
            meta: SyncMeta::new(SyncId::now(), at),
            owner_id: owner.clone(),
            kind,
            name: name.into(),
            payload_json: payload_json.into(),
        }
    }
}

/// One row of `app_account` as it stands in the database.
fn read_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppAccount> {
    let id: String = row.get(0)?;
    let provider: Option<String> = row.get(1)?;
    let deleted_at: Option<i64> = row.get(8)?;
    let version: i64 = row.get(9)?;
    Ok(AppAccount {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(7)?,
            deleted_at,
            version: Version::from(version),
        },
        provider: match provider {
            Some(text) => Some(Provider::parse(&text).map_err(|error| to_sql_error(&error))?),
            None => None,
        },
        subject: row.get(2)?,
        email: row.get(3)?,
        display_name: row.get(4)?,
        signed_in_at: row.get(5)?,
        signed_out_at: row.get(6)?,
    })
}

/// One row of `profile` as it stands in the database.
fn read_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileRecord> {
    let id: String = row.get(0)?;
    let owner_id: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let deleted_at: Option<i64> = row.get(6)?;
    let version: i64 = row.get(7)?;
    Ok(ProfileRecord {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(5)?,
            deleted_at,
            version: Version::from(version),
        },
        owner_id: to_sync_id(&owner_id)?,
        kind: ProfileKind::parse(&kind).map_err(|error| to_sql_error(&error))?,
        name: row.get(3)?,
        payload_json: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> Storage {
        let mut storage = Storage::in_memory().expect("in-memory database");
        storage.migrate_at(1_700_000_000_000).expect("migrate");
        storage
    }

    #[test]
    fn the_account_row_is_a_singleton() {
        let storage = storage();
        let account = storage
            .app_account_or_create(1_700_000_000_000)
            .expect("create");
        assert_eq!(account.meta.id, app_account_id());
        assert_eq!(account.provider, None);

        // A second row is refused by the table's CHECK rather than by this code, which is
        // the point of pinning the id in the schema.
        let second = AppAccount {
            meta: SyncMeta::new(SyncId::now(), 1_700_000_000_000),
            provider: Some(Provider::Google),
            subject: Some("42".to_owned()),
            email: None,
            display_name: None,
            signed_in_at: Some(1_700_000_000_000),
            signed_out_at: None,
        };
        assert!(storage.save_app_account(&second).is_err());
    }

    #[test]
    fn signing_in_fills_the_row_and_signing_out_keeps_it() {
        let storage = storage();
        let mut account = storage
            .app_account_or_create(1_700_000_000_000)
            .expect("create");
        account.provider = Some(Provider::Google);
        account.subject = Some("10769150350006150715113082367".to_owned());
        account.email = Some("faisal@example.com".to_owned());
        account.display_name = Some("Faisal".to_owned());
        account.signed_in_at = Some(1_700_000_000_000);
        storage.save_app_account(&account).expect("save");

        let mut signed_in = storage.app_account().expect("read").expect("a row");
        assert_eq!(signed_in.provider, Some(Provider::Google));
        assert_eq!(signed_in.signed_in_at, Some(1_700_000_000_000));
        assert_eq!(signed_in.signed_out_at, None);

        // Signing out is a fact about the row, not the absence of one: the subject and the
        // email stay, and the reader can still say who used this application.
        signed_in.signed_out_at = Some(1_700_000_001_000);
        signed_in.meta.touch(1_700_000_001_000);
        storage.save_app_account(&signed_in).expect("save");

        let after = storage.app_account().expect("read").expect("a row");
        assert_eq!(after.subject, signed_in.subject);
        assert_eq!(after.signed_out_at, Some(1_700_000_001_000));
    }

    #[test]
    fn a_profile_belongs_to_its_owner_and_nobody_else() {
        let storage = storage();
        let owner = storage
            .app_account_or_create(1_700_000_000_000)
            .expect("create");
        let mine = ProfileRecord::new(
            &owner.meta.id,
            ProfileKind::Connection,
            "staging",
            "{\"host\":\"db.staging\"}",
            1_700_000_000_000,
        );
        storage.save_profile(&mine).expect("save");

        let theirs = SyncId::now();
        assert!(storage.profiles(&theirs).expect("read").is_empty());
        assert_eq!(
            storage.profiles(&owner.meta.id).expect("read"),
            vec![mine.clone()]
        );

        // The row itself is still readable by identity: the owner column decides what a
        // listing shows, not what the database holds.
        assert_eq!(storage.profile(&mine.meta.id).expect("read"), Some(mine));
    }

    #[test]
    fn deleting_a_profile_keeps_the_row_and_the_listing_drops_it() {
        let storage = storage();
        let owner = storage
            .app_account_or_create(1_700_000_000_000)
            .expect("create");
        let record = ProfileRecord::new(
            &owner.meta.id,
            ProfileKind::SavedQuery,
            "monthly",
            "{}",
            1_700_000_000_000,
        );
        storage.save_profile(&record).expect("save");
        assert_eq!(storage.profiles(&owner.meta.id).expect("read").len(), 1);

        assert!(storage
            .soft_delete_profile(&record.meta.id, 1_700_000_001_000)
            .expect("delete"));
        assert!(storage.profiles(&owner.meta.id).expect("read").is_empty());

        // The tombstone is still there, and still carries its owner: a future sync sends
        // the deletion, and a purge has something to walk.
        let tombstone = storage
            .profile(&record.meta.id)
            .expect("read")
            .expect("the row survives");
        assert!(tombstone.meta.is_deleted());
        assert_eq!(tombstone.owner_id, record.owner_id);
        assert_eq!(
            storage.profiles_including_deleted().expect("all"),
            vec![tombstone]
        );

        // Deleting a tombstone again is a no-op rather than an error.
        assert!(storage
            .soft_delete_profile(&record.meta.id, 1_700_000_002_000)
            .expect("delete again"));

        assert!(storage
            .restore_profile(&record.meta.id, 1_700_000_003_000)
            .expect("restore"));
        assert_eq!(storage.profiles(&owner.meta.id).expect("read").len(), 1);
    }
}
