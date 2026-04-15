use chrono::{DateTime, Utc, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::Result;
use crate::path;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentMetadata {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
}

impl AgentMetadata {
    pub fn created_at_datetime(&self) -> Result<DateTime<Utc>> {
        let naive = NaiveDateTime::parse_from_str(&self.created_at, "%Y-%m-%d %H:%M:%S")?;
        Ok(DateTime::from_naive_utc_and_offset(naive, Utc))
    }
}

pub struct AgentManager {
    pool: SqlitePool,
}

impl AgentManager {
    pub async fn new(root_dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref().to_path_buf();
        let db_path = path::agent_db_path(&root_dir);

        let db_url = format!("sqlite:{}", db_path.to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await?;

        Self::initialize_database(&pool).await?;

        Ok(Self { pool })
    }

    async fn initialize_database(pool: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn create_agent(&self, name: String, description: Option<String>) -> Result<AgentMetadata> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        sqlx::query(
            r#"
            INSERT INTO agents (id, name, description, created_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(&name)
        .bind(&description)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(AgentMetadata { id, name, description, created_at })
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<AgentMetadata> {
        let agent = sqlx::query_as::<_, AgentMetadata>(
            r#"
            SELECT id, name, description, created_at
            FROM agents
            WHERE id = ?
            "#,
        )
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(agent)
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentMetadata>> {
        let agents = sqlx::query_as::<_, AgentMetadata>(
            r#"
            SELECT id, name, description, created_at
            FROM agents
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(agents)
    }

    pub async fn update_agent_name(&self, agent_id: &str, name: String) -> Result<AgentMetadata> {
        sqlx::query(
            r#"
            UPDATE agents
            SET name = ?
            WHERE id = ?
            "#,
        )
        .bind(&name)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;

        self.get_agent(agent_id).await
    }

    pub async fn update_agent_description(&self, agent_id: &str, description: Option<String>) -> Result<AgentMetadata> {
        sqlx::query(
            r#"
            UPDATE agents
            SET description = ?
            WHERE id = ?
            "#,
        )
        .bind(&description)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;

        self.get_agent(agent_id).await
    }
}
