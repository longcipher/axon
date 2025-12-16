use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use eyre::{Result, eyre};

/// Helper to inspect and buffer request bodies for WAF analysis
#[derive(Clone)]
pub struct BodyInspector {
    pub max_body_size: usize,
    pub buffer: Arc<Mutex<Vec<u8>>>,
    pub enabled: bool,
}

impl BodyInspector {
    pub fn new(max_body_size: usize, enabled: bool) -> Self {
        Self {
            max_body_size,
            buffer: Arc::new(Mutex::new(Vec::new())),
            enabled,
        }
    }

    pub fn append_chunk(&self, chunk: &Bytes) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let mut buffer = self
            .buffer
            .lock()
            .map_err(|_| eyre!("Failed to lock body buffer"))?;

        if buffer.len() + chunk.len() > self.max_body_size {
            return Err(eyre!(
                "Request body exceeds maximum size of {} bytes",
                self.max_body_size
            ));
        }

        buffer.extend_from_slice(chunk);
        Ok(())
    }

    pub fn get_body(&self) -> Vec<u8> {
        if let Ok(buffer) = self.buffer.lock() {
            buffer.clone()
        } else {
            Vec::new()
        }
    }

    pub fn clear(&self) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
    }
}
