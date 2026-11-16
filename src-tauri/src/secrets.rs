//! Logins (server passwords, plugin tokens) live in the system's credential
//! store: Keychain on macOS, Credential Manager on Windows, Secret Service on
//! Linux. All of them share one entry, read once per run, so macOS asks for
//! access once instead of once per login. Builds before that kept one entry
//! per login; those move into the shared one the first time they're read.
//!
//! Older builds kept them in library.db as `secret.*` settings, and `migrate`
//! moves those over. Where the credential store doesn't work, such a row is
//! still read so nobody gets signed out, but nothing new is written there.

use std::collections::{HashMap, HashSet};
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

/// The account of the shared entry.
const VAULT: &str = "logins";

#[derive(Default)]
struct Vault {
    logins: HashMap<String, String>,
    /// Keys already looked for as entries of their own this run.
    checked: HashSet<String>,
}

#[cfg(not(test))]
fn with_cache<R>(f: impl FnOnce(&mut Option<Vault>) -> R) -> R {
    static CACHE: std::sync::Mutex<Option<Vault>> = std::sync::Mutex::new(None);
    f(&mut CACHE.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
fn with_cache<R>(f: impl FnOnce(&mut Option<Vault>) -> R) -> R {
    thread_local!(static CACHE: std::cell::RefCell<Option<Vault>> = const { std::cell::RefCell::new(None) });
    CACHE.with(|c| f(&mut c.borrow_mut()))
}

/// Run `f` on the logins, reading the shared entry the first time. Not cached
/// when the store can't be read, so a later call tries again.
fn with_vault<R>(f: impl FnOnce(&mut Vault) -> R) -> Result<R, String> {
    with_cache(|cache| {
        if cache.is_none() {
            let logins = match store::get(service(), VAULT)? {
                Some(json) => serde_json::from_str(&json).map_err(|e| format!("the saved logins are damaged: {e}"))?,
                None => HashMap::new(),
            };
            *cache = Some(Vault { logins, checked: HashSet::new() });
        }
        Ok(f(cache.as_mut().expect("just filled")))
    })
}

fn save(v: &Vault) -> Result<(), String> {
    let json = serde_json::to_string(&v.logins).map_err(|e| e.to_string())?;
    store::set(service(), VAULT, &json)
}

/// A login from an entry of its own (older builds), moved into the shared one.
fn take_old_entry(v: &mut Vault, account: &str) -> Option<String> {
    if !v.checked.insert(account.to_string()) {
        return None;
    }
    let old = store::get(service(), account).ok().flatten()?;
    v.logins.insert(account.to_string(), old.clone());
    if save(v).is_ok() {
        let _ = store::delete(service(), account);
    }
    Some(old)
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
    let found = match with_vault(|v| v.logins.get(account(key)).cloned().or_else(|| take_old_entry(v, account(key)))) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("secrets: reading {key}: {e}");
            None
        }
    };
    found.or_else(|| legacy_row(conn, key)).filter(|v| !v.is_empty())
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    with_vault(|v| {
        v.logins.insert(account(key).to_string(), value.to_string());
        v.checked.insert(account(key).to_string());
        save(v)
    })
    .and_then(|r| r)
    .map_err(|e| format!("Couldn't save the login in the system keychain: {e}"))?;
    remove_legacy_row(conn, key).map_err(|e| e.to_string())
}

pub fn delete(conn: &Connection, key: &str) -> Result<(), String> {
    // Signing out still forgets the database copy if the store can't be reached.
    let stored = with_vault(|v| {
        v.checked.insert(account(key).to_string());
        let _ = store::delete(service(), account(key));
        if v.logins.remove(account(key)).is_some() { save(v) } else { Ok(()) }
    })
    .and_then(|r| r);
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
        let moved = if value.is_empty() {
            Ok(())
        } else {
            with_vault(|v| {
                v.logins.insert(account(&key).to_string(), value.clone());
                save(v)
            })
            .and_then(|r| r)
        };
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
        assert!(store::get(service(), VAULT).unwrap().unwrap().contains("\"plugin.demo.session\":\"sk\""));
    }

    #[test]
    fn old_entries_move_into_the_shared_one() {
        let conn = library();
        store::set(service(), "server.1.password", "pw").unwrap();
        assert_eq!(get(&conn, "secret.server.1.password").as_deref(), Some("pw"));
        assert_eq!(store::get(service(), "server.1.password").unwrap(), None, "the old entry is gone");
        with_cache(|c| *c = None);
        assert_eq!(get(&conn, "secret.server.1.password").as_deref(), Some("pw"), "read back from the shared entry");
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
