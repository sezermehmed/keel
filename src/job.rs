//! Run a blocking piece of work on another thread and wake the UI when it finishes.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread;

pub fn on_thread<T, F>(work: F) -> impl Future<Output = T> + Send
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let state = Arc::new(State {
        result: Mutex::new(None),
        waker: Mutex::new(None),
        started: AtomicBool::new(false),
        work: Mutex::new(Some(work)),
    });

    std::future::poll_fn(move |cx| poll_job(Arc::clone(&state), cx))
}

struct State<T, F> {
    result: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
    started: AtomicBool,
    work: Mutex<Option<F>>,
}

fn poll_job<T, F>(state: Arc<State<T, F>>, cx: &mut Context<'_>) -> Poll<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if !state.started.swap(true, Ordering::SeqCst) {
        let work = state
            .work
            .lock()
            .expect("install thread lock")
            .take()
            .expect("install work missing");
        let thread_state = Arc::clone(&state);
        thread::spawn(move || {
            let value = work();
            *thread_state.result.lock().expect("install thread lock") = Some(value);
            if let Some(waker) = thread_state
                .waker
                .lock()
                .expect("install thread lock")
                .take()
            {
                waker.wake();
            }
        });
    }

    if let Some(value) = state.result.lock().expect("install thread lock").take() {
        return Poll::Ready(value);
    }
    *state.waker.lock().expect("install thread lock") = Some(cx.waker().clone());
    if let Some(value) = state.result.lock().expect("install thread lock").take() {
        return Poll::Ready(value);
    }
    Poll::Pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Wake;
    use std::time::{Duration, Instant};

    struct Flag(Arc<AtomicBool>);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn on_thread_wakes_the_waiter() {
        let mut future = std::pin::pin!(on_thread(|| {
            thread::sleep(Duration::from_millis(40));
            41 + 1
        }));
        let flag = Arc::new(AtomicBool::new(false));
        let waker = Waker::from(Arc::new(Flag(Arc::clone(&flag))));
        let mut context = Context::from_waker(&waker);

        assert!(
            matches!(future.as_mut().poll(&mut context), Poll::Pending),
            "the worker should still be running after the first poll"
        );

        let started = Instant::now();
        while !flag.load(Ordering::SeqCst) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the worker never woke the UI"
            );
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(future.as_mut().poll(&mut context), Poll::Ready(42));
    }
}
