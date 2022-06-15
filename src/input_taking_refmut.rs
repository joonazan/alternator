use crate::resumable::{self, SleepingPill};
use futures::future::FutureExt;
use std::future::Future;

pub fn run<W, F, Fut: Future, I>(f: F, world: W) -> FutureStatus<W, Fut, I>
where
    F: FnOnce(InputSource<W, I>, W) -> Fut,
{
    let out = resumable::run(
        |pill, (world, _), _| f(InputSource { pill }, world),
        (world, None),
        (),
    );
    convert_status(out)
}

pub struct InputSource<W, I> {
    pill: SleepingPill<(W, Option<I>)>,
}

impl<W, I> InputSource<W, I> {
    pub async fn request_input<P: Fn(&I) -> bool>(&self, mut world: W, accept: P) -> (W, I) {
        let mut input = None;
        loop {
            (world, input) = self.pill.sleep((world, input)).await;
            if let Some(i) = input {
                if accept(&i) {
                    return (world, i);
                }
                input = Some(i);
            }
        }
    }
}

pub enum FutureStatus<W, F: Future, I> {
    Done(F::Output),
    AwaitingInput(W, InputStarvedFuture<W, F, I>),
}
use FutureStatus::*;

pub struct InputStarvedFuture<W, F: Future, I> {
    future: resumable::SleepingFuture<(W, Option<I>), F>,
}

impl<W, F: Future, I> InputStarvedFuture<W, F, I> {
    pub fn resume_with(self, world: W, input: I) -> FutureStatus<W, F, I> {
        convert_status(self.future.resume((world, Some(input))))
    }
}

fn convert_status<W, F: Future, I>(
    res: resumable::FutureStatus<(W, Option<I>), F>,
) -> FutureStatus<W, F, I> {
    match res {
        resumable::FutureStatus::Done(x) => Done(x),
        resumable::FutureStatus::Sleeping((world, _), future) => {
            AwaitingInput(world, InputStarvedFuture { future })
        }
    }
}

pub fn join<'is, W, A, Af: Future<Output = W> + 'is, B, Bf: Future<Output = W> + 'is, I>(
    world: W,
    input: &'is InputSource<W, I>,
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
        (world, None),
        &input.pill,
        |(w, _)| a(w).map(|w| (w, None)),
        |(w, _)| b(w).map(|w| (w, None)),
    )
    .map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Command {
        Add(i32),
        Mul(i32),
        Stop,
    }
    use Command::*;

    async fn calculator<'a>(is: InputSource<&'a mut i32, Command>, mut result: &'a mut i32) {
        loop {
            let (w, cmd) = is.request_input(result, |_| true).await;
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
            AwaitingInput(t, f) => {
                assert_eq!(*t, 5);
                f.resume_with(t, Mul(3))
            }
            _ => panic!("should be awaiting input"),
        };

        state = match state {
            AwaitingInput(t, f) => {
                assert_eq!(*t, 15);
                f.resume_with(t, Add(1))
            }
            _ => panic!("should be awaiting input"),
        };

        state = match state {
            AwaitingInput(t, f) => {
                assert_eq!(*t, 16);
                f.resume_with(t, Stop)
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
            let (b, action) = is.request_input(board, |(p, _)| *p == player).await;
            board = b;
            let plan = &mut board[player];
            match action.1 {
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
                AwaitingInput(b, f) => f.resume_with(b, input),
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
}
