//! Login tokens (plugin secrets) live in the system's credential store:
//! Keychain on macOS, Credential Manager on Windows, Secret Service on Linux.
//! They're named by their settings keys (`secret.…`).
//!
//! Older builds kept them in library.db as `secret.*` settings, and `migrate`
//! moves those over. Where the credential store doesn't work, such a row is
//! still read so nobody gets signed out, but nothing new is written there.

use std::sync::OnceLock;

use rusqlite::{Connection, OptionalExtension};

static SERVICE: OnceLock<String> = OnceLock::new();

/// The name credentials are filed under: the app identifier, plus the library
/// folder for test libraries, so those never see or overwrite real logins.
pub fn init(identifier: &str, test_library: Option<&std::path::Path>) {
    let service = match test_library {
        Some(dir) => format!("{identifier} ({})", dir.display()),
        None => identifier.to_string(),
    };
    let _ = SERVICE.set(service);
}

fn service() -> &'static str {
    SERVICE.get().map_or("dev.n1ji.honeycomb", String::as_str)
}

/// The account name inside the credential store: the key without `secret.`.
fn account(key: &str) -> &str {
    key.strip_prefix("secret.").unwrap_or(key)
}

#[cfg(not(test))]
mod store {
    use keyring::{Entry, Error};

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        match Entry::new(service, account).and_then(|e| e.get_password()) {
            Ok(v) => Ok(Some(v)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        Entry::new(service, account).and_then(|e| e.set_password(value)).map_err(|e| e.to_string())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        match Entry::new(service, account).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Tests never touch the real credential store. Each test runs on its own
/// thread, so each gets its own empty store.
#[cfg(test)]
mod store {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static STORE: RefCell<HashMap<(String, String), String>> = RefCell::new(HashMap::new());
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(STORE.with(|s| s.borrow().get(&(service.into(), account.into())).cloned()))
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        STORE.with(|s| s.borrow_mut().insert((service.into(), account.into()), value.into()));
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        STORE.with(|s| s.borrow_mut().remove(&(service.into(), account.into())));
        Ok(())
    }
}

fn legacy_row(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
}

fn remove_legacy_row(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key]).map(|_| ())
}

/// The secret stored under `key`, or None (empty values count as none).
pub fn get(conn: &Connection, key: &str) -> Option<String> {
    let found = match store::get(service(), account(key)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("secrets: reading {key}: {e}");
            None
        }
    };
    found.or_else(|| legacy_row(conn, key)).filter(|v| !v.is_empty())
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    store::set(service(), account(key), value).map_err(|e| format!("Couldn't save the login in the system keychain: {e}"))?;
    remove_legacy_row(conn, key).map_err(|e| e.to_string())
}

pub fn delete(conn: &Connection, key: &str) -> Result<(), String> {
    // Signing out still forgets the database copy if the store can't be reached.
    let stored = store::delete(service(), account(key));
    remove_legacy_row(conn, key).map_err(|e| e.to_string())?;
    stored.map_err(|e| format!("Couldn't remove the login from the system keychain: {e}"))
}

/// Move `secret.*` settings left by older builds into the credential store.
/// A row stays (and is still read) only if the store refused it.
pub fn migrate(conn: &Connection) {
    let rows: Vec<(String, String)> = match conn
        .prepare("SELECT key, value FROM settings WHERE key LIKE 'secret.%'")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
    {
        Ok(rows) => rows,
        Err(e) => return eprintln!("secrets: reading old logins: {e}"),
    };
    for (key, value) in rows {
        let moved = if value.is_empty() { Ok(()) } else { store::set(service(), account(&key), &value) };
        match moved.map_err(|e| e.to_string()).and_then(|()| remove_legacy_row(conn, &key).map_err(|e| e.to_string())) {
            Ok(()) => eprintln!("secrets: moved {key} into the system keychain"),
            Err(e) => eprintln!("secrets: kept {key} in the database: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn old_rows_move_out_of_the_database() {
        let conn = library();
        crate::db::set_setting(&conn, "secret.plugin.demo.session", "sk").unwrap();
        crate::db::set_setting(&conn, "secret.empty", "").unwrap();
        crate::db::set_setting(&conn, "volume", "0.5").unwrap();
        migrate(&conn);
        assert_eq!(legacy_row(&conn, "secret.plugin.demo.session"), None);
        assert_eq!(legacy_row(&conn, "secret.empty"), None);
        assert_eq!(legacy_row(&conn, "volume").as_deref(), Some("0.5"));
        assert_eq!(get(&conn, "secret.plugin.demo.session").as_deref(), Some("sk"));
        assert_eq!(store::get(service(), "plugin.demo.session").unwrap().as_deref(), Some("sk"));
    }

    #[test]
    fn set_get_delete() {
        let conn = library();
        assert_eq!(get(&conn, "secret.plugin.demo.token"), None);
        set(&conn, "secret.plugin.demo.token", "tok").unwrap();
        assert_eq!(get(&conn, "secret.plugin.demo.token").as_deref(), Some("tok"));
        assert_eq!(legacy_row(&conn, "secret.plugin.demo.token"), None);
        delete(&conn, "secret.plugin.demo.token").unwrap();
        assert_eq!(get(&conn, "secret.plugin.demo.token"), None);
    }

    #[test]
    fn a_row_the_store_refused_is_still_read() {
        let conn = library();
        crate::db::set_setting(&conn, "secret.plugin.x.token", "t").unwrap();
        assert_eq!(get(&conn, "secret.plugin.x.token").as_deref(), Some("t"));
        // Signing out removes it either way.
        delete(&conn, "secret.plugin.x.token").unwrap();
        assert_eq!(get(&conn, "secret.plugin.x.token"), None);
    }
}
