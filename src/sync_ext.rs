//! Poison-tolerant locking.
//!
//! A `std::sync::Mutex` is poisoned when a thread panics while holding it, and
//! every later `lock().unwrap()` then panics too, so one bug on one thread
//! spreads to every thread that shares the lock (a session's tokio workers, the
//! capture and encode threads). The data macrdp keeps behind these locks
//! (caches, counters, handles, per-connection state) stays usable after such a
//! panic, so recovering the guard is the right default. Use these instead of
//! `lock().unwrap()`.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub trait LockExt<T: ?Sized> {
    /// Lock, taking the guard even if an earlier holder panicked.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> LockExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub trait RwLockExt<T: ?Sized> {
    /// Read-lock, taking the guard even if an earlier writer panicked.
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T>;
    /// Write-lock, taking the guard even if an earlier writer panicked.
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T: ?Sized> RwLockExt<T> for RwLock<T> {
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_lock_is_recovered() {
        let m = std::sync::Arc::new(Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.is_poisoned());
        *m.lock_or_recover() += 1;
        assert_eq!(*m.lock_or_recover(), 2);
    }
}
