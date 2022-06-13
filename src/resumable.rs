use crate::poller::Poller;
use ghost_cell::GhostToken;
use pin_project::pin_project;
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

#[derive(Clone)]
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

#[pin_project]
pub struct Join<'a, 'b, A, Af: Future, B, Bf: Future>
where
    A: FnOnce(GhostToken<'b>) -> Af,
    B: FnOnce(GhostToken<'b>) -> Bf,
{
    token_store: &'a Cell<Option<GhostToken<'b>>>,
    futures: JoinFutures<'b, A, Af, B, Bf>,
}

#[pin_project]
enum JoinFutures<'b, A, Af: Future, B, Bf: Future>
where
    A: FnOnce(GhostToken<'b>) -> Af,
    B: FnOnce(GhostToken<'b>) -> Bf,
{
    Unpolled(GhostToken<'b>, A, B),
    Polled(Option<Pin<Box<Af>>>, Option<Pin<Box<Bf>>>),
}
use JoinFutures::*;

impl<'a, 'b, A, Af: Future, B, Bf: Future> Join<'a, 'b, A, Af, B, Bf>
where
    A: FnOnce(GhostToken<'b>) -> Af,
    B: FnOnce(GhostToken<'b>) -> Bf,
{
    pub fn new(token: GhostToken<'b>, pill: &'a SleepingPill<'b>, a: A, b: B) -> Self {
        Self {
            token_store: &pill.0,
            futures: Unpolled(token, a, b),
        }
    }
}

impl<'a, 'b, A, Af: Future<Output = GhostToken<'b>>, B, Bf: Future<Output = GhostToken<'b>>> Future
    for Join<'a, 'b, A, Af, B, Bf>
where
    A: FnOnce(GhostToken<'b>) -> Af,
    B: FnOnce(GhostToken<'b>) -> Bf,
{
    type Output = GhostToken<'b>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let proj = self.project();
        match proj.futures {
            Unpolled(_, _, _) => {
                let (t, a, b) = match std::mem::replace(proj.futures, Polled(None, None)) {
                    Unpolled(t, a, b) => (t, a, b),
                    _ => unreachable!()
                };
                let mut af = Box::pin(a(t));
                let (t, af) = match af.as_mut().poll(cx) {
                    Poll::Ready(t) => (t, None),
                    Poll::Pending => (proj.token_store.take().unwrap(), Some(af)),
                };
                let mut bf = Box::pin(b(t));
                let (t, bf) = match bf.as_mut().poll(cx) {
                    Poll::Ready(t) => (t, None),
                    Poll::Pending => (proj.token_store.take().unwrap(), Some(bf)),
                };
                proj.token_store.set(Some(t));
                *proj.futures = Polled(af, bf);
            }
            Polled(ref mut a, ref mut b) => {
                if let Some(af) = a {
                    match af.as_mut().poll(cx) {
                        Poll::Ready(t) => {
                            *a = None;
                            proj.token_store.set(Some(t));
                        }
                        Poll::Pending => {}
                    };
                }

                if let Some(bf) = b {
                    match bf.as_mut().poll(cx) {
                        Poll::Ready(t) => {
                            *b = None;
                            proj.token_store.set(Some(t));
                        }
                        Poll::Pending => {}
                    };
                }
            }
        }

        match proj.futures {
            Polled(None, None) => Poll::Ready(proj.token_store.take().unwrap()),
            _ => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_cell::GhostToken;

    async fn three_yields<'b>(s: SleepingPill<'b>, t: GhostToken<'b>, _: ()) -> GhostToken<'b> {
        let t = s.sleep(t).await;
        let t = s.sleep(t).await;
        s.sleep(t).await
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

    async fn concurrent<'b>(s: SleepingPill<'b>, t: GhostToken<'b>, _: ()) {
        let ty = |t| three_yields(s.clone(), t, ());
        Join::new(t, &s, ty, ty).await;
    }

    #[test]
    fn join_test() {
        GhostToken::new(|token| {
            let mut count = 0;
            let mut r = run(concurrent, token, ());
            while let Sleeping(token, program) = r {
                r = program.resume(token);
                count += 1;
            }
            assert_eq!(count, 3);
        });
    }
}
