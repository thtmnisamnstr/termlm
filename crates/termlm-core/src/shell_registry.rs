use std::collections::BTreeMap;
use termlm_protocol::{ShellContext, ShellKind};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ShellSession {
    pub shell_pid: u32,
    pub tty: String,
    pub shell_kind: ShellKind,
    pub shell_version: String,
    pub env_subset: BTreeMap<String, String>,
    pub context: Option<ShellContext>,
}

#[derive(Debug, Default)]
pub struct ShellRegistry {
    sessions: BTreeMap<Uuid, ShellSession>,
}

impl ShellRegistry {
    pub fn insert(&mut self, shell_id: Uuid, session: ShellSession) {
        self.sessions.insert(shell_id, session);
    }

    pub fn remove(&mut self, shell_id: &Uuid) {
        self.sessions.remove(shell_id);
    }

    pub fn set_context(&mut self, shell_id: Uuid, context: ShellContext) {
        if let Some(session) = self.sessions.get_mut(&shell_id) {
            session.context = Some(context);
        }
    }

    pub fn get(&self, shell_id: &Uuid) -> Option<&ShellSession> {
        self.sessions.get(shell_id)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uuid, &ShellSession)> {
        self.sessions.iter()
    }
}
