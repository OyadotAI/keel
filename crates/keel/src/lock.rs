//! Taking a lock that a panic cannot take away.
//!
//! There are thirty-odd `Mutex`es in the daemon and they hold the things a window is waiting on:
//! the approval queue, the background jobs, the session rules, the pairing code. Every one of them
//! used to be taken with `.lock().expect(...)`.
//!
//! That is the wrong end of the trade. A panic inside any critical section poisons its lock
//! *permanently*, and from then on every request touching it panics too — so a single panic in
//! `approve` does not cost one approval, it costs every approval for the life of the process. The
//! app sees a dropped connection each time, the person sees a turn that stopped and never asked
//! anything, and nothing in the window can say why. That is the "refused without asking" symptom
//! this repository already watches for, arriving by a route no amount of care at the call sites
//! would close.
//!
//! So: recover the data and carry on. It may be inconsistent — a `Vec<Pending>` a panic
//! interrupted mid-push — but a queue that has lost one entry is recoverably wrong, and a queue
//! nobody can ever read again is not. The poison flag is cleared so the next caller does not
//! report it a second time, and the fact is sent up, because a panic in a critical section is a
//! bug we want to see even though it no longer stops the daemon.

use std::sync::{Mutex, MutexGuard};

pub trait Locked<T> {
    /// The guard, whether or not a previous holder panicked.
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // Once, not once per caller: the flag stays set until it is cleared, and the
                // interesting event is the panic, not the thousand locks that follow it.
                self.clear_poison();
                sentry::capture_message(
                    "recovered a poisoned lock — a panic happened inside a critical section",
                    sentry::Level::Error,
                );
                poisoned.into_inner()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: the second caller must get the data, not the panic.
    #[test]
    fn a_panic_under_the_lock_does_not_take_the_data_with_it() {
        let m = std::sync::Arc::new(Mutex::new(vec![1, 2, 3]));
        let holder = std::sync::Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let mut guard = holder.locked();
            guard.push(4);
            panic!("something went wrong while holding it");
        })
        .join();

        assert!(m.lock().is_err(), "the mutex really is poisoned");
        assert_eq!(*m.locked(), vec![1, 2, 3, 4], "the data comes back anyway");
        assert!(
            m.lock().is_ok(),
            "and the poison is cleared for the next caller"
        );
    }
}
