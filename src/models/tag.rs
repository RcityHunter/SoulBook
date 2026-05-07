use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use surrealdb::types::{Datetime, RecordId as Thing};
use validator::Validate;

lazy_static::lazy_static! {
    static ref HEX_COLOR_REGEX: Regex = Regex::new(r"^#[0-9A-Fa-f]{6}$").unwrap();
}

pub fn hex_color_regex() -> &'static Regex {
    &HEX_COLOR_REGEX
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    #[serde(default, deserialize_with = "deserialize_optional_record_id")]
    pub id: Option<Thing>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub color: String,
    #[serde(default, deserialize_with = "deserialize_optional_record_id")]
    pub space_id: Option<Thing>,
    pub usage_count: i64,
    pub created_by: String,
    pub created_at: Datetime,
    pub updated_at: Datetime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentTag {
    #[serde(default, deserialize_with = "deserialize_optional_record_id")]
    pub id: Option<Thing>,
    pub document_id: Thing,
    pub tag_id: Thing,
    pub tagged_by: String,
    pub tagged_at: Datetime,
}

fn deserialize_optional_record_id<'de, D>(deserializer: D) -> Result<Option<Thing>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum MaybeRecordId {
        RecordId(Thing),
        String(String),
        None,
    }

    match Option::<MaybeRecordId>::deserialize(deserializer)? {
        Some(MaybeRecordId::RecordId(thing)) => Ok(Some(thing)),
        Some(MaybeRecordId::String(value)) => record_id_from_string(&value)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid record id: {}", value))),
        Some(MaybeRecordId::None) | None => Ok(None),
    }
}

fn record_id_from_string(value: &str) -> Option<Thing> {
    let (table, key) = value.split_once(':')?;
    let key = key
        .strip_prefix('⟨')
        .and_then(|inner| inner.strip_suffix('⟩'))
        .unwrap_or(key);

    Some(Thing::new(table, key))
}

#[derive(Debug, Validate, Deserialize)]
pub struct CreateTagRequest {
    #[validate(length(min = 1, max = 50))]
    pub name: String,
    #[validate(length(max = 200))]
    pub description: Option<String>,
    #[validate(length(min = 4, max = 7))]
    pub color: String,
    pub space_id: Option<String>,
}

#[derive(Debug, Validate, Deserialize)]
pub struct UpdateTagRequest {
    #[validate(length(min = 1, max = 50))]
    pub name: Option<String>,
    #[validate(length(max = 200))]
    pub description: Option<String>,
    #[validate(length(min = 4, max = 7))]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TagDocumentRequest {
    pub document_id: String,
    pub tag_ids: Vec<String>,
}

impl Tag {
    pub fn new(name: String, color: String, created_by: String) -> Self {
        let slug = Self::generate_slug(&name);
        Self {
            id: None,
            name,
            slug,
            description: None,
            color,
            space_id: None,
            usage_count: 0,
            created_by,
            created_at: Datetime::default(),
            updated_at: Datetime::default(),
        }
    }

    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    pub fn with_space(mut self, space_id: Option<Thing>) -> Self {
        self.space_id = space_id;
        self
    }

    pub fn generate_slug(name: &str) -> String {
        name.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<&str>>()
            .join("-")
    }

    pub fn increment_usage(&mut self) {
        self.usage_count += 1;
    }

    pub fn decrement_usage(&mut self) {
        if self.usage_count > 0 {
            self.usage_count -= 1;
        }
    }
}

impl DocumentTag {
    pub fn new(document_id: Thing, tag_id: Thing, tagged_by: String) -> Self {
        Self {
            id: None,
            document_id,
            tag_id,
            tagged_by,
            tagged_at: Datetime::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database::record_id_to_string;

    #[test]
    fn tag_accepts_string_record_ids_from_surreal_query() {
        let tag: Tag = serde_json::from_value(serde_json::json!({
            "id": "tag:abc123",
            "name": "Design",
            "slug": "design",
            "description": "desc",
            "color": "#6366f1",
            "space_id": "space:space123",
            "usage_count": 0,
            "created_by": "user-1",
            "created_at": "2026-05-07T06:00:00Z",
            "updated_at": "2026-05-07T06:00:00Z"
        }))
        .expect("tag should deserialize string record ids");

        assert_eq!(
            tag.id.as_ref().map(record_id_to_string).as_deref(),
            Some("tag:abc123")
        );
        assert_eq!(
            tag.space_id.as_ref().map(record_id_to_string).as_deref(),
            Some("space:space123")
        );
    }
}
