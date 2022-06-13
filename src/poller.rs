use futures::task;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// The simplest possible executor.
/// Does not support waking.
pub struct Poller<T: Future + Unpin>(T);

impl<T: Future + Unpin> Poller<T> {
    pub fn new(x: T) -> Self {
        Self(x)
    }

    pub fn poll_once(&mut self) -> Option<<T as Future>::Output> {
        let mut ctx = Context::from_waker(task::noop_waker_ref());
        match Pin::new(&mut self.0).poll(&mut ctx) {
            Poll::Ready(x) => Some(x),
            Poll::Pending => None,
        }
    }
}
