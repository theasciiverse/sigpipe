//! Wire messages between the two players of a versus match.
//!
//! Each side runs its own board; nothing is simulated twice. The host picks
//! each round's level and seed (so both boards and both bags are the same),
//! starts the round once both are ready, and each side tells the other its
//! view of its board ten times a second, the junk it sends, and how its round
//! ended. Both work out the round's winner from the two results the same way.
//!
//! Encoding is postcard (serde) inside u32-length-prefixed frames, as in Panzer
//! and Tetrix.

use crate::engine::View;
use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change; the host rejects mismatched guests.
pub const VERSION: u32 = 1;

/// How a round ended for one player.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundEnd {
    pub ok: bool,
    pub piped: u32,
    pub score: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Msg {
    /// Guest's first message.
    Hello { name: String, version: u32 },
    Reject { reason: String },
    /// Host: the host's name, once the guest is in.
    Welcome { name: String },
    /// Host: a new round. `wins` are the host's and the guest's so far.
    Round { round: u32, level: u32, seed: u64, wins: [u32; 2] },
    /// Read the card, ready to play.
    Ready,
    /// Host: both are ready, the countdown starts now.
    Go,
    View(View),
    /// Lock `count` free cells on the receiver's board.
    Junk { count: u32, seed: u64 },
    Done(RoundEnd),
    /// Keeps a quiet connection from timing out.
    Ping,
}

pub fn encode(m: &Msg) -> Vec<u8> {
    postcard::to_allocvec(m).expect("postcard encoding of an owned message cannot fail")
}

pub fn decode(b: &[u8]) -> Option<Msg> {
    postcard::from_bytes(b).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Game, Level};

    #[test]
    fn view_round_trip_and_size() {
        let mut g = Game::new(Level::new(9), 4, 0);
        for _ in 0..8 {
            g.move_cursor(crate::engine::Dir::Right);
            g.place();
        }
        for _ in 0..2000 {
            g.tick(0.02);
        }
        let bytes = encode(&Msg::View(g.view()));
        // sent ten times a second: keep it small
        assert!(bytes.len() < 1500, "view is {} bytes", bytes.len());
        match decode(&bytes) {
            Some(Msg::View(v)) => assert_eq!(v, g.view()),
            other => panic!("decoded {other:?}"),
        }
        assert!(decode(&[0xff, 0xff]).is_none());
    }
}
