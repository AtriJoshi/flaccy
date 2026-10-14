//! The equalizer's saved state as one small JSON file beside `config.toml`. A
//! file that cannot be read, or that comes from a newer schema, is ignored
//! rather than trusted, and the equalizer starts off and flat.

use flaccy_shared::equalizer::state::EqualizerState;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long a state waits after the last change before it is written, so a drag
/// that edits the curve fifty times a second saves once.
const WRITE_DELAY: Duration = Duration::from_millis(500);

enum Message {
    Save(Box<EqualizerState>),
    Shutdown,
}

pub struct EqualizerStore {
    path: PathBuf,
    sender: Sender<Message>,
    writer: Option<JoinHandle<()>>,
}

pub fn default_path() -> PathBuf {
    crate::config::config_dir().join("equalizer.json")
}

/// Reads the state at `path`, falling back to the default on any problem.
pub fn load_from(path: &std::path::Path) -> EqualizerState {
    let Ok(bytes) = std::fs::read(path) else {
        return EqualizerState::default();
    };
    match serde_json::from_slice::<EqualizerState>(&bytes) {
        Ok(state) if state.schema_version <= EqualizerState::SCHEMA_VERSION => state,
        _ => EqualizerState::default(),
    }
}

fn write_atomically(path: &std::path::Path, state: &EqualizerState) {
    let Ok(bytes) = serde_json::to_vec(state) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, bytes).is_ok() && std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

impl EqualizerStore {
    pub fn new(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel::<Message>();
        let target = path.clone();
        let writer = std::thread::Builder::new()
            .name("flaccy-eq-store".into())
            .spawn(move || {
                let mut pending: Option<Box<EqualizerState>> = None;
                loop {
                    let message = if pending.is_some() {
                        match receiver.recv_timeout(WRITE_DELAY) {
                            Ok(message) => Some(message),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => Some(Message::Shutdown),
                        }
                    } else {
                        receiver.recv().ok().or(Some(Message::Shutdown))
                    };
                    match message {
                        Some(Message::Save(state)) => pending = Some(state),
                        Some(Message::Shutdown) => {
                            if let Some(state) = pending.take() {
                                write_atomically(&target, &state);
                            }
                            return;
                        }
                        None => {
                            if let Some(state) = pending.take() {
                                write_atomically(&target, &state);
                            }
                        }
                    }
                }
            })
            .ok();
        Self { path, sender, writer }
    }

    pub fn load(&self) -> EqualizerState {
        load_from(&self.path)
    }

    pub fn save(&self, state: &EqualizerState) {
        let _ = self.sender.send(Message::Save(Box::new(state.clone())));
    }
}

impl Drop for EqualizerStore {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Shutdown);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flaccy-eq-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.join("equalizer.json")
    }

    #[test]
    fn a_missing_or_corrupt_file_loads_the_default() {
        let path = scratch("corrupt");
        assert_eq!(load_from(&path), EqualizerState::default());
        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(load_from(&path), EqualizerState::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_newer_schema_is_ignored() {
        let path = scratch("newer");
        let mut state = EqualizerState::default();
        state.schema_version = EqualizerState::SCHEMA_VERSION + 1;
        state.is_enabled = true;
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).expect("write");
        assert_eq!(load_from(&path), EqualizerState::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_burst_of_saves_lands_as_the_last_state_and_survives_a_drop() {
        let path = scratch("burst");
        {
            let store = EqualizerStore::new(path.clone());
            for index in 0..20 {
                let mut state = EqualizerState::default();
                state.is_enabled = true;
                state.output_names.insert("sink".into(), format!("Output {index}"));
                store.save(&state);
            }
        }
        let loaded = load_from(&path);
        assert!(loaded.is_enabled);
        assert_eq!(loaded.output_names.get("sink").map(String::as_str), Some("Output 19"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
