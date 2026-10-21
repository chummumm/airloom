//! Event-driven source discovery. There is no periodic refresh task.

use crate::{CaptureError, CaptureSource, Result};
use tokio::sync::watch;

/// A consistent snapshot of the source watcher's retained Windows sessions.
/// Each scope reports its own error so one unavailable session manager does not
/// hide otherwise usable playback endpoints or applications on other devices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceSnapshot {
    pub endpoints: Vec<CaptureSource>,
    pub applications: Vec<CaptureSource>,
    pub endpoint_error: Option<String>,
    pub application_error: Option<String>,
}

/// Owns one Windows source subscription. Dropping it requests unregistration on
/// its COM worker; cleanup waits at most one second on the caller's thread.
/// Dense changes are merged into the latest snapshot, rather than an event log.
pub struct SourceChanges {
    updates: watch::Receiver<SourceSnapshot>,
    initial: bool,
    #[cfg(windows)]
    control: crate::platform::SourceWatch,
}

/// Starts an MTA worker, registers notifications, and takes the initial snapshot.
/// Startup can block on Windows APIs; async callers should use `spawn_blocking`.
/// Automatic updates come only from device/session notifications and process
/// exit handles. `refresh` is the sole explicit re-enumeration request.
pub fn watch_sources() -> Result<SourceChanges> {
    #[cfg(windows)]
    {
        let (control, updates) = crate::platform::SourceWatch::start()?;
        Ok(SourceChanges {
            updates,
            initial: true,
            control,
        })
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::UnsupportedPlatform)
    }
}

impl SourceChanges {
    /// Returns the initial snapshot immediately, then awaits genuine changes or
    /// an explicit refresh. Cancel-safe: cancelling this future loses no update.
    pub async fn recv(&mut self) -> Result<SourceSnapshot> {
        receive_snapshot(&mut self.updates, &mut self.initial).await
    }

    /// Reads the most recently completed snapshot without initiating enumeration.
    pub fn snapshot(&self) -> SourceSnapshot {
        self.updates.borrow().clone()
    }

    /// Requests a manual refresh, including sessions retained from callbacks.
    /// A completed refresh is published even if its contents did not change.
    pub fn refresh(&self) -> Result<()> {
        #[cfg(windows)]
        {
            self.control.refresh()
        }
        #[cfg(not(windows))]
        {
            Err(CaptureError::UnsupportedPlatform)
        }
    }
}

async fn receive_snapshot(
    updates: &mut watch::Receiver<SourceSnapshot>,
    initial: &mut bool,
) -> Result<SourceSnapshot> {
    if *initial {
        *initial = false;
    } else {
        updates
            .changed()
            .await
            .map_err(|_| CaptureError::WorkerStopped)?;
    }
    Ok(updates.borrow_and_update().clone())
}

#[cfg(any(windows, test))]
pub(crate) mod signal {
    use crossbeam_channel::{bounded, Receiver, Sender};
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::sync::Arc;

    pub(crate) const DEVICES: u8 = 1;
    pub(crate) const APPLICATIONS: u8 = 2;
    pub(crate) const REFRESH: u8 = 4;
    const CLOSED: usize = 1 << (usize::BITS - 1);

    /// A bounded wake token plus dirty bits merges bursts without a timer. The
    /// callback gate keeps the new-session queue alive until its writers leave.
    pub(crate) struct ChangeSignal {
        dirty: AtomicU8,
        callbacks: AtomicUsize,
        wake: Sender<()>,
    }

    impl ChangeSignal {
        pub(crate) fn channel() -> (Arc<Self>, Receiver<()>) {
            let (wake, receiver) = bounded(1);
            (
                Arc::new(Self {
                    dirty: AtomicU8::new(0),
                    callbacks: AtomicUsize::new(0),
                    wake,
                }),
                receiver,
            )
        }

        pub(crate) fn notify(&self, changes: u8) {
            if !self.is_closed() {
                self.dirty.fetch_or(changes, Ordering::Release);
                let _ = self.wake.try_send(());
            }
        }

        pub(crate) fn take_changes(&self) -> u8 {
            self.dirty.swap(0, Ordering::AcqRel)
        }

        pub(crate) fn close(&self) {
            self.callbacks.fetch_or(CLOSED, Ordering::AcqRel);
            let _ = self.wake.try_send(());
        }

        pub(crate) fn is_closed(&self) -> bool {
            self.callbacks.load(Ordering::Acquire) & CLOSED != 0
        }

        pub(crate) fn has_callbacks(&self) -> bool {
            self.callbacks.load(Ordering::Acquire) & !CLOSED != 0
        }

        pub(crate) fn enter(&self) -> Option<CallbackEntry<'_>> {
            self.callbacks
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    (value & CLOSED == 0).then_some(value + 1)
                })
                .ok()
                .map(|_| CallbackEntry(self))
        }
    }

    pub(crate) struct CallbackEntry<'a>(&'a ChangeSignal);

    impl Drop for CallbackEntry<'_> {
        fn drop(&mut self) {
            if self.0.callbacks.fetch_sub(1, Ordering::AcqRel) == CLOSED + 1 {
                let _ = self.0.wake.try_send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::signal::{ChangeSignal, APPLICATIONS, DEVICES, REFRESH};
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    #[test]
    fn bursts_are_coalesced_without_losing_change_kinds() {
        let (signal, wake) = ChangeSignal::channel();
        for _ in 0..100 {
            signal.notify(DEVICES);
            signal.notify(APPLICATIONS);
        }
        signal.notify(REFRESH);
        assert_eq!(wake.len(), 1);
        wake.recv().unwrap();
        assert_eq!(signal.take_changes(), DEVICES | APPLICATIONS | REFRESH);
        assert_eq!(signal.take_changes(), 0);
        assert!(wake.is_empty());
        signal.notify(APPLICATIONS);
        wake.recv().unwrap();
        assert_eq!(signal.take_changes(), APPLICATIONS);
    }

    #[test]
    fn closing_rejects_new_callbacks_and_wakes_after_existing_callback_finishes() {
        let (signal, wake) = ChangeSignal::channel();
        let active = signal.enter().unwrap();
        signal.close();
        wake.recv().unwrap();
        assert!(signal.enter().is_none());
        assert!(signal.has_callbacks());
        drop(active);
        wake.recv().unwrap();
        assert!(!signal.has_callbacks());
        signal.notify(DEVICES);
        assert!(wake.is_empty());
    }

    #[test]
    fn latest_snapshot_survives_bursts_and_cancelled_receive() {
        let (sender, mut updates) = watch::channel(SourceSnapshot::default());
        let mut initial = true;
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            pin!(receive_snapshot(&mut updates, &mut initial)).poll(&mut context),
            Poll::Ready(Ok(_))
        ));
        // Dropping a pending recv must not consume the next notification.
        assert!(pin!(receive_snapshot(&mut updates, &mut initial))
            .poll(&mut context)
            .is_pending());
        for id in ["first", "latest"] {
            sender.send_replace(SourceSnapshot {
                endpoints: vec![CaptureSource {
                    id: id.into(),
                    name: id.into(),
                    is_default: false,
                }],
                ..Default::default()
            });
        }
        let Poll::Ready(Ok(snapshot)) =
            pin!(receive_snapshot(&mut updates, &mut initial)).poll(&mut context)
        else {
            panic!("latest snapshot was lost");
        };
        assert_eq!(snapshot.endpoints[0].id, "latest");
        drop(sender);
        assert!(matches!(
            pin!(receive_snapshot(&mut updates, &mut initial)).poll(&mut context),
            Poll::Ready(Err(CaptureError::WorkerStopped))
        ));
    }
}
