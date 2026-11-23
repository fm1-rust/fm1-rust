//! Yield to other ready work on the current dispatcher.
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// A future that requests one more poll before completing.
#[derive(Clone, Copy, Debug, Default)]
pub struct YieldNow(bool);

impl YieldNow {
    /// Create a fresh yield.
    #[must_use]
    pub const fn new() -> Self {
        Self(false)
    }
}

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if core::mem::replace(&mut self.0, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Let the dispatcher poll other ready tasks.
pub async fn yield_now() {
    YieldNow::new().await
}
