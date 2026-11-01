//! Long-term memory in SQLite: the conversation history and facts about the user.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Older messages are deleted; what matters long-term ends up in `facts`.
const KEEP_MESSAGES: i64 = 1000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fact {
    pub id: i64,
    pub content: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Changes the model proposes after a conversation.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FactChanges {
    pub add: Vec<String>,
    pub update: Vec<FactUpdate>,
    pub delete: Vec<i64>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct FactUpdate {
    pub id: i64,
    pub content: String,
}

pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS messages (
               id INTEGER PRIMARY KEY,
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
             );
             CREATE TABLE IF NOT EXISTS facts (
               id INTEGER PRIMARY KEY,
               content TEXT NOT NULL,
               updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
             );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add_message(&self, role: &str, content: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO messages (role, content) VALUES (?1, ?2)",
            params![role, content],
        )?;
        conn.execute(
            "DELETE FROM messages WHERE id <= (SELECT MAX(id) FROM messages) - ?1",
            params![KEEP_MESSAGES],
        )?;
        Ok(())
    }

    /// The last `limit` messages, oldest first.
    pub fn recent_messages(&self, limit: usize) -> Result<Vec<StoredMessage>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT role, content, created_at FROM
               (SELECT * FROM messages ORDER BY id DESC LIMIT ?1) ORDER BY id",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(StoredMessage {
                role: r.get(0)?,
                content: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn facts(&self) -> Result<Vec<Fact>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, content, updated_at FROM facts ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(Fact {
                id: r.get(0)?,
                content: r.get(1)?,
                updated_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_fact(&self, content: &str) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO facts (content) VALUES (?1)",
            params![content.trim()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_fact(&self, id: i64, content: &str) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE facts SET content = ?2, updated_at = datetime('now', 'localtime') WHERE id = ?1",
            params![id, content.trim()],
        )?;
        Ok(n > 0)
    }

    pub fn delete_fact(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn()
            .execute("DELETE FROM facts WHERE id = ?1", params![id])?
            > 0)
    }

    pub fn clear_messages(&self) -> Result<()> {
        self.conn().execute("DELETE FROM messages", [])?;
        Ok(())
    }

    pub fn clear_facts(&self) -> Result<()> {
        self.conn().execute("DELETE FROM facts", [])?;
        Ok(())
    }

    pub fn apply(&self, changes: &FactChanges) -> Result<()> {
        for content in changes.add.iter().filter(|c| !c.trim().is_empty()) {
            self.add_fact(content)?;
        }
        for u in &changes.update {
            self.update_fact(u.id, &u.content)?;
        }
        for id in &changes.delete {
            self.delete_fact(*id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_messages_in_order() {
        let m = MemoryStore::in_memory().unwrap();
        for i in 0..5 {
            m.add_message("user", &format!("m{i}")).unwrap();
        }
        let recent: Vec<_> = m
            .recent_messages(3)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect();
        assert_eq!(recent, ["m2", "m3", "m4"]);
    }

    #[test]
    fn applies_fact_changes() {
        let m = MemoryStore::in_memory().unwrap();
        let a = m.add_fact("Heisst Veit").unwrap();
        let b = m.add_fact("Mag Tee").unwrap();
        m.apply(&FactChanges {
            add: vec!["Hat einen Hund".into(), " ".into()],
            update: vec![FactUpdate {
                id: b,
                content: "Mag Kaffee schwarz".into(),
            }],
            delete: vec![a],
        })
        .unwrap();
        let facts: Vec<_> = m.facts().unwrap().into_iter().map(|f| f.content).collect();
        assert_eq!(facts, ["Mag Kaffee schwarz", "Hat einen Hund"]);
    }
}
