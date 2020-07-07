//! Helpers for quirks of the current tokio runtime.

use std::cell::RefCell;
use std::future::Future;
use std::sync::{Arc, Weak, Mutex};
use std::task::{Context, Poll, RawWaker, Waker};
use std::thread::{self, Thread};

use lazy_static::lazy_static;
use tokio::runtime::{self, Runtime};

thread_local! {
    static BLOCKING: RefCell<bool> = RefCell::new(false);
}

fn is_in_tokio() -> bool {
    tokio::runtime::Handle::try_current()
        .is_ok()
}

fn is_blocking() -> bool {
    BLOCKING.with(|v| *v.borrow())
}

struct BlockingGuard(bool);

impl BlockingGuard {
    fn set() -> Self {
        Self(BLOCKING.with(|v| {
            let old = *v.borrow();
            *v.borrow_mut() = true;
            old
        }))
    }
}

impl Drop for BlockingGuard {
    fn drop(&mut self) {
        BLOCKING.with(|v| {
            *v.borrow_mut() = self.0;
        });
    }
}

lazy_static! {
    // avoid openssl bug: https://github.com/openssl/openssl/issues/6214
    // by dropping the runtime as early as possible
    static ref RUNTIME: Mutex<Weak<Runtime>> = Mutex::new(Weak::new());
}

extern {
    fn OPENSSL_thread_stop();
}

/// Get or create the current main tokio runtime.
///
/// This makes sure that tokio's worker threads are marked for us so that we know whether we
/// can/need to use `block_in_place` in our `block_on` helper.
pub fn get_runtime_with_builder<F: Fn() -> runtime::Builder>(get_builder: F) -> Arc<Runtime> {

    let mut guard = RUNTIME.lock().unwrap();

    if let Some(rt) = guard.upgrade() { return rt; }

    let mut builder = get_builder();
    builder.on_thread_stop(|| {
        // avoid openssl bug: https://github.com/openssl/openssl/issues/6214
        // call OPENSSL_thread_stop to avoid race with openssl cleanup handlers
        unsafe { OPENSSL_thread_stop(); }
    });

    let runtime = builder.build().expect("failed to spawn tokio runtime");
    let rt = Arc::new(runtime);

    *guard = Arc::downgrade(&rt.clone());

    rt
}

/// Get or create the current main tokio runtime.
///
/// This calls get_runtime_with_builder() using the tokio default threaded scheduler
pub fn get_runtime() -> Arc<Runtime> {

    get_runtime_with_builder(|| {
        let mut builder = runtime::Builder::new();
        builder.threaded_scheduler();
        builder.enable_all();
        builder
    })
}


/// Block on a synchronous piece of code.
pub fn block_in_place<R>(fut: impl FnOnce() -> R) -> R {
    // don't double-exit the context (tokio doesn't like that)
    // also, if we're not actually in a tokio-worker we must not use block_in_place() either
    if is_blocking() || !is_in_tokio() {
        fut()
    } else {
        // we are in an actual tokio worker thread, block it:
        tokio::task::block_in_place(move || {
            let _guard = BlockingGuard::set();
            fut()
        })
    }
}

/// Block on a future in this thread.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    // don't double-exit the context (tokio doesn't like that)
    if is_blocking() {
        block_on_local_future(fut)
    } else if is_in_tokio() {
        // inside a tokio worker we need to tell tokio that we're about to really block:
        tokio::task::block_in_place(move || {
            let _guard = BlockingGuard::set();
            block_on_local_future(fut)
        })
    } else {
        // not a worker thread, not associated with a runtime, make sure we have a runtime (spawn
        // it on demand if necessary), then enter it
        let _guard = BlockingGuard::set();
        get_runtime().enter(move || block_on_local_future(fut))
    }
}

/*
fn block_on_impl<F>(mut fut: F) -> F::Output
where
    F: Future + Send,
    F::Output: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let fut_ptr = &mut fut as *mut F as usize; // hack to not require F to be 'static
    tokio::spawn(async move {
        let fut: F = unsafe { std::ptr::read(fut_ptr as *mut F) };
        tx
            .send(fut.await)
            .map_err(drop)
            .expect("failed to send block_on result to channel")
    });

    futures::executor::block_on(async move {
        rx.await.expect("failed to receive block_on result from channel")
    })
    std::mem::forget(fut);
}
*/

/// This used to be our tokio main entry point. Now this just calls out to `block_on` for
/// compatibility, which will perform all the necessary tasks on-demand anyway.
pub fn main<F: Future>(fut: F) -> F::Output {
    block_on(fut)
}

fn block_on_local_future<F: Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };

    let waker = Arc::new(thread::current());
    let waker = thread_waker_clone(Arc::into_raw(waker) as *const ());
    let waker = unsafe { Waker::from_raw(waker) };
    let mut context = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut context) {
            Poll::Ready(out) => return out,
            Poll::Pending => thread::park(),
        }
    }
}

const THREAD_WAKER_VTABLE: std::task::RawWakerVTable = std::task::RawWakerVTable::new(
    thread_waker_clone,
    thread_waker_wake,
    thread_waker_wake_by_ref,
    thread_waker_drop,
);

fn thread_waker_clone(this: *const ()) -> RawWaker {
    let this = unsafe { Arc::from_raw(this as *const Thread) };
    let cloned = Arc::clone(&this);
    let _ = Arc::into_raw(this);

    RawWaker::new(Arc::into_raw(cloned) as *const (), &THREAD_WAKER_VTABLE)
}

fn thread_waker_wake(this: *const ()) {
    let this = unsafe { Arc::from_raw(this as *const Thread) };
    this.unpark();
}

fn thread_waker_wake_by_ref(this: *const ()) {
    let this = unsafe { Arc::from_raw(this as *const Thread) };
    this.unpark();
    let _ = Arc::into_raw(this);
}

fn thread_waker_drop(this: *const ()) {
    let this = unsafe { Arc::from_raw(this as *const Thread) };
    drop(this);
}
