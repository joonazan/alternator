use crate::resumable::{self, SleepingPill};
use ghost_cell::{GhostCell, GhostToken};
use std::future::Future;
use std::rc::Rc;

pub fn run<'b, F, Fut: Future, I, T>(
    f: F,
    token: GhostToken<'b>,
    arg: T,
) -> FutureStatus<'b, Fut, I>
where
    F: FnOnce(InputSource<'b, I>, GhostToken<'b>, T) -> Fut,
{
    let transport = Rc::new(GhostCell::new(None));
    let out = resumable::run(
        |pill, token, arg| {
            f(
                InputSource {
                    pill,
                    input: transport.clone(),
                },
                token,
                arg,
            )
        },
        token,
        arg,
    );
    convert_status(out, transport)
}

pub struct InputSource<'b, I> {
    pill: SleepingPill<'b>,
    input: Rc<GhostCell<'b, Option<I>>>,
}

impl<'b, I> InputSource<'b, I> {
    pub async fn request_input<P: Fn(&I) -> bool>(
        &self,
        mut token: GhostToken<'b>,
        accept: P,
    ) -> (GhostToken<'b>, I) {
        loop {
            token = self.pill.sleep(token).await;
            let input_container = self.input.borrow_mut(&mut token);
            if let Some(i) = input_container.take() {
                if accept(&i) {
                    return (token, i);
                } else {
                    *input_container = Some(i);
                }
            }
        }
    }
}

pub enum FutureStatus<'b, F: Future, I> {
    Done(F::Output),
    AwaitingInput(GhostToken<'b>, InputStarvedFuture<'b, F, I>),
}
use FutureStatus::*;

pub struct InputStarvedFuture<'b, F: Future, I> {
    future: resumable::SleepingFuture<'b, F>,
    input: Rc<GhostCell<'b, Option<I>>>,
}

impl<'b, F: Future, I> InputStarvedFuture<'b, F, I> {
    pub fn resume_with(self, mut token: GhostToken<'b>, input: I) -> FutureStatus<'b, F, I> {
        *self.input.borrow_mut(&mut token) = Some(input);
        convert_status(self.future.resume(token), self.input)
    }
}

fn convert_status<'b, F: Future, I>(
    res: resumable::FutureStatus<'b, F>,
    input: Rc<GhostCell<'b, Option<I>>>,
) -> FutureStatus<'b, F, I> {
    match res {
        resumable::FutureStatus::Done(x) => Done(x),
        resumable::FutureStatus::Sleeping(token, future) => {
            AwaitingInput(token, InputStarvedFuture { future, input })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_cell::GhostToken;

    enum Command {
        Add(i32),
        Mul(i32),
        Stop,
    }
    use Command::*;

    async fn calculator<'b>(
        is: InputSource<'b, Command>,
        mut token: GhostToken<'b>,
        result: &GhostCell<'b, i32>,
    ) {
        loop {
            let (t, cmd) = is.request_input(token, |_| true).await;
            token = t;
            match cmd {
                Add(x) => *result.borrow_mut(&mut token) += x,
                Mul(x) => *result.borrow_mut(&mut token) *= x,
                Stop => break,
            }
        }
    }

    #[test]
    fn calculate() {
        GhostToken::new(|token| {
            let res = GhostCell::new(5);

            let mut state = run(calculator, token, &res);
            state = match state {
                AwaitingInput(t, f) => {
                    assert_eq!(*res.borrow(&t), 5);
                    f.resume_with(t, Mul(3))
                }
                _ => panic!("should be awaiting input"),
            };

            state = match state {
                AwaitingInput(t, f) => {
                    assert_eq!(*res.borrow(&t), 15);
                    f.resume_with(t, Add(1))
                }
                _ => panic!("should be awaiting input"),
            };

            state = match state {
                AwaitingInput(t, f) => {
                    assert_eq!(*res.borrow(&t), 16);
                    f.resume_with(t, Stop)
                }
                _ => panic!("should be awaiting input"),
            };

            match state {
                Done(()) => {}
                _ => panic!("should have stopped"),
            }
            drop(state);

            assert_eq!(res.into_inner(), 16);
        })
    }
}
