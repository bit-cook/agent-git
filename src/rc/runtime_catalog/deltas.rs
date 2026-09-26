//! Scoped journal cursors recover discovery changes without re-reading every native row.

use super::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

const RETAIN: i64 = 100_000;

#[derive(Serialize, Deserialize)]
struct Cursor {
    instance: String,
    scope: String,
    sequence: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Removed {
    pub session_ref: String,
    pub cwd: PathBuf,
}

pub(super) struct Delta {
    pub rows: Vec<CatalogRow>,
    pub removed: Vec<Removed>,
    pub cursor: String,
    pub reset: bool,
    pub more: bool,
}

pub(super) fn initialize(connection: &Connection) -> crate::Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS change_state (
             id INTEGER PRIMARY KEY CHECK(id=1), instance TEXT NOT NULL, floor INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS changes (
             sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_ref TEXT NOT NULL,
             cwd TEXT NOT NULL, row TEXT);
         CREATE TRIGGER IF NOT EXISTS catalog_insert AFTER INSERT ON sessions BEGIN
             INSERT INTO changes(session_ref,cwd,row) VALUES(NEW.session_ref,NEW.cwd,NEW.row);
         END;
         CREATE TRIGGER IF NOT EXISTS catalog_remove AFTER DELETE ON sessions BEGIN
             INSERT INTO changes(session_ref,cwd,row) VALUES(OLD.session_ref,OLD.cwd,NULL);
         END;
         CREATE TRIGGER IF NOT EXISTS catalog_update AFTER UPDATE OF row,cwd ON sessions
             WHEN OLD.row != NEW.row OR OLD.cwd != NEW.cwd BEGIN
             INSERT INTO changes(session_ref,cwd,row)
                 SELECT OLD.session_ref,OLD.cwd,NULL WHERE OLD.cwd != NEW.cwd;
             INSERT INTO changes(session_ref,cwd,row) VALUES(NEW.session_ref,NEW.cwd,NEW.row);
         END;",
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO change_state VALUES(1,?1,0)",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    connection.execute_batch("PRAGMA user_version=2; COMMIT;")?;
    Ok(())
}

pub(super) fn prune(connection: &Connection) -> crate::Result<()> {
    connection.execute(
        "UPDATE change_state SET floor=max(floor,coalesce((SELECT max(sequence) FROM changes),0)-?1) WHERE id=1",
        [RETAIN],
    )?;
    connection.execute(
        "DELETE FROM changes WHERE sequence<=(SELECT floor FROM change_state WHERE id=1)",
        [],
    )?;
    Ok(())
}

fn current(
    connection: &Connection,
    roots: &CanonicalRoots,
    valid: &HashSet<(String, u64)>,
) -> crate::Result<Cursor> {
    let instance =
        connection.query_row("SELECT instance FROM change_state WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let sequence = connection.query_row(
        "SELECT max(floor,coalesce((SELECT max(sequence) FROM changes),0)) FROM change_state WHERE id=1",
        [], |r| r.get(0),
    )?;
    let mut sources = valid.iter().collect::<Vec<_>>();
    sources.sort_unstable();
    let scope = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&**roots, sources))?)
    );
    Ok(Cursor {
        instance,
        scope,
        sequence,
    })
}

pub(super) fn snapshot(
    connection: &Connection,
    roots: &CanonicalRoots,
    valid: &HashSet<(String, u64)>,
) -> crate::Result<String> {
    Ok(serde_json::to_string(&current(connection, roots, valid)?)?)
}

pub(super) fn read(
    connection: &Connection,
    roots: &CanonicalRoots,
    valid: &HashSet<(String, u64)>,
    after: &str,
    limit: usize,
) -> crate::Result<Delta> {
    ensure!(after.len() <= 2048, "catalog change cursor is too large");
    let after: Cursor = serde_json::from_str(after)?;
    let mut cursor = current(connection, roots, valid)?;
    let floor: u64 =
        connection.query_row("SELECT floor FROM change_state WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if after.instance != cursor.instance
        || after.scope != cursor.scope
        || after.sequence < floor
        || after.sequence > cursor.sequence
    {
        return Ok(Delta {
            rows: vec![],
            removed: vec![],
            cursor: serde_json::to_string(&cursor)?,
            reset: true,
            more: false,
        });
    }
    let records = connection.prepare(
        "SELECT sequence,session_ref,cwd,row FROM changes WHERE sequence>?1 ORDER BY sequence LIMIT ?2",
    )?.query_map(params![after.sequence, limit + 1], |r| {
        Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    let more = records.len() > limit;
    let mut changes = BTreeMap::new();
    for (sequence, reference, cwd, serialized) in records.into_iter().take(limit) {
        cursor.sequence = sequence;
        let cwd = PathBuf::from(cwd);
        // Removal is authorized by the indexed project path even when that directory is gone.
        if !roots.iter().any(|root| cwd.starts_with(root)) {
            continue;
        }
        let row = serialized
            .map(|row| serde_json::from_str::<CatalogRow>(&row))
            .transpose()?;
        let row = row.filter(|row| {
            valid.contains(&(row.source_id.clone(), row.source_generation))
                && super::super::policy::is_within(&row.cwd, roots)
        });
        changes.insert(reference, (cwd, row));
    }
    let mut rows = Vec::new();
    let mut removed = Vec::new();
    for (session_ref, (cwd, row)) in changes {
        if let Some(row) = row {
            rows.push(row);
        } else {
            removed.push(Removed { session_ref, cwd });
        }
    }
    Ok(Delta {
        rows,
        removed,
        cursor: serde_json::to_string(&cursor)?,
        reset: false,
        more,
    })
}
