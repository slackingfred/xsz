use std::{
    future::Future,
    hint::cold_path,
    pin::pin,
    sync::{Arc, LazyLock},
    task::{Context, Poll, Wake, Waker},
    thread::{Builder, Thread, current, park},
};

use async_task::{Runnable, Task};
use futures_lite::FutureExt;
use kanal::{Receiver, Sender, unbounded};

use crate::global::config;

/// Stack size for executor worker threads.
const STACK_SIZE: usize = 128 * 1024;

pub struct Executor {
    sender: Sender<Runnable>,
    receiver: Receiver<Runnable>,
}

impl Executor {
    fn new(nthreads: u8) -> Self {
        let (sender, receiver) = unbounded::<Runnable>();
        for i in 0..nthreads {
            let receiver = receiver.clone();
            if let Err(e) = Builder::new()
                .name(format!("xsz-worker{}", i))
                .stack_size(STACK_SIZE)
                .spawn(move || {
                    while let Ok(r) = receiver.recv() {
                        r.run();
                    }
                })
            {
                cold_path();
                // Don't fail hard: `block_on` still runs tasks on the
                // calling thread, so we degrade to fewer workers instead
                // of aborting.  Make the degradation visible though.
                eprintln!(
                    "Failed to spawn worker thread {} ({}); running with fewer jobs",
                    i, e
                );
            }
        }
        Self { sender, receiver }
    }
    #[inline]
    fn schedule(&self, runnable: Runnable) {
        self.sender.send(runnable).unwrap();
    }
}

pub fn global() -> &'static Executor {
    // jobs - 1 because the main thread is also a worker thread when calling block_on
    static EXECUTOR: LazyLock<Executor> = LazyLock::new(|| Executor::new(config().jobs - 1));
    &EXECUTOR
}

pub fn spawn<F>(fut: F) -> Task<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let schedule = |runnable| global().schedule(runnable);
    let (runnable, task) = async_task::spawn(fut, schedule);
    runnable.schedule();
    task
}

pub fn block_on<F>(fut: F) -> F::Output
where
    F: Future,
{
    let recv = global().receiver.clone().to_async();
    let f = fut.or(async move {
        loop {
            match recv.recv().await {
                Ok(r) => {
                    r.run();
                }
                Err(e) => {
                    // The executor's sender lives in a `static`, so this
                    // should be unreachable.  If it ever happens, stop
                    // draining: `recv` would otherwise return `Err` in a
                    // tight loop and starve `fut` forever.
                    cold_path();
                    eprintln!("{}", e);
                    break;
                }
            }
        }
        // Never resolve: keep polling `fut` (which `or` polls first) but
        // stop competing for executor work.
        std::future::pending().await
    });
    let mut f = pin!(f);
    let thread = current();
    struct ThreadWaker {
        thread: Thread,
    }
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.thread.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.thread.unpark();
        }
    }
    let inner = Arc::new(ThreadWaker { thread });
    let waker = Waker::from(inner);
    let mut cx = Context::from_waker(&waker);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(r) => return r,
            Poll::Pending => park(),
        }
    }
}
