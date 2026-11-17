//! High-resolution asynchronous sleeps.
//!
//! [`tokio::time::sleep`] rounds deadlines up to the next millisecond, and on
//! some platforms the underlying wait is rounded up further still to the system
//! timer resolution (which can be as coarse as ~16ms on Windows). That's too
//! imprecise for `wasi:clocks`' monotonic clock, whose accuracy is directly
//! observable by guests -- e.g. guest code which paces frames by sleeping
//! between them -- so this module implements a small timer facility which
//! resolves deadlines with the best precision the host offers.
//!
//! A single, lazily started thread is shared by all sleepers. It waits for the
//! earliest registered deadline and then wakes the corresponding tasks. Each
//! wait is split into two phases: an interruptible, coarse wait for the bulk of
//! the time, followed by a short, precise wait for the remainder. The coarse
//! phase ensures that newly registered deadlines are noticed promptly, while
//! the precise phase is what provides sub-millisecond accuracy.
//!
//! Unlike [`tokio::time::sleep`] this does not require any of Tokio's drivers to
//! be enabled; it only needs a [`Waker`], so it works with any executor.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// How much of a sleep is spent in the precise phase rather than in the
/// interruptible, coarse phase.
///
/// This must comfortably exceed the granularity of the coarse phase so that its
/// overshoot cannot consume the entire precise phase. On Windows a condition
/// variable timeout is rounded up to the system timer tick, which is typically
/// 15.6ms, so a full tick is used there.
#[cfg(not(windows))]
const PRECISE_PHASE: Duration = Duration::from_millis(1);
#[cfg(windows)]
const PRECISE_PHASE: Duration = Duration::from_millis(16);

/// Maximum duration of a single precise wait on Windows, where precise waits
/// cannot be interrupted and are therefore performed in slices so that newly
/// registered deadlines are still noticed promptly.
#[cfg(windows)]
const PRECISE_SLICE: Duration = Duration::from_millis(2);

/// Sleep for `duration`.
///
/// If `duration` is too large to be represented as an [`Instant`], this waits
/// forever.
pub(crate) async fn sleep(duration: Duration) {
    match Instant::now().checked_add(duration) {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Sleep until `deadline`.
pub(crate) async fn sleep_until(deadline: Instant) {
    Sleep {
        deadline,
        entry: None,
    }
    .await
}

/// The future returned by [`sleep`] and [`sleep_until`].
struct Sleep {
    deadline: Instant,
    entry: Option<Arc<Entry>>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let deadline = this.deadline;
        if Instant::now() >= deadline {
            return Poll::Ready(());
        }
        let entry = match &this.entry {
            Some(entry) => entry,
            None => this.entry.insert(timer().register(deadline, cx.waker())),
        };
        if entry.poll(cx.waker()) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            timer().cancel(&entry);
        }
    }
}

/// Returns the process-wide timer thread, starting it on first use.
fn timer() -> Arc<Shared> {
    static TIMER: OnceLock<Arc<Shared>> = OnceLock::new();
    TIMER
        .get_or_init(|| {
            let shared = Arc::new(Shared::default());
            let thread_shared = shared.clone();
            std::thread::Builder::new()
                .name("wasmtime-wasi-timer".into())
                .spawn(move || run(&thread_shared))
                .expect("failed to spawn timer thread");
            shared
        })
        .clone()
}

/// State shared between the timer thread and the sleepers.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Notified when work is registered or cancelled, or when a coarse wait
    /// expires.
    ready: Condvar,
}

#[derive(Default)]
struct State {
    /// Deadlines, earliest first.
    queue: BinaryHeap<Reverse<HeapEntry>>,
    /// Number of cancelled entries which are still in `queue`.
    cancelled: usize,
    /// Bumped whenever `queue` changes, so that the precise phase can notice
    /// newly registered deadlines.
    generation: u64,
}

impl Shared {
    fn register(self: &Arc<Self>, deadline: Instant, waker: &Waker) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            deadline,
            seq: NEXT_SEQ.fetch_add(1, AtomicOrdering::Relaxed),
            state: Mutex::new(EntryState {
                waker: Some(waker.clone()),
                fired: false,
            }),
            cancelled: AtomicBool::new(false),
            popped: AtomicBool::new(false),
        });
        let mut state = lock(&self.state);
        state.queue.push(Reverse(HeapEntry(entry.clone())));
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.ready.notify_one();
        entry
    }

    fn cancel(&self, entry: &Entry) {
        let mut state = lock(&self.state);
        // Entries are removed from the queue by the timer thread, which marks
        // them as popped while holding this same lock, so this is exact.
        if entry.popped.load(AtomicOrdering::Acquire) {
            return;
        }
        entry.cancelled.store(true, AtomicOrdering::Release);
        state.cancelled += 1;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.ready.notify_one();
    }
}

/// Sequence number used to break ties between identical deadlines, keeping the
/// [`BinaryHeap`] ordering total.
static NEXT_SEQ: AtomicU64 = AtomicU64::new(0);

/// A registered sleep.
struct Entry {
    deadline: Instant,
    seq: u64,
    state: Mutex<EntryState>,
    /// Whether this entry has been cancelled, i.e. the sleeping task went away.
    cancelled: AtomicBool,
    /// Whether this entry has been removed from the queue.
    popped: AtomicBool,
}

struct EntryState {
    waker: Option<Waker>,
    fired: bool,
}

impl Entry {
    /// Stores `waker` to be woken once this entry's deadline has elapsed,
    /// returning whether it has already elapsed.
    fn poll(&self, waker: &Waker) -> bool {
        let mut state = lock(&self.state);
        if state.fired {
            return true;
        }
        if !state.waker.as_ref().is_some_and(|w| w.will_wake(waker)) {
            state.waker = Some(waker.clone());
        }
        false
    }

    fn fire(&self) {
        let waker = {
            let mut state = lock(&self.state);
            state.fired = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// A [`BinaryHeap`] entry, ordered by deadline and then by registration order.
struct HeapEntry(Arc<Entry>);

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0.deadline, self.0.seq).cmp(&(other.0.deadline, other.0.seq))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take down the timer thread permanently.
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The timer thread: wakes sleepers as their deadlines elapse.
fn run(shared: &Shared) {
    let mut fired = Vec::new();
    loop {
        let mut state = lock(&shared.state);

        // Collect all expired entries, dropping cancelled entries as they reach
        // the front of the queue.
        let now = Instant::now();
        while let Some(Reverse(HeapEntry(entry))) = state.queue.peek() {
            if entry.cancelled.load(AtomicOrdering::Acquire) {
                entry.popped.store(true, AtomicOrdering::Release);
                state.queue.pop();
                state.cancelled -= 1;
            } else if entry.deadline <= now {
                entry.popped.store(true, AtomicOrdering::Release);
                let Reverse(HeapEntry(entry)) = state.queue.pop().unwrap();
                fired.push(entry);
            } else {
                break;
            }
        }
        compact(&mut state);
        drop(state);

        // Wake outside of the lock, since wakers may immediately try to take it.
        for entry in fired.drain(..) {
            entry.fire();
        }

        // Wait for the next deadline, or for new work to be registered. The
        // queue is inspected while holding the lock which is handed to the wait
        // below, so registrations racing with this are never missed.
        let state = lock(&shared.state);
        let Some(Reverse(HeapEntry(entry))) = state.queue.peek() else {
            drop(shared.ready.wait(state));
            continue;
        };
        let deadline = entry.deadline;
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            continue;
        };
        if remaining > PRECISE_PHASE {
            drop(shared.ready.wait_timeout(state, remaining - PRECISE_PHASE));
        } else {
            precise_wait(shared, state, deadline);
        }
    }
}

/// Waits until `deadline` without allowing new work to interrupt the wait.
///
/// The caller passes the guard for the shared state's lock, which is released
/// before sleeping.
#[cfg(not(windows))]
fn precise_wait(_shared: &Shared, state: MutexGuard<'_, State>, deadline: Instant) {
    drop(state);
    if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        std::thread::sleep(remaining);
    }
}

/// Waits until `deadline` in short slices, abandoning the wait as soon as new
/// work is registered so that an earlier deadline is not delayed.
///
/// The caller passes the guard for the shared state's lock, which is released
/// before sleeping.
#[cfg(windows)]
fn precise_wait(shared: &Shared, state: MutexGuard<'_, State>, deadline: Instant) {
    let generation = state.generation;
    drop(state);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        std::thread::sleep(remaining.min(PRECISE_SLICE));
        if lock(&shared.state).generation != generation {
            return;
        }
    }
}

/// Rebuilds the queue when it holds many cancelled entries which are not at its
/// front and therefore won't be dropped any time soon.
fn compact(state: &mut State) {
    if state.cancelled > 64 && state.cancelled * 2 > state.queue.len() {
        state
            .queue
            .retain(|Reverse(HeapEntry(entry))| !entry.cancelled.load(AtomicOrdering::Acquire));
        state.cancelled = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest median overshoot a precise sleep may exhibit.
    #[cfg(not(windows))]
    const MAX_OVERSHOOT: Duration = Duration::from_micros(750);
    #[cfg(windows)]
    const MAX_OVERSHOOT: Duration = Duration::from_millis(3);

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        // Note that neither Tokio's time nor I/O drivers are enabled here;
        // sleeping must not depend on them.
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn sleeps_are_precise() {
        for duration in [
            Duration::from_micros(250),
            Duration::from_millis(1),
            Duration::from_millis(5),
        ] {
            let median = block_on(async {
                let mut samples = Vec::new();
                for _ in 0..21 {
                    let start = Instant::now();
                    sleep(duration).await;
                    samples.push(start.elapsed().saturating_sub(duration));
                }
                samples.sort();
                samples[samples.len() / 2]
            });
            assert!(
                median < MAX_OVERSHOOT,
                "median overshoot of {median:?} when sleeping for {duration:?} \
                 exceeds {MAX_OVERSHOOT:?}"
            );
        }
    }

    #[test]
    fn sleeps_never_return_early() {
        block_on(async {
            for _ in 0..50 {
                let start = Instant::now();
                sleep(Duration::from_millis(1)).await;
                assert!(start.elapsed() >= Duration::from_millis(1));
            }
        });
    }

    #[test]
    fn zero_duration_sleep_is_ready_immediately() {
        let mut sleep = std::pin::pin!(sleep(Duration::ZERO));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(sleep.as_mut().poll(&mut cx), Poll::Ready(()));
    }

    #[test]
    fn cancelled_sleeps_do_not_delay_others() {
        block_on(async {
            // Register a long sleep and then cancel it before it elapses.
            let mut long = std::pin::pin!(sleep(Duration::from_secs(3600)));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(long.as_mut().poll(&mut cx).is_pending());
            drop(long);

            // A subsequent, much shorter sleep must not be delayed by it.
            let start = Instant::now();
            sleep(Duration::from_millis(1)).await;
            assert!(start.elapsed() < Duration::from_secs(1));
        });
    }

    #[test]
    fn earlier_deadlines_are_not_delayed() {
        block_on(async {
            // Register a long sleep and then, from a separate task, a shorter
            // one. The timer thread must notice the shorter deadline rather
            // than sleeping until the long one elapses.
            let long = tokio::spawn(async {
                sleep(Duration::from_secs(3600)).await;
            });
            tokio::task::yield_now().await;
            let start = Instant::now();
            sleep(Duration::from_millis(1)).await;
            assert!(start.elapsed() < Duration::from_secs(1));
            long.abort();
        });
    }

    #[test]
    fn many_sleeps_elapse_in_order() {
        block_on(async {
            let mut tasks = Vec::new();
            for i in 0..20u64 {
                tasks.push(tokio::spawn(async move {
                    sleep(Duration::from_micros(100 + i * 500)).await;
                    (i, Instant::now())
                }));
            }
            let mut finished = Vec::new();
            for task in tasks {
                finished.push(task.await.unwrap());
            }
            finished.sort_by_key(|(i, _)| *i);
            let times: Vec<_> = finished.iter().map(|(_, t)| *t).collect();
            assert!(times.is_sorted(), "sleeps completed out of order");
        });
    }
}
