//! Shared librime engine wrapper.

use keytao_core::{default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct CoreEngine(Arc<Mutex<Option<Engine>>>);

impl CoreEngine {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    pub fn init(&self) -> Result<(), String> {
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let user = user_dir.to_string_lossy().into_owned();
        let shared = default_shared_data_dir();

        deploy(user.clone(), shared)?;
        let engine = Engine::new()?;
        *self.0.lock().unwrap() = Some(engine);
        Ok(())
    }

    pub fn process_key(&self, keycode: u32, mask: u32) -> Option<ImeState> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|e| e.process_key(keycode, mask))
    }

    pub fn select_candidate(&self, index: usize) -> Option<ImeState> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|e| e.select_candidate(index))
    }

    pub fn change_page(&self, backward: bool) -> Option<ImeState> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|e| e.change_page(backward))
    }

    pub fn reset(&self) -> Option<ImeState> {
        self.0.lock().unwrap().as_ref().map(|e| e.reset())
    }
}
