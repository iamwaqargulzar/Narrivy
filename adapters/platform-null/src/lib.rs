//! Null platform adapter used for portable integration tests.

use narrivy_platform_api::{PlatformError, TargetProvider, TextDelivery};
use narrivy_types::{ApplicationIdentity, TargetContext};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct NullPlatform {
    delivered: Arc<Mutex<Vec<String>>>,
}

impl NullPlatform {
    pub fn delivered(&self) -> Vec<String> {
        match self.delivered.lock() {
            Ok(values) => values.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl TargetProvider for NullPlatform {
    fn capture_target(&self) -> Result<TargetContext, PlatformError> {
        Ok(TargetContext {
            application: Some(ApplicationIdentity {
                app_id: "null.app".into(),
                display_name: "Null App".into(),
            }),
            window_title: Some("Test".into()),
        })
    }
}

impl TextDelivery for NullPlatform {
    fn insert_text(&self, _target: &TargetContext, text: &str) -> Result<(), PlatformError> {
        match self.delivered.lock() {
            Ok(mut values) => values.push(text.to_owned()),
            Err(mut poisoned) => poisoned.get_mut().push(text.to_owned()),
        }
        Ok(())
    }
}
