//! COM apartment ownership for one thread.

use narrivy_platform_api::PlatformError;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::error;

/// Joins the calling thread to the multithreaded apartment for its lifetime.
/// Not `Send`: it must be dropped on the thread that created it, after every
/// COM object that thread created.
pub(crate) struct ComApartment {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl ComApartment {
    pub(crate) fn enter() -> Result<Self, PlatformError> {
        // SAFETY: no reserved pointer; called on the thread that will own
        // the apartment. S_FALSE (already initialised in the MTA) is success
        // and must be balanced by CoUninitialize like S_OK.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_err() {
            // RPC_E_CHANGED_MODE: the thread is in an STA. Capture threads are
            // ours, so this only happens if a caller misuses the type.
            return Err(error("com_initialize_failed"));
        }
        Ok(Self {
            _not_send: std::marker::PhantomData,
        })
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `enter`, on the
        // same thread (the type is !Send).
        unsafe { CoUninitialize() };
    }
}
