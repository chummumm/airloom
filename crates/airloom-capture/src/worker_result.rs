use crate::{CaptureError, Result};
use std::sync::Mutex;
use std::thread::JoinHandle;

/// A source failure belongs to the reader. Cleanup has its own result so an
/// exited application cannot prevent the caller from replacing a stopped stream.
pub(crate) struct WorkerExit {
    pub(crate) capture: Result<()>,
    pub(crate) cleanup: Result<()>,
}

impl WorkerExit {
    pub(crate) fn complete(self, terminal_error: &Mutex<Option<CaptureError>>) -> Result<()> {
        if let Err(error) = self.capture {
            if let Ok(mut slot) = terminal_error.lock() {
                *slot = Some(error);
            }
        }
        self.cleanup
    }
}

pub(crate) fn join_worker(worker: JoinHandle<Result<()>>) -> Result<()> {
    worker.join().map_err(|_| CaptureError::WorkerPanicked)?
}

/// Always attempt both operations. A failed mute restore is the actionable
/// cleanup failure even when WASAPI's Stop call also failed.
pub(crate) fn cleanup_result(stopped: Result<()>, mute_restored: Result<()>) -> Result<()> {
    mute_restored.and(stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn an_exited_application_is_reported_to_the_reader_but_does_not_fail_stop() {
        let terminal_error = Arc::new(Mutex::new(None));
        let worker_error = terminal_error.clone();
        let worker = std::thread::spawn(move || {
            WorkerExit {
                capture: Err(CaptureError::ApplicationExited),
                cleanup: Ok(()),
            }
            .complete(&worker_error)
        });
        assert!(join_worker(worker).is_ok());
        assert!(matches!(
            terminal_error.lock().unwrap().take(),
            Some(CaptureError::ApplicationExited)
        ));
    }

    #[test]
    fn simultaneous_capture_and_cleanup_failures_remain_independent() {
        let terminal_error = Mutex::new(None);
        let cleanup = WorkerExit {
            capture: Err(CaptureError::Conversion("capture packet".into())),
            cleanup: Err(CaptureError::Windows {
                operation: "Restore endpoint mute",
                code: -1,
            }),
        }
        .complete(&terminal_error);
        assert!(matches!(
            terminal_error.lock().unwrap().take(),
            Some(CaptureError::Conversion(message)) if message == "capture packet"
        ));
        assert!(matches!(
            cleanup,
            Err(CaptureError::Windows {
                operation: "Restore endpoint mute",
                code: -1,
            })
        ));
    }

    #[test]
    fn cleanup_failures_are_not_published_as_source_failures() {
        let terminal_error = Mutex::new(None);
        let cleanup = WorkerExit {
            capture: Ok(()),
            cleanup: Err(CaptureError::Windows {
                operation: "Stop WASAPI capture",
                code: -2,
            }),
        }
        .complete(&terminal_error);
        assert!(terminal_error.lock().unwrap().is_none());
        assert!(matches!(
            cleanup,
            Err(CaptureError::Windows {
                operation: "Stop WASAPI capture",
                code: -2,
            })
        ));
    }

    #[test]
    fn mute_restoration_failure_is_not_hidden_by_a_wasapi_stop_failure() {
        let cleanup = cleanup_result(
            Err(CaptureError::Windows {
                operation: "Stop WASAPI capture",
                code: -2,
            }),
            Err(CaptureError::Windows {
                operation: "Restore endpoint mute",
                code: -1,
            }),
        );
        assert!(matches!(
            cleanup,
            Err(CaptureError::Windows {
                operation: "Restore endpoint mute",
                code: -1,
            })
        ));
    }

    #[test]
    fn a_worker_panic_is_a_stop_failure() {
        let worker = std::thread::spawn(|| -> Result<()> { panic!("capture worker test panic") });
        assert!(matches!(
            join_worker(worker),
            Err(CaptureError::WorkerPanicked)
        ));
    }
}
