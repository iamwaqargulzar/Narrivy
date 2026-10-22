//! Daemon single-instance ownership, delegated to the Linux platform adapter.
//! The portable surface stays identical so callers do not know the difference.

pub use narrivy_platform_linux::instance::{
    InstanceError, InstanceLock, StateDirError, acquire_instance_lock, runtime_dir, state_dir,
};
