use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::collections::HashMap;

use super::cache::DbCache;

const PINNED: i64 = 1 << 11;
const FOLDED_GROUP: i64 = 1 << 28;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Attention {
    pub pinned: Option<bool>,
    pub muted: Option<bool>,
    pub folded: Option<bool>,
    pub archived: Option<bool>,
    pub notification_level: Option<i64>,
}

impl Attention {
    fn from_contact(username: &str, flag: Option<i64>, notify: Option<i64>) -> Self {
        let is_group = username.ends_with("@chatroom");
        Self {
            pinned: flag.map(|value| value & PINNED != 0),
            muted: if is_group {
                match notify {
                    Some(0) => Some(true),
                    Some(1) => Some(false),
                    _ => None,
                }
            } else {
                None
            },
            folded: if is_group {
                flag.map(|value| value & FOLDED_GROUP != 0)
            } else {
                Some(false)
            },
            archived: None,
            notification_level: if is_group { notify } else { None },
        }
    }
}

pub fn for_session(username: &str, contacts: &HashMap<String, Attention>) -> Attention {
    let mut attention = contacts.get(username).cloned().unwrap_or_default();
    if username == "@placeholder_foldgroup" || username == "brandsessionholder" {
        attention.folded = Some(true);
    }
    attention
}

fn read_contacts(conn: &Connection) -> Result<HashMap<String, Attention>> {
    let mut stmt = conn
        .prepare("SELECT username, flag, chat_room_notify FROM contact")
        .context("contact.db 缺少 contact 偏好字段")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("读取 contact.db 会话偏好失败")?;
    Ok(rows
        .into_iter()
        .map(|(username, flag, notify)| {
            let attention = Attention::from_contact(&username, flag, notify);
            (username, attention)
        })
        .collect())
}

pub async fn load_attention(db: &DbCache) -> Result<HashMap<String, Attention>> {
    let path = db
        .get("contact/contact.db")
        .await?
        .context("无法解密 contact.db，不能读取会话偏好")?;
    tokio::task::spawn_blocking(move || {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("只读打开 contact.db 失败")?;
        read_contacts(&conn)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_pin_mute_and_fold_are_independent() {
        let attention =
            Attention::from_contact("group@chatroom", Some(PINNED | FOLDED_GROUP), Some(0));
        assert_eq!(attention.pinned, Some(true));
        assert_eq!(attention.muted, Some(true));
        assert_eq!(attention.folded, Some(true));
        assert_eq!(attention.notification_level, Some(0));
        assert_eq!(attention.archived, None);
        let normal = Attention::from_contact("group@chatroom", Some(0), Some(1));
        assert_eq!(normal.muted, Some(false));
        assert_eq!(normal.folded, Some(false));
    }

    #[test]
    fn private_notify_zero_does_not_mean_muted() {
        let attention = Attention::from_contact("wxid_person", Some(PINNED), Some(0));
        assert_eq!(attention.pinned, Some(true));
        assert_eq!(attention.muted, None);
        assert_eq!(attention.notification_level, None);
        assert_eq!(attention.folded, Some(false));
    }

    #[test]
    fn unknown_fields_and_missing_contacts_stay_unknown() {
        let attention = Attention::from_contact("group@chatroom", None, Some(2));
        assert_eq!(attention.pinned, None);
        assert_eq!(attention.folded, None);
        assert_eq!(attention.muted, None);
        assert_eq!(attention.notification_level, Some(2));
        let contacts = HashMap::new();
        assert_eq!(
            for_session("missing@chatroom", &contacts),
            Attention::default()
        );
        assert_eq!(
            for_session("@placeholder_foldgroup", &contacts).folded,
            Some(true)
        );
    }

    #[test]
    fn missing_schema_fails_clearly() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE contact (username TEXT, flag INTEGER)", [])
            .unwrap();
        let error = read_contacts(&conn).unwrap_err().to_string();
        assert!(error.contains("contact.db 缺少 contact 偏好字段"));
    }
}
