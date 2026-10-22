//! Bounded daemon event bus with drop-oldest backpressure (ADR-0012).
//!
//! Lifecycle events are content-free. Volatile transcript presentation uses a
//! redacted typed variant with coalescing and lower priority. The queue stays
//! capacity-bounded and every drop is counted.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Debug)]
pub struct EventBus<T> {
    state: Mutex<BusState<T>>,
    available: Condvar,
    capacity: usize,
    evicted_queued: AtomicU64,
    dropped_incoming: AtomicU64,
    coalesced: AtomicU64,
    rejected_closed: AtomicU64,
    published: AtomicU64,
}

#[derive(Debug)]
struct BusState<T> {
    queue: VecDeque<T>,
    closed: bool,
}

impl<T> EventBus<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(BusState {
                queue: VecDeque::with_capacity(capacity),
                closed: false,
            }),
            available: Condvar::new(),
            capacity: capacity.max(1),
            evicted_queued: AtomicU64::new(0),
            dropped_incoming: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            rejected_closed: AtomicU64::new(0),
            published: AtomicU64::new(0),
        }
    }

    pub fn publish(&self, event: T) -> bool {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.closed {
            self.rejected_closed.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        if state.queue.len() >= self.capacity {
            state.queue.pop_front();
            self.evicted_queued.fetch_add(1, Ordering::SeqCst);
        }
        state.queue.push_back(event);
        self.published.fetch_add(1, Ordering::SeqCst);
        drop(state);
        self.available.notify_one();
        true
    }

    /// Publish a control event while preferentially discarding a volatile
    /// event when the queue is full. This keeps high-rate presentation data
    /// from displacing lifecycle truth.
    pub fn publish_priority<F>(&self, event: T, is_volatile: F) -> bool
    where
        F: Fn(&T) -> bool,
    {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.closed {
            self.rejected_closed.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        if state.queue.len() >= self.capacity {
            if let Some(position) = state.queue.iter().position(is_volatile) {
                state.queue.remove(position);
            } else {
                state.queue.pop_front();
            }
            self.evicted_queued.fetch_add(1, Ordering::SeqCst);
        }
        state.queue.push_back(event);
        self.published.fetch_add(1, Ordering::SeqCst);
        drop(state);
        self.available.notify_one();
        true
    }

    /// Replace an already queued volatile value with its newest revision. If
    /// the queue is full of control events, drop the incoming volatile value
    /// instead of sacrificing control ordering. Returns whether it was queued.
    pub fn publish_coalesced<F, E>(&self, event: T, same_stream: F, is_volatile: E) -> bool
    where
        F: Fn(&T) -> bool,
        E: Fn(&T) -> bool,
    {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.closed {
            self.rejected_closed.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        self.published.fetch_add(1, Ordering::SeqCst);
        if let Some(position) = state.queue.iter().position(same_stream) {
            state.queue[position] = event;
            self.coalesced.fetch_add(1, Ordering::SeqCst);
            drop(state);
            self.available.notify_one();
            return true;
        }
        if state.queue.len() >= self.capacity {
            let Some(position) = state.queue.iter().position(is_volatile) else {
                self.dropped_incoming.fetch_add(1, Ordering::SeqCst);
                return false;
            };
            state.queue.remove(position);
            self.evicted_queued.fetch_add(1, Ordering::SeqCst);
        }
        state.queue.push_back(event);
        drop(state);
        self.available.notify_one();
        true
    }

    /// Non-blocking drain of one event.
    pub fn try_next(&self) -> Option<T> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.queue.pop_front()
    }

    /// Wait until an event is available, admission closes, or the defensive
    /// timeout expires. Close and queue state share one mutex, so neither an
    /// accepted publication nor shutdown notification can be lost.
    pub fn wait_next(&self, timeout: Duration) -> Option<T> {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut state = match self.available.wait_timeout_while(state, timeout, |state| {
            state.queue.is_empty() && !state.closed
        }) {
            Ok((state, _)) => state,
            Err(poisoned) => poisoned.into_inner().0,
        };
        state.queue.pop_front()
    }

    /// Atomically close publication admission and wake waiters. Events
    /// accepted before this boundary remain drainable; later publications are
    /// rejected.
    pub fn close(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.closed = true;
        drop(state);
        self.available.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        match self.state.lock() {
            Ok(state) => state.closed,
            Err(poisoned) => poisoned.into_inner().closed,
        }
    }

    pub fn len(&self) -> usize {
        match self.state.lock() {
            Ok(state) => state.queue.len(),
            Err(poisoned) => poisoned.into_inner().queue.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn evicted_queued(&self) -> u64 {
        self.evicted_queued.load(Ordering::SeqCst)
    }

    pub fn dropped_incoming(&self) -> u64 {
        self.dropped_incoming.load(Ordering::SeqCst)
    }

    pub fn coalesced(&self) -> u64 {
        self.coalesced.load(Ordering::SeqCst)
    }

    pub fn rejected_closed(&self) -> u64 {
        self.rejected_closed.load(Ordering::SeqCst)
    }

    pub fn published_seq_total(&self) -> u64 {
        self.published.load(Ordering::SeqCst)
    }
}

impl<T> Default for EventBus<T> {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn bus_drop_oldest_when_full_and_counts_drops() {
        let bus: EventBus<u32> = EventBus::new(3);
        for i in 0..5 {
            bus.publish(i);
        }
        assert_eq!(bus.len(), 3);
        assert_eq!(bus.evicted_queued(), 2);
        assert_eq!(bus.try_next(), Some(2));
        assert_eq!(bus.try_next(), Some(3));
        assert_eq!(bus.try_next(), Some(4));
        assert_eq!(bus.try_next(), None);
    }

    #[test]
    fn bus_counts_publications() {
        let bus: EventBus<()> = EventBus::new(8);
        bus.publish(());
        bus.publish(());
        assert_eq!(bus.published_seq_total(), 2);
    }

    #[test]
    fn priority_publish_discards_volatile_before_control() {
        let bus = EventBus::new(3);
        bus.publish(("control", 1));
        bus.publish(("volatile", 1));
        bus.publish(("control", 2));

        bus.publish_priority(("control", 3), |item| item.0 == "volatile");

        assert_eq!(bus.try_next(), Some(("control", 1)));
        assert_eq!(bus.try_next(), Some(("control", 2)));
        assert_eq!(bus.try_next(), Some(("control", 3)));
        assert_eq!(bus.coalesced(), 0);
        assert_eq!(bus.evicted_queued(), 1);
        assert_eq!(bus.dropped_incoming(), 0);
    }

    #[test]
    fn coalescing_replaces_in_place_and_never_evicts_control_only_queue() {
        let bus = EventBus::new(2);
        bus.publish(("volatile", 1));
        bus.publish(("control", 1));
        assert!(bus.publish_coalesced(
            ("volatile", 2),
            |item| item.0 == "volatile",
            |item| item.0 == "volatile"
        ));
        assert_eq!(bus.try_next(), Some(("volatile", 2)));
        assert_eq!(bus.try_next(), Some(("control", 1)));

        bus.publish(("control", 2));
        bus.publish(("control", 3));
        assert!(!bus.publish_coalesced(("volatile", 3), |_| false, |item| item.0 == "volatile"));
        assert_eq!(bus.try_next(), Some(("control", 2)));
        assert_eq!(bus.try_next(), Some(("control", 3)));
        assert_eq!(bus.coalesced(), 1);
        assert_eq!(bus.evicted_queued(), 0);
        assert_eq!(bus.dropped_incoming(), 1);
    }

    #[test]
    fn waiter_wakes_for_publication_without_polling() -> Result<(), &'static str> {
        let bus = Arc::new(EventBus::new(2));
        let (entered_tx, entered_rx) = sync_channel(1);
        let waiter_bus = bus.clone();
        let (result_tx, result_rx) = sync_channel(1);
        let waiter = std::thread::spawn(move || {
            let _ = entered_tx.send(());
            let _ = result_tx.send(waiter_bus.wait_next(Duration::from_secs(1)));
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "waiter did not start")?;
        assert!(bus.publish(7));
        let received = result_rx
            .recv_timeout(Duration::from_millis(500))
            .map_err(|_| "publication did not wake waiter")?;
        assert_eq!(received, Some(7));
        waiter.join().map_err(|_| "waiter panicked")?;
        Ok(())
    }

    #[test]
    fn close_wake_is_not_lost_and_returns_before_fallback() -> Result<(), &'static str> {
        let bus: Arc<EventBus<()>> = Arc::new(EventBus::new(2));
        let (entered_tx, entered_rx) = sync_channel(1);
        let waiter_bus = bus.clone();
        let (result_tx, result_rx) = sync_channel(1);
        let waiter = std::thread::spawn(move || {
            let _ = entered_tx.send(());
            let _ = result_tx.send(waiter_bus.wait_next(Duration::from_secs(2)));
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "waiter did not start")?;
        bus.close();
        let received = result_rx
            .recv_timeout(Duration::from_millis(500))
            .map_err(|_| "close did not wake waiter")?;
        assert_eq!(received, None);
        waiter.join().map_err(|_| "waiter panicked")?;
        Ok(())
    }

    #[test]
    fn close_drains_admitted_events_and_rejects_late_publication() {
        let bus = EventBus::new(2);
        assert!(bus.publish(1));
        bus.close();
        assert!(!bus.publish(2));
        assert_eq!(bus.wait_next(Duration::from_secs(1)), Some(1));
        assert_eq!(bus.wait_next(Duration::from_secs(1)), None);
        assert_eq!(bus.rejected_closed(), 1);
        assert!(bus.is_closed());
    }
}
