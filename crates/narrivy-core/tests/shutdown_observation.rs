use narrivy_core::{WorkerStop, WorkerSupervisor};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

struct Epilogue {
    entered: Sender<()>,
    release: Arc<AtomicBool>,
}

impl Drop for Epilogue {
    fn drop(&mut self) {
        let _ = self.entered.send(());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.release.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

struct ReleaseGuard(Arc<AtomicBool>);

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

thread_local! {
    static EPILOGUE: RefCell<Option<Epilogue>> = const { RefCell::new(None) };
}

#[test]
fn observation_shares_one_deadline_across_pending_native_joins()
-> Result<(), Box<dyn std::error::Error>> {
    let supervisor = Arc::new(WorkerSupervisor::new());
    let release = Arc::new(AtomicBool::new(false));
    let release_guard = ReleaseGuard(Arc::clone(&release));
    let (entered_tx, entered_rx) = channel();
    for _ in 0..8 {
        let entered = entered_tx.clone();
        let release = Arc::clone(&release);
        supervisor.spawn("observation-epilogue", move |_| {
            EPILOGUE.with(|slot| *slot.borrow_mut() = Some(Epilogue { entered, release }));
        })?;
    }
    for _ in 0..8 {
        entered_rx.recv_timeout(Duration::from_secs(2))?;
    }
    let outcomes = supervisor.shutdown_all_until(Instant::now() + Duration::from_millis(10))?;
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome)| *outcome == WorkerStop::TimedOut)
    );

    let started = Instant::now();
    let count = supervisor.live_worker_count_until(started + Duration::from_millis(20));
    let elapsed = started.elapsed();
    drop(release_guard);
    let final_outcomes = supervisor.shutdown_all_until(Instant::now() + Duration::from_secs(2))?;
    assert_eq!(count, 8);
    assert!(
        elapsed < Duration::from_millis(80),
        "observation accumulated waits: {elapsed:?}"
    );
    assert!(
        final_outcomes
            .iter()
            .all(|(_, outcome)| *outcome == WorkerStop::Stopped)
    );
    Ok(())
}
