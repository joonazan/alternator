use crate::resumable::{self, SleepingPill};
use futures::future::FutureExt;
use std::future::Future;

pub fn run<W, F, Fut: Future, I, InputHint>(f: F, world: W) -> FutureStatus<W, Fut, I, InputHint>
where
    F: FnOnce(InputSource<W, I, InputHint>, W) -> Fut,
{
    let out = resumable::run(
        |pill, (world, _, _), _| f(InputSource { pill }, world),
        (world, None, None),
        (),
    );
    convert_status(out)
}

pub struct InputSource<W, I, InputHint = ()> {
    pill: SleepingPill<(W, Option<I>, Option<InputHint>)>,
}

impl<W, I: Clone, InputHint: Clone> InputSource<W, I, InputHint> {
    pub async fn request_input<II, F>(&self, mut world: W, hint: InputHint, filter: F) -> (W, II)
    where
        F: Fn(I, &W) -> Option<II>,
    {
        let mut input = None;
        loop {
            (world, input, _) = self.pill.sleep((world, input, Some(hint.clone()))).await;
            if let Some(i) = input {
                if let Some(part) = filter(i.clone(), &world) {
                    return (world, part);
                }
                input = Some(i);
            }
        }
    }
}

impl<W, I: Clone> InputSource<W, I> {
    pub async fn get_input<II, F: Fn(I, &W) -> Option<II>>(&self, world: W, filter: F) -> (W, II) {
        self.request_input(world, (), filter).await
    }
}

pub enum FutureStatus<W, F: Future, I, InputHint> {
    Done(F::Output),
    AwaitingInput {
        world: W,
        accepted: bool,
        future: InputStarvedFuture<W, F, I, InputHint>,
        input_hint: InputHint,
    },
}
use FutureStatus::*;

pub struct InputStarvedFuture<W, F: Future, I, InputHint> {
    future: resumable::SleepingFuture<(W, Option<I>, Option<InputHint>), F>,
}

impl<W, F: Future, I, InputHint> InputStarvedFuture<W, F, I, InputHint> {
    pub fn resume_with(self, world: W, input: I) -> FutureStatus<W, F, I, InputHint> {
        convert_status(self.future.resume((world, Some(input), None)))
    }
}

fn convert_status<W, F: Future, I, InputHint>(
    res: resumable::FutureStatus<(W, Option<I>, Option<InputHint>), F>,
) -> FutureStatus<W, F, I, InputHint> {
    match res {
        resumable::FutureStatus::Done(x) => Done(x),
        resumable::FutureStatus::Sleeping((world, input, input_hint), future) => AwaitingInput {
            world,
            accepted: input.is_none(),
            future: InputStarvedFuture { future },
            input_hint: input_hint.unwrap(),
        },
    }
}

pub fn join<
    'is,
    W,
    A,
    Af: Future<Output = W> + 'is,
    B,
    Bf: Future<Output = W> + 'is,
    I,
    InputHint,
>(
    world: W,
    input: &'is InputSource<W, I, InputHint>,
    a: A,
    b: B,
) -> impl Future<Output = W> + 'is
where
    A: FnOnce(W) -> Af + 'is,
    B: FnOnce(W) -> Bf + 'is,
{
    // Discarding the input at the end is fine; it must None
    // because the only way to finish a future is making progress
    // and that requires consuming an input.
    resumable::Join::new(
        (world, None, None),
        &input.pill,
        |(w, _, _)| a(w).map(|w| (w, None, None)),
        |(w, _, _)| b(w).map(|w| (w, None, None)),
    )
    .map(|(w, _, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    enum Command {
        Add(i32),
        Mul(i32),
        Stop,
    }
    use Command::*;

    async fn calculator<'a>(is: InputSource<&'a mut i32, Command>, mut result: &'a mut i32) {
        loop {
            let (w, cmd) = is.get_input(result, |i, _| Some(i)).await;
            result = w;
            match cmd {
                Add(x) => *result += x,
                Mul(x) => *result *= x,
                Stop => break,
            }
        }
    }

    #[test]
    fn calculate() {
        let mut res = 5;

        let mut state = run(calculator, &mut res);
        state = match state {
            AwaitingInput {
                world: t, future, ..
            } => {
                assert_eq!(*t, 5);
                future.resume_with(t, Mul(3))
            }
            _ => panic!("should be awaiting input"),
        };

        state = match state {
            AwaitingInput {
                world: t, future, ..
            } => {
                assert_eq!(*t, 15);
                future.resume_with(t, Add(1))
            }
            _ => panic!("should be awaiting input"),
        };

        state = match state {
            AwaitingInput {
                world: t, future, ..
            } => {
                assert_eq!(*t, 16);
                future.resume_with(t, Stop)
            }
            _ => panic!("should be awaiting input"),
        };

        match state {
            Done(()) => {}
            _ => panic!("should have stopped"),
        }
        drop(state);

        assert_eq!(res, 16);
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Move {
        Fireball,
        Kick,
    }
    use Move::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Action {
        ChooseMove(Move),
        ChooseEnergy(u16),
    }
    use Action::*;

    type Board = [(Option<Move>, Option<u16>); 2];

    async fn turn<'a>(
        is: InputSource<&'a mut Board, (usize, Action)>,
        board: &'a mut Board,
    ) -> &'a mut Board {
        join(
            board,
            &is,
            |t| player_turn(0, &is, t),
            |t| player_turn(1, &is, t),
        )
        .await
    }

    async fn player_turn<'a>(
        player: usize,
        is: &InputSource<&'a mut Board, (usize, Action)>,
        mut board: &'a mut Board,
    ) -> &'a mut Board {
        loop {
            let (b, action) = is
                .get_input(board, |(p, a), _| if p == player { Some(a) } else { None })
                .await;
            board = b;
            let plan = &mut board[player];
            match action {
                ChooseMove(m) => plan.0 = Some(m),
                ChooseEnergy(e) => plan.1 = Some(e),
            };
            if plan.0.is_some() && plan.1.is_some() {
                break;
            }
        }
        board
    }

    #[test]
    fn concurrent_turns() {
        let mut board = [(None, None), (None, None)];

        let mut state = run(turn, &mut board);
        for input in [
            (1, ChooseMove(Fireball)),
            (0, ChooseEnergy(34)),
            (0, ChooseMove(Kick)),
            (1, ChooseEnergy(4)),
        ] {
            state = match state {
                AwaitingInput {
                    world: b, future, ..
                } => future.resume_with(b, input),
                _ => panic!("should be awaiting input"),
            };
        }

        match state {
            Done(b) => {
                assert_eq!(*b, [(Some(Kick), Some(34)), (Some(Fireball), Some(4))]);
            }
            _ => panic!("should have stopped"),
        }
    }

    #[test]
    fn test_rejection() {
        let mut board = [(None, None), (None, None)];
        let mut state = run(turn, &mut board);
        state = match state {
            AwaitingInput {
                world: b, future, ..
            } => future.resume_with(b, (7, ChooseEnergy(4))),
            _ => panic!("should be awaiting input"),
        };
        match state {
            AwaitingInput {
                accepted: false, ..
            } => {}
            _ => panic!("should have rejected the input"),
        }
    }
}
