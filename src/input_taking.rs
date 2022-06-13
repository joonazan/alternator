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
    pill: SleepingPill<GhostToken<'b>>,
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
    future: resumable::SleepingFuture<GhostToken<'b>, F>,
    input: Rc<GhostCell<'b, Option<I>>>,
}

impl<'b, F: Future, I> InputStarvedFuture<'b, F, I> {
    pub fn resume_with(self, mut token: GhostToken<'b>, input: I) -> FutureStatus<'b, F, I> {
        *self.input.borrow_mut(&mut token) = Some(input);
        convert_status(self.future.resume(token), self.input)
    }
}

fn convert_status<'b, F: Future, I>(
    res: resumable::FutureStatus<GhostToken<'b>, F>,
    input: Rc<GhostCell<'b, Option<I>>>,
) -> FutureStatus<'b, F, I> {
    match res {
        resumable::FutureStatus::Done(x) => Done(x),
        resumable::FutureStatus::Sleeping(token, future) => {
            AwaitingInput(token, InputStarvedFuture { future, input })
        }
    }
}

pub fn join<'a, 'b, A, Af: Future, B, Bf: Future, I>(
    token: GhostToken<'b>,
    input: &'a InputSource<'b, I>,
    a: A,
    b: B,
) -> resumable::Join<'a, GhostToken<'b>, A, Af, B, Bf>
where
    A: FnOnce(GhostToken<'b>) -> Af,
    B: FnOnce(GhostToken<'b>) -> Bf,
{
    resumable::Join::new(token, &input.pill, a, b)
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

    #[derive(Debug, PartialEq)]
    enum Move {
        Fireball,
        Kick,
    }
    use Move::*;

    #[derive(Debug, PartialEq)]
    enum Action {
        ChooseMove(Move),
        ChooseEnergy(u16),
    }
    use Action::*;

    async fn turn<'a, 'b>(
        is: InputSource<'b, (usize, Action)>,
        token: GhostToken<'b>,
        board: &'a GhostCell<'b, [(Option<Move>, Option<u16>); 2]>,
    ) -> GhostToken<'b> {
        join(
            token,
            &is,
            |t| player_turn(0, t, &is, board),
            |t| player_turn(1, t, &is, board),
        )
        .await
    }

    async fn player_turn<'a, 'b>(
        player: usize,
        mut token: GhostToken<'b>,
        is: &InputSource<'b, (usize, Action)>,
        board: &'a GhostCell<'b, [(Option<Move>, Option<u16>); 2]>,
    ) -> GhostToken<'b> {
        loop {
            let (t, action) = is.request_input(token, |(p, _)| *p == player).await;
            token = t;
            let plan = &mut board.borrow_mut(&mut token)[player];
            match action.1 {
                ChooseMove(m) => plan.0 = Some(m),
                ChooseEnergy(e) => plan.1 = Some(e),
            };
            if plan.0.is_some() && plan.1.is_some() {
                break;
            }
        }
        token
    }

    #[test]
    fn concurrent_turns() {
        GhostToken::new(|token| {
            let res = GhostCell::new([(None, None), (None, None)]);

            let mut state = run(turn, token, &res);
            for input in [
                (1, ChooseMove(Fireball)),
                (0, ChooseEnergy(34)),
                (0, ChooseMove(Kick)),
                (1, ChooseEnergy(4)),
            ] {
                state = match state {
                    AwaitingInput(t, f) => f.resume_with(t, input),
                    _ => panic!("should be awaiting input"),
                };
            }

            match state {
                Done(t) => {
                    assert_eq!(
                        res.borrow(&t),
                        &[(Some(Kick), Some(34)), (Some(Fireball), Some(4))]
                    );
                }
                _ => panic!("should have stopped"),
            }
        });
    }
}
