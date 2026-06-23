use std::{mem::ManuallyDrop, path::Path};

use fd_lock::{RwLock as FdRwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreLockAccess {
    ReadOnly,
    Write,
}

enum StoreLockGuard {
    Write(ManuallyDrop<RwLockWriteGuard<'static, std::fs::File>>),
    Read(ManuallyDrop<RwLockReadGuard<'static, std::fs::File>>),
}

pub(crate) struct StoreLock {
    guard: StoreLockGuard,
    rw_lock: ManuallyDrop<Box<FdRwLock<std::fs::File>>>,
}

impl std::fmt::Debug for StoreLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreLock").finish_non_exhaustive()
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // SAFETY: Drop order is critical. The guard borrows from the boxed
        // lock, so the guard must be dropped before the box is released.
        unsafe {
            match &mut self.guard {
                StoreLockGuard::Write(g) => ManuallyDrop::drop(g),
                StoreLockGuard::Read(g) => ManuallyDrop::drop(g),
            }
            ManuallyDrop::drop(&mut self.rw_lock);
        }
    }
}

pub(crate) fn acquire_store_lock(
    lock_path: &Path,
    access: StoreLockAccess,
    create: bool,
) -> Result<StoreLock> {
    let file = std::fs::OpenOptions::new()
        .create(create)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| AppError::io(lock_path, e))?;

    let mut rw_lock = Box::new(FdRwLock::new(file));

    // SAFETY: The Box gives the fd lock a stable address. StoreLock's custom
    // Drop implementation releases the guard before dropping the Box.
    let lock_ref: &'static mut FdRwLock<std::fs::File> =
        unsafe { &mut *(rw_lock.as_mut() as *mut _) };

    let guard = match access {
        StoreLockAccess::Write => {
            let g = lock_ref.write().map_err(|e| AppError::StoreLocked {
                lock_path: lock_path.to_path_buf(),
                source: Box::new(e),
            })?;
            StoreLockGuard::Write(ManuallyDrop::new(g))
        }
        StoreLockAccess::ReadOnly => {
            let g = lock_ref.read().map_err(|e| AppError::StoreLocked {
                lock_path: lock_path.to_path_buf(),
                source: Box::new(e),
            })?;
            StoreLockGuard::Read(ManuallyDrop::new(g))
        }
    };

    Ok(StoreLock {
        guard,
        rw_lock: ManuallyDrop::new(rw_lock),
    })
}
