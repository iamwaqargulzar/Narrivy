use narrivy_core::{WorkerStop, WorkerSupervisor};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

struct Epilogue {
    entered: Sender<()>,
    release: Receiver<()>,
}

impl Drop for Epilogue {
    fn drop(&mut self) {
        let _ = self.entered.send(());
        let _ = self.release.recv_timeout(Duration::from_secs(5));
    }
}

thread_local! {
    static EPILOGUE: RefCell<Option<Epilogue>> = const { RefCell::new(None) };
}

#[test]
fn completed_job_with_running_thread_destructor_remains_bounded_and_reapable()
-> Result<(), Box<dyn std::error::Error>> {
    let supervisor = Arc::new(WorkerSupervisor::new());
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let handle = supervisor.spawn("blocked-thread-epilogue", move |_| {
        EPILOGUE.with(|slot| {
            *slot.borrow_mut() = Some(Epilogue {
                entered: entered_tx,
                release: release_rx,
            });
        });
    })?;
    // The job and completion send have finished, but the native thread has not.
    entered_rx.recv_timeout(Duration::from_secs(2))?;
    let shutting_down = Arc::clone(&supervisor);
    let (finished_tx, finished_rx) = channel();
    let shutdown = std::thread::spawn(move || {
        let outcome = shutting_down.shutdown_all_until(Instant::now() + Duration::from_millis(100));
        let _ = finished_tx.send(outcome);
    });
    let bounded = finished_rx.recv_timeout(Duration::from_millis(400));
    // Release before asserting, including on the buggy implementation.
    release_tx.send(())?;
    let returned_within_bound = bounded.is_ok();
    let outcomes = match bounded {
        Ok(outcomes) => outcomes?,
        Err(_) => finished_rx.recv_timeout(Duration::from_secs(2))??,
    };
    shutdown.join().map_err(|_| "shutdown thread panicked")?;
    let final_reap = handle.stop(Duration::from_secs(2))?;
    assert!(
        returned_within_bound,
        "completion receipt led to an unbounded join during thread destruction"
    );
    assert!(
        outcomes
            .iter()
            .any(|(_, stop)| *stop == WorkerStop::TimedOut)
    );
    assert_eq!(final_reap, WorkerStop::Stopped);
    Ok(())
}
