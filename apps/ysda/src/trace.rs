use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::domain::AgentRun;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct TraceRecorder {
    root: PathBuf,
}

impl TraceRecorder {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn save(&self, run: &AgentRun) -> AppResult<PathBuf> {
        fs::create_dir_all(&self.root).map_err(|error| AppError::Trace(error.to_string()))?;
        let path = self.path_for(run.run_id);
        let bytes =
            serde_json::to_vec_pretty(run).map_err(|error| AppError::Trace(error.to_string()))?;
        fs::write(&path, bytes).map_err(|error| AppError::Trace(error.to_string()))?;
        Ok(path)
    }

    pub fn load(&self, run_id: Uuid) -> AppResult<AgentRun> {
        let bytes =
            fs::read(self.path_for(run_id)).map_err(|error| AppError::Trace(error.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|error| AppError::Trace(error.to_string()))
    }

    fn path_for(&self, run_id: Uuid) -> PathBuf {
        self.root.join(format!("{run_id}.json"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
