use crate::poller::Poller;
use ghost_cell::GhostToken;
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

pub fn run<'b, F, Fut: Future, T>(f: F, token: GhostToken<'b>, arg: T) -> FutureStatus<'b, Fut>
where
    F: FnOnce(SleepingPill<'b>, GhostToken<'b>, T) -> Fut,
{
    let token_transport = Rc::new(Cell::new(None));
    let future = f(SleepingPill(token_transport.clone()), token, arg);
    let poller = Poller::new(Box::pin(future));

    SleepingFuture {
        token_transport,
        poller,
    }
    .start()
}

#[must_use]
pub struct SleepingFuture<'b, F: Future> {
    token_transport: Rc<Cell<Option<GhostToken<'b>>>>,
    poller: Poller<Pin<Box<F>>>,
}

pub enum FutureStatus<'b, F: Future> {
    Done(F::Output),
    Sleeping(GhostToken<'b>, SleepingFuture<'b, F>),
}
use FutureStatus::*;

impl<'b, F: Future> SleepingFuture<'b, F> {
    pub fn resume(self, token: GhostToken<'b>) -> FutureStatus<'b, F> {
        self.token_transport.set(Some(token));
        self.start()
    }

    fn start(mut self) -> FutureStatus<'b, F> {
        if let Some(res) = self.poller.poll_once() {
            Done(res)
        } else {
            match self.token_transport.take() {
                Some(t) => Sleeping(t, self),
                None => {
                    panic!("Pending on something else than SleepingPill::sleep is not allowed.")
                }
            }
        }
    }
}

pub struct SleepingPill<'b>(Rc<Cell<Option<GhostToken<'b>>>>);

impl<'b> SleepingPill<'b> {
    pub async fn sleep(&self, token: GhostToken<'b>) -> GhostToken<'b> {
        self.0.set(Some(token));
        PendOnce::new().await;
        self.0.take().expect("SleepingPill::sleep(...) was passed to an incompatible executor. You should just await it instead.")
    }
}

struct PendOnce(bool);

impl PendOnce {
    pub fn new() -> Self {
        Self(false)
    }
}

impl Future for PendOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_cell::{GhostCell, GhostToken};

    async fn three_yields<'b>(s: SleepingPill<'b>, t: GhostToken<'b>, _: ()) {
        let t = s.sleep(t).await;
        let t = s.sleep(t).await;
        s.sleep(t).await;
    }

    #[test]
    fn yields_three_times() {
        GhostToken::new(|token| {
            let mut count = 0;
            let mut r = run(three_yields, token, ());
            while let Sleeping(token, program) = r {
                r = program.resume(token);
                count += 1;
            }
            assert_eq!(count, 3);
        });
    }
}
