use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::Result;
use crate::path::PathBuilder;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentMetadata {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

impl AgentMetadata {
    pub fn created_at_datetime(&self) -> Result<DateTime<Utc>> {
        Ok(DateTime::parse_from_rfc3339(&self.created_at)?.with_timezone(&Utc))
    }
}

pub struct AgentManager {
    pool: SqlitePool,
    path_builder: PathBuilder,
}

impl AgentManager {
    pub async fn new(root_dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let path_builder = PathBuilder::new(root_dir);
        let db_path = path_builder.agent_db_path();

        let db_url = format!("sqlite:{}", db_path.to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await?;

        Self::initialize_database(&pool).await?;

        Ok(Self { pool, path_builder })
    }

    async fn initialize_database(pool: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn create_agent(&self, name: String) -> Result<AgentMetadata> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO agents (id, name, created_at)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(&name)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(AgentMetadata { id, name, created_at })
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<AgentMetadata> {
        let agent = sqlx::query_as::<_, AgentMetadata>(
            r#"
            SELECT id, name, created_at
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
            SELECT id, name, created_at
            FROM agents
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(agents)
    }

    pub fn path_builder(&self) -> &PathBuilder {
        &self.path_builder
    }
}
