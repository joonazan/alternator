use crate::poller::Poller;
use pin_project::pin_project;
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

pub fn run<Token, F, Fut: Future, T>(f: F, token: Token, arg: T) -> FutureStatus<Token, Fut>
where
    F: FnOnce(SleepingPill<Token>, Token, T) -> Fut,
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
pub struct SleepingFuture<Token, F: Future> {
    token_transport: Rc<Cell<Option<Token>>>,
    poller: Poller<Pin<Box<F>>>,
}

pub enum FutureStatus<Token, F: Future> {
    Done(F::Output),
    Sleeping(Token, SleepingFuture<Token, F>),
}
use FutureStatus::*;

impl<Token, F: Future> SleepingFuture<Token, F> {
    pub fn resume(self, token: Token) -> FutureStatus<Token, F> {
        self.token_transport.set(Some(token));
        self.start()
    }

    fn start(mut self) -> FutureStatus<Token, F> {
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

pub struct SleepingPill<Token>(Rc<Cell<Option<Token>>>);

impl<Token> SleepingPill<Token> {
    pub async fn sleep(&self, token: Token) -> Token {
        self.0.set(Some(token));
        PendOnce::new().await;
        self.0.take().expect("SleepingPill::sleep(...) was passed to an incompatible executor. You should just await it instead.")
    }
}

impl<Token> Clone for SleepingPill<Token> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
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
pub struct Join<'a, Token, A, Af: Future, B, Bf: Future>
where
    A: FnOnce(Token) -> Af,
    B: FnOnce(Token) -> Bf,
{
    token_store: &'a Cell<Option<Token>>,
    futures: JoinFutures<Token, A, Af, B, Bf>,
}

#[pin_project]
enum JoinFutures<Token, A, Af: Future, B, Bf: Future>
where
    A: FnOnce(Token) -> Af,
    B: FnOnce(Token) -> Bf,
{
    Unpolled(Token, A, B),
    Polled(Option<Pin<Box<Af>>>, Option<Pin<Box<Bf>>>),
}
use JoinFutures::*;

impl<'a, Token, A, Af: Future, B, Bf: Future> Join<'a, Token, A, Af, B, Bf>
where
    A: FnOnce(Token) -> Af,
    B: FnOnce(Token) -> Bf,
{
    pub fn new(token: Token, pill: &'a SleepingPill<Token>, a: A, b: B) -> Self {
        Self {
            token_store: &pill.0,
            futures: Unpolled(token, a, b),
        }
    }
}

impl<'a, Token, A, Af: Future<Output = Token>, B, Bf: Future<Output = Token>> Future
    for Join<'a, Token, A, Af, B, Bf>
where
    A: FnOnce(Token) -> Af,
    B: FnOnce(Token) -> Bf,
{
    type Output = Token;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let proj = self.project();
        match proj.futures {
            Unpolled(_, _, _) => {
                let (t, a, b) = match std::mem::replace(proj.futures, Polled(None, None)) {
                    Unpolled(t, a, b) => (t, a, b),
                    _ => unreachable!(),
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

    async fn three_yields<'b>(
        s: SleepingPill<GhostToken<'b>>,
        t: GhostToken<'b>,
        _: (),
    ) -> GhostToken<'b> {
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

    async fn concurrent<'b>(s: SleepingPill<GhostToken<'b>>, t: GhostToken<'b>, _: ()) {
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
