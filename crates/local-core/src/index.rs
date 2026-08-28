use std::{collections::HashMap, path::Path};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CoreError, ParsedMarkdown, Result, WorkspaceObject, markdown::revision};

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO schema_migrations(version) VALUES(1);
CREATE TABLE IF NOT EXISTS files(
  id INTEGER PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
  size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, hash TEXT NOT NULL,
  parse_status TEXT NOT NULL, parse_error TEXT, indexed_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS objects(
  file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
  stable_id TEXT NOT NULL, object_type TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL,
  frontmatter_json TEXT NOT NULL, created TEXT, updated TEXT, identity_status TEXT NOT NULL DEFAULT 'unique'
);
CREATE INDEX IF NOT EXISTS objects_stable_id ON objects(stable_id);
CREATE INDEX IF NOT EXISTS objects_type ON objects(object_type);
CREATE TABLE IF NOT EXISTS object_properties(
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  property_name TEXT NOT NULL, value_text TEXT, value_type TEXT NOT NULL,
  PRIMARY KEY(file_id, property_name)
);
CREATE INDEX IF NOT EXISTS object_properties_lookup ON object_properties(property_name, value_text);
CREATE TABLE IF NOT EXISTS object_links(
  source_file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  relation TEXT NOT NULL, target_stable_id TEXT NOT NULL,
  PRIMARY KEY(source_file_id, relation, target_stable_id)
);
CREATE TABLE IF NOT EXISTS plugin_state(plugin_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT NOT NULL, PRIMARY KEY(plugin_id,key));
CREATE TABLE IF NOT EXISTS reconciliation_runs(id INTEGER PRIMARY KEY, started_at TEXT NOT NULL, finished_at TEXT, changed_files INTEGER NOT NULL DEFAULT 0, diagnostic TEXT);
CREATE VIRTUAL TABLE IF NOT EXISTS object_fts USING fts5(stable_id UNINDEXED, relative_path, filename, title, body, metadata, tokenize='unicode61 remove_diacritics 2');
"#;

#[derive(Debug)]
pub struct IndexStore {
    connection: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SearchInput {
    pub query: String,
    #[serde(rename = "type")]
    pub object_type: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub object_id: Option<String>,
    pub object_type: Option<String>,
    pub relative_path: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEntry {
    pub source_id: String,
    pub source_type: String,
    pub title: String,
    pub property: String,
    pub start: String,
    pub end: Option<String>,
    pub all_day: bool,
    pub revision: String,
}

impl IndexStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, "index_open", path.to_str()))?;
        }
        let connection =
            Connection::open(path).map_err(|error| CoreError::index(error, "index_open"))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|error| CoreError::index(error, "index_migrate"))?;
        Ok(Self { connection })
    }

    pub fn in_memory() -> Result<Self> {
        let connection =
            Connection::open_in_memory().map_err(|error| CoreError::index(error, "index_open"))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|error| CoreError::index(error, "index_migrate"))?;
        Ok(Self { connection })
    }

    pub fn clear(&mut self) -> Result<()> {
        self.connection.execute_batch("DELETE FROM object_fts; DELETE FROM object_links; DELETE FROM object_properties; DELETE FROM objects; DELETE FROM files;")
            .map_err(|error| CoreError::index(error, "index_clear"))
    }

    pub fn upsert_markdown(
        &mut self,
        relative_path: &str,
        bytes: &[u8],
        mtime_ns: i64,
        parsed: &ParsedMarkdown,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| CoreError::index(error, "index_upsert"))?;
        upsert_markdown_tx(&tx, relative_path, bytes, mtime_ns, parsed)?;
        update_identity_status(&tx).map_err(|error| CoreError::index(error, "index_identity"))?;
        tx.commit()
            .map_err(|error| CoreError::index(error, "index_upsert"))
    }

    pub fn file_metadata(&self) -> Result<HashMap<String, (i64, i64)>> {
        let mut statement = self
            .connection
            .prepare("SELECT relative_path,size,mtime_ns FROM files")
            .map_err(|error| CoreError::index(error, "index_metadata"))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
            .map_err(|error| CoreError::index(error, "index_metadata"))?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(|error| CoreError::index(error, "index_metadata"))
    }

    pub fn reconcile_markdown(
        &mut self,
        changed: &[(String, Vec<u8>, i64, ParsedMarkdown)],
        removed: &[String],
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| CoreError::index(error, "index_reconcile"))?;
        for relative_path in removed {
            let file_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM files WHERE relative_path=?1",
                    [relative_path],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| CoreError::index(error, "index_reconcile"))?;
            if let Some(file_id) = file_id {
                tx.execute("DELETE FROM object_fts WHERE rowid=?1", [file_id])
                    .map_err(|error| CoreError::index(error, "index_reconcile"))?;
                tx.execute("DELETE FROM files WHERE id=?1", [file_id])
                    .map_err(|error| CoreError::index(error, "index_reconcile"))?;
            }
        }
        for (relative_path, bytes, mtime_ns, parsed) in changed {
            upsert_markdown_tx(&tx, relative_path, bytes, *mtime_ns, parsed)?;
        }
        update_identity_status(&tx).map_err(|error| CoreError::index(error, "index_identity"))?;
        tx.commit()
            .map_err(|error| CoreError::index(error, "index_reconcile"))
    }

    pub fn replace_markdown(
        &mut self,
        files: &[(String, Vec<u8>, i64, ParsedMarkdown)],
    ) -> Result<()> {
        let existing = self.file_metadata()?.into_keys().collect::<Vec<_>>();
        self.reconcile_markdown(files, &existing)
    }

    pub fn remove_path(&mut self, relative_path: &str) -> Result<()> {
        self.reconcile_markdown(&[], &[relative_path.to_owned()])
    }

    pub fn get_object(&self, id: &str) -> Result<Option<WorkspaceObject>> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM objects WHERE stable_id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|error| CoreError::index(error, "object_get"))?;
        if count > 1 {
            return Err(CoreError::new(
                "identity_conflict",
                crate::ErrorCategory::Identity,
                "More than one file uses this stable ID",
                "object_get",
            ));
        }
        self.connection.query_row("SELECT o.stable_id,o.object_type,o.title,o.body,f.relative_path,f.hash,o.created,o.updated,o.frontmatter_json FROM objects o JOIN files f ON f.id=o.file_id WHERE o.stable_id=?1", [id], row_to_object).optional().map_err(|error| CoreError::index(error,"object_get"))
    }

    pub fn query_objects(&self, object_type: Option<&str>) -> Result<Vec<WorkspaceObject>> {
        let sql = "SELECT o.stable_id,o.object_type,o.title,o.body,f.relative_path,f.hash,o.created,o.updated,o.frontmatter_json FROM objects o JOIN files f ON f.id=o.file_id WHERE (?1 IS NULL OR o.object_type=?1) AND o.identity_status='unique' ORDER BY o.updated DESC,o.title";
        let mut statement = self
            .connection
            .prepare(sql)
            .map_err(|error| CoreError::index(error, "object_query"))?;
        let rows = statement
            .query_map([object_type], row_to_object)
            .map_err(|error| CoreError::index(error, "object_query"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| CoreError::index(error, "object_query"))
    }

    pub fn search(&self, input: &SearchInput) -> Result<Vec<SearchResult>> {
        let tokens = input
            .query
            .split_whitespace()
            .filter(|value| !value.is_empty())
            .map(|value| format!("\"{}\"*", value.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare("SELECT NULLIF(fts.stable_id,''),o.object_type,fts.relative_path,fts.title,snippet(object_fts,4,'','', ' … ',18),bm25(object_fts,0.0,2.0,4.0,8.0,1.0,3.0),f.hash FROM object_fts fts JOIN files f ON f.id=fts.rowid LEFT JOIN objects o ON o.file_id=f.id WHERE object_fts MATCH ?1 AND (?2 IS NULL OR o.object_type=?2) AND (?3 IS NULL OR fts.relative_path LIKE ?3 || '%') ORDER BY 6 LIMIT ?4").map_err(|error| CoreError::index(error,"search_query"))?;
        let rows = statement
            .query_map(
                params![
                    tokens,
                    input.object_type,
                    input.path_prefix,
                    input.limit.unwrap_or(50)
                ],
                |row| {
                    Ok(SearchResult {
                        object_id: row.get(0)?,
                        object_type: row.get(1)?,
                        relative_path: row.get(2)?,
                        title: row.get(3)?,
                        snippet: row.get(4)?,
                        score: row.get(5)?,
                        revision: row.get(6)?,
                    })
                },
            )
            .map_err(|error| CoreError::index(error, "search_query"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| CoreError::index(error, "search_query"))
    }

    pub fn calendar(&self, range_start: &str, range_end: &str) -> Result<Vec<CalendarEntry>> {
        let range_start = calendar_instant(range_start)?;
        let range_end = calendar_instant(range_end)?;
        if range_start >= range_end {
            return Err(CoreError::validation(
                "invalid_calendar_range",
                "The calendar range end must be after its start",
                "calendar_query",
            ));
        }
        let mut statement = self.connection.prepare("SELECT o.stable_id,o.object_type,o.title,p.property_name,p.value_text,e.value_text,f.hash FROM object_properties p JOIN objects o ON o.file_id=p.file_id JOIN files f ON f.id=p.file_id LEFT JOIN object_properties e ON e.file_id=p.file_id AND e.property_name='end' WHERE p.property_name IN ('due','date','start')").map_err(|error| CoreError::index(error,"calendar_query"))?;
        let rows = statement
            .query_map([], |row| {
                let start: String = row.get(4)?;
                Ok(CalendarEntry {
                    source_id: row.get(0)?,
                    source_type: row.get(1)?,
                    title: row.get(2)?,
                    property: row.get(3)?,
                    all_day: start.len() == 10,
                    start,
                    end: row.get(5)?,
                    revision: row.get(6)?,
                })
            })
            .map_err(|error| CoreError::index(error, "calendar_query"))?;
        let mut entries = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| CoreError::index(error, "calendar_query"))?;
        entries.retain(|entry| {
            calendar_instant(&entry.start)
                .is_ok_and(|start| start >= range_start && start < range_end)
        });
        entries.sort_by_key(|entry| calendar_instant(&entry.start).ok());
        Ok(entries)
    }

    pub fn file_count(&self) -> Result<u64> {
        self.connection
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .map_err(|error| CoreError::index(error, "index_count"))
    }

    pub fn diagnostics(&self) -> Result<Vec<crate::Diagnostic>> {
        let mut values = Vec::new();
        let mut malformed = self
            .connection
            .prepare("SELECT relative_path,parse_error FROM files WHERE parse_error IS NOT NULL")
            .map_err(|error| CoreError::index(error, "diagnostics"))?;
        for row in malformed
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| CoreError::index(error, "diagnostics"))?
        {
            let (path, message) = row.map_err(|error| CoreError::index(error, "diagnostics"))?;
            values.push(crate::Diagnostic {
                code: "parse_error".into(),
                message,
                relative_path: Some(path),
                object_id: None,
            });
        }
        let mut conflicts=self.connection.prepare("SELECT stable_id,group_concat(f.relative_path, ', ') FROM objects o JOIN files f ON f.id=o.file_id WHERE o.identity_status='conflict' GROUP BY stable_id").map_err(|error|CoreError::index(error,"diagnostics"))?;
        for row in conflicts
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| CoreError::index(error, "diagnostics"))?
        {
            let (id, paths) = row.map_err(|error| CoreError::index(error, "diagnostics"))?;
            values.push(crate::Diagnostic {
                code: "identity_conflict".into(),
                message: format!("Multiple files use this stable ID: {paths}"),
                relative_path: None,
                object_id: Some(id),
            });
        }
        let mut broken=self.connection.prepare("SELECT o.stable_id,f.relative_path,l.target_stable_id FROM object_links l JOIN objects o ON o.file_id=l.source_file_id JOIN files f ON f.id=l.source_file_id LEFT JOIN objects target ON target.stable_id=l.target_stable_id WHERE target.stable_id IS NULL").map_err(|error|CoreError::index(error,"diagnostics"))?;
        for row in broken
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| CoreError::index(error, "diagnostics"))?
        {
            let (id, path, target) = row.map_err(|error| CoreError::index(error, "diagnostics"))?;
            values.push(crate::Diagnostic {
                code: "broken_reference".into(),
                message: format!("Referenced object does not exist: {target}"),
                relative_path: Some(path),
                object_id: Some(id),
            });
        }
        Ok(values)
    }
}

fn upsert_markdown_tx(
    tx: &rusqlite::Transaction<'_>,
    relative_path: &str,
    bytes: &[u8],
    mtime_ns: i64,
    parsed: &ParsedMarkdown,
) -> Result<()> {
    let (status, parse_error) = match parsed {
        ParsedMarkdown::Managed(_) => ("managed", None),
        ParsedMarkdown::Unmanaged { .. } => ("unmanaged", None),
        ParsedMarkdown::Malformed { error, .. } => ("malformed", Some(error.as_str())),
    };
    tx.execute("INSERT INTO files(relative_path,kind,size,mtime_ns,hash,parse_status,parse_error,indexed_at) VALUES(?1,'markdown',?2,?3,?4,?5,?6,?7) ON CONFLICT(relative_path) DO UPDATE SET size=excluded.size,mtime_ns=excluded.mtime_ns,hash=excluded.hash,parse_status=excluded.parse_status,parse_error=excluded.parse_error,indexed_at=excluded.indexed_at",
        params![relative_path, bytes.len() as i64, mtime_ns, revision(bytes), status, parse_error, crate::now_rfc3339()]).map_err(|error| CoreError::index(error, "index_upsert"))?;
    let file_id: i64 = tx
        .query_row(
            "SELECT id FROM files WHERE relative_path=?1",
            [relative_path],
            |row| row.get(0),
        )
        .map_err(|error| CoreError::index(error, "index_upsert"))?;
    tx.execute("DELETE FROM object_fts WHERE rowid=?1", [file_id])
        .map_err(|error| CoreError::index(error, "index_upsert"))?;
    tx.execute(
        "DELETE FROM object_links WHERE source_file_id=?1",
        [file_id],
    )
    .map_err(|error| CoreError::index(error, "index_upsert"))?;
    tx.execute("DELETE FROM object_properties WHERE file_id=?1", [file_id])
        .map_err(|error| CoreError::index(error, "index_upsert"))?;
    tx.execute("DELETE FROM objects WHERE file_id=?1", [file_id])
        .map_err(|error| CoreError::index(error, "index_upsert"))?;
    match parsed {
        ParsedMarkdown::Managed(object) => {
            let frontmatter =
                serde_json::to_string(&object.properties).unwrap_or_else(|_| "{}".into());
            tx.execute("INSERT INTO objects(file_id,stable_id,object_type,title,body,frontmatter_json,created,updated) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![file_id, object.id, object.object_type, object.title, object.body, frontmatter, object.created, object.updated]).map_err(|error| CoreError::index(error, "index_upsert"))?;
            for (key, value) in &object.properties {
                let (value_type, value_text) = scalar(value);
                tx.execute("INSERT INTO object_properties(file_id,property_name,value_text,value_type) VALUES(?1,?2,?3,?4)", params![file_id,key,value_text,value_type]).map_err(|error| CoreError::index(error, "index_upsert"))?;
                if key == "project"
                    && let Some(target) = value.as_str()
                {
                    tx.execute("INSERT OR IGNORE INTO object_links(source_file_id,relation,target_stable_id) VALUES(?1,'project',?2)", params![file_id,target]).map_err(|error| CoreError::index(error,"index_upsert"))?;
                }
            }
            let metadata = object
                .properties
                .values()
                .map(value_text)
                .collect::<Vec<_>>()
                .join(" ");
            tx.execute("INSERT INTO object_fts(rowid,stable_id,relative_path,filename,title,body,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![file_id,object.id,relative_path,filename(relative_path),object.title,object.body,metadata]).map_err(|error| CoreError::index(error,"index_upsert"))?;
        }
        ParsedMarkdown::Unmanaged { title, body, .. }
        | ParsedMarkdown::Malformed { title, body, .. } => {
            tx.execute("INSERT INTO object_fts(rowid,stable_id,relative_path,filename,title,body,metadata) VALUES(?1,'',?2,?3,?4,?5,'')", params![file_id,relative_path,filename(relative_path),title,body]).map_err(|error| CoreError::index(error,"index_upsert"))?;
        }
    }
    Ok(())
}

fn calendar_instant(value: &str) -> Result<jiff::Timestamp> {
    let normalized = if value.len() == 10 {
        format!("{value}T00:00:00Z")
    } else {
        value.to_owned()
    };
    normalized.parse::<jiff::Timestamp>().map_err(|_| {
        CoreError::validation(
            "invalid_calendar_date",
            "Calendar values must use YYYY-MM-DD or RFC 3339 with an explicit offset",
            "calendar_query",
        )
    })
}

fn filename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
}
fn value_text(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
fn scalar(value: &serde_json::Value) -> (&'static str, String) {
    let kind = match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    };
    (kind, value_text(value))
}
fn update_identity_status(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute("UPDATE objects SET identity_status=CASE WHEN (SELECT COUNT(*) FROM objects b WHERE b.stable_id=objects.stable_id)>1 THEN 'conflict' ELSE 'unique' END",[])?;
    Ok(())
}
fn row_to_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceObject> {
    let json: String = row.get(8)?;
    let properties: HashMap<String, serde_json::Value> =
        serde_json::from_str(&json).unwrap_or_default();
    Ok(WorkspaceObject {
        id: row.get(0)?,
        object_type: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        relative_path: row.get(4)?,
        revision: row.get(5)?,
        created: row.get(6)?,
        updated: row.get(7)?,
        properties: properties.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{new_object_id, now_rfc3339, parse_markdown, serialize_object};
    use std::collections::BTreeMap;
    #[test]
    fn fts_and_rebuildable_index_work() {
        let mut index = IndexStore::in_memory().unwrap();
        let now = now_rfc3339();
        let object = WorkspaceObject {
            id: new_object_id("note"),
            object_type: "note".into(),
            title: "Delta Search".into(),
            body: "rareword body".into(),
            relative_path: "notes/delta.md".into(),
            revision: String::new(),
            created: Some(now.clone()),
            updated: Some(now),
            properties: BTreeMap::new(),
        };
        let bytes = serialize_object(&object).unwrap();
        let parsed = parse_markdown(&object.relative_path, &bytes);
        index
            .upsert_markdown(&object.relative_path, &bytes, 0, &parsed)
            .unwrap();
        let found = index
            .search(&SearchInput {
                query: "rareword".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].object_id.as_deref(), Some(object.id.as_str()));
        index.clear().unwrap();
        assert!(index.query_objects(None).unwrap().is_empty());
    }

    #[test]
    fn upsert_removes_properties_that_disappeared_from_a_file() {
        let mut index = IndexStore::in_memory().unwrap();
        let now = now_rfc3339();
        let mut object = WorkspaceObject {
            id: new_object_id("task"),
            object_type: "task".into(),
            title: "Task".into(),
            body: String::new(),
            relative_path: "task.md".into(),
            revision: String::new(),
            created: Some(now.clone()),
            updated: Some(now),
            properties: BTreeMap::from([("due".into(), serde_json::json!("2026-09-04"))]),
        };
        let bytes = serialize_object(&object).unwrap();
        index
            .upsert_markdown("task.md", &bytes, 1, &parse_markdown("task.md", &bytes))
            .unwrap();
        object.properties.clear();
        let bytes = serialize_object(&object).unwrap();
        index
            .upsert_markdown("task.md", &bytes, 2, &parse_markdown("task.md", &bytes))
            .unwrap();
        assert!(
            index
                .calendar("2026-09-01", "2026-10-01")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn calendar_compares_rfc3339_values_as_instants() {
        let mut index = IndexStore::in_memory().unwrap();
        let now = now_rfc3339();
        let object = WorkspaceObject {
            id: new_object_id("task"),
            object_type: "task".into(),
            title: "Offset task".into(),
            body: String::new(),
            relative_path: "offset.md".into(),
            revision: String::new(),
            created: Some(now.clone()),
            updated: Some(now),
            properties: BTreeMap::from([(
                "due".into(),
                serde_json::json!("2026-09-01T00:30:00+02:00"),
            )]),
        };
        let bytes = serialize_object(&object).unwrap();
        index
            .upsert_markdown("offset.md", &bytes, 1, &parse_markdown("offset.md", &bytes))
            .unwrap();
        let entries = index
            .calendar("2026-08-31T22:00:00Z", "2026-08-31T23:00:00Z")
            .unwrap();
        assert_eq!(entries.len(), 1);
    }
}
