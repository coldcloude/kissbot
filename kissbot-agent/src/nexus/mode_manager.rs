use tokio::sync::RwLock;

use crate::nexus::types::Mode;

pub struct ModeManager {
    mode: RwLock<Mode>,
}

impl ModeManager {
    pub fn new(initial_mode: Mode) -> Self {
        Self {
            mode: RwLock::new(initial_mode),
        }
    }

    pub async fn current(&self) -> Mode {
        self.mode.read().await.clone()
    }

    pub async fn set_mode(&self, mode: Mode) {
        *self.mode.write().await = mode;
    }

    /// 检查当前是否为角色模式
    pub async fn is_role_mode(&self) -> bool {
        matches!(*self.mode.read().await, Mode::Role)
    }

    /// 检查当前是否为事件模式，并返回事件 ID
    pub async fn event_id(&self) -> Option<String> {
        match &*self.mode.read().await {
            Mode::Event(id) => Some(id.clone()),
            Mode::Role => None,
        }
    }
}
