//! A versus match over iroh: two players on the same LAN, the same board and
//! the same bag each round, racing side by side, each pipe they lay sending
//! junk to the other (Tetrix's attack model).
//!
//! - **Junk**: every `JUNK_EVERY` segments you pipe, and every loop back
//!   through a crossover, locks a free cell on your rival's board.
//! - **A round** is one level. Passing beats failing, then more segments, then
//!   the higher score; the same on both sides, so both agree. First to
//!   `WINS_NEEDED` takes the match; each round is a level harder.
//!
//! The match state lives here; `main.rs` drives the screens.

use crate::engine::View;
use crate::lan;
use crate::net::{Cmd, Evt, Net};
use crate::proto::{Msg, RoundEnd};
use log::info;
use std::cmp::Ordering;
use std::time::{Duration, Instant};

pub const JUNK_EVERY: u32 = 3;
pub const WINS_NEEDED: u32 = 3;
const VIEW_EVERY: Duration = Duration::from_millis(100);
const PING_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    /// Host, waiting for a rival.
    Hosting,
    /// Guest, picking a match from the LAN.
    Browsing,
    /// Guest, dialling the picked host.
    Joining,
    /// In the match (rounds run on the app's own screens).
    Match,
    /// The connection is gone; `note` says why.
    Gone,
}

pub struct Versus {
    pub net: Option<Net>,
    pub host: bool,
    pub me: String,
    pub rival: Option<String>,
    pub stage: Stage,
    pub note: String,
    listener: Option<lan::Listener>,
    pub found: Vec<lan::Found>,
    pub pick: usize,
    pub round: u32,
    /// Rounds won: mine, theirs.
    pub wins: [u32; 2],
    pub me_ready: bool,
    pub rival_ready: bool,
    pub mine: Option<RoundEnd>,
    pub theirs: Option<RoundEnd>,
    /// Whether this round's winner has been counted.
    pub scored: bool,
    pub rival_view: Option<View>,
    /// Segments piped since the last junk sent.
    pub toward_junk: u32,
    last_view: Instant,
    last_ping: Instant,
}

/// The name shown to the rival: the login name.
pub fn my_name() -> String {
    std::env::var("USER").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "player".into())
}

impl Versus {
    fn new(host: bool, stage: Stage) -> Versus {
        Versus {
            net: None,
            host,
            me: my_name(),
            rival: None,
            stage,
            note: String::new(),
            listener: None,
            found: Vec::new(),
            pick: 0,
            round: 0,
            wins: [0; 2],
            me_ready: false,
            rival_ready: false,
            mine: None,
            theirs: None,
            scored: false,
            rival_view: None,
            toward_junk: 0,
            last_view: Instant::now(),
            last_ping: Instant::now(),
        }
    }

    pub fn host() -> Versus {
        let mut v = Versus::new(true, Stage::Hosting);
        v.net = Some(Net::host(v.me.clone()));
        v.note = "opening the network...".into();
        info!("versus: hosting as {}", v.me);
        v
    }

    pub fn browse() -> Versus {
        let mut v = Versus::new(false, Stage::Browsing);
        match lan::Listener::start(lan::PORT) {
            Ok(l) => v.listener = Some(l),
            Err(e) => v.note = format!("can't listen on the LAN: {e}"),
        }
        info!("versus: browsing as {}", v.me);
        v
    }

    /// Refresh the list of matches on the LAN (guest, browsing).
    pub fn refresh(&mut self) {
        if let Some(l) = &self.listener {
            self.found = l.hosts();
            self.pick = self.pick.min(self.found.len().saturating_sub(1));
        }
    }

    pub fn join_picked(&mut self) {
        let Some(f) = self.found.get(self.pick).cloned() else { return };
        info!("versus: joining {} at {}", f.beacon.name, f.from);
        self.net = Some(Net::join(f.addr(), self.me.clone()));
        self.rival = Some(f.beacon.name.clone());
        self.listener = None;
        self.stage = Stage::Joining;
        self.note = format!("dialling {}...", f.beacon.name);
    }

    pub fn send(&mut self, m: Msg) {
        if let Some(n) = &self.net {
            n.send(Cmd::Broadcast(m));
        }
    }

    pub fn poll(&mut self) -> Option<Evt> {
        self.net.as_ref()?.poll()
    }

    pub fn rival_name(&self) -> &str {
        self.rival.as_deref().unwrap_or("rival")
    }

    /// Reset for a new round.
    pub fn new_round(&mut self, round: u32) {
        self.round = round;
        self.me_ready = false;
        self.rival_ready = false;
        self.mine = None;
        self.theirs = None;
        self.scored = false;
        self.rival_view = None;
        self.toward_junk = 0;
    }

    /// Count the round once both results are in. `Greater`: mine.
    pub fn settle_round(&mut self) -> Option<Ordering> {
        let (Some(m), Some(t)) = (self.mine, self.theirs) else { return None };
        let o = verdict(m, t);
        if !self.scored {
            self.scored = true;
            match o {
                Ordering::Greater => self.wins[0] += 1,
                Ordering::Less => self.wins[1] += 1,
                Ordering::Equal => {}
            }
            info!("versus: round {} {:?}, wins {:?}", self.round, o, self.wins);
        }
        Some(o)
    }

    pub fn match_over(&self) -> bool {
        self.wins.iter().any(|w| *w >= WINS_NEEDED)
    }

    /// Send the board's view now and then, or a ping, so the link stays warm.
    pub fn keep_up(&mut self, view: Option<View>) {
        let now = Instant::now();
        if let Some(v) = view
            && now.duration_since(self.last_view) >= VIEW_EVERY
        {
            self.last_view = now;
            self.last_ping = now;
            self.send(Msg::View(v));
        } else if now.duration_since(self.last_ping) >= PING_EVERY {
            self.last_ping = now;
            self.send(Msg::Ping);
        }
    }
}

/// Who took the round, mine against theirs: passing beats failing, then more
/// segments, then the higher score.
pub fn verdict(mine: RoundEnd, theirs: RoundEnd) -> Ordering {
    mine.ok.cmp(&theirs.ok).then(mine.piped.cmp(&theirs.piped)).then(mine.score.cmp(&theirs.score))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(ok: bool, piped: u32, score: i64) -> RoundEnd {
        RoundEnd { ok, piped, score }
    }

    #[test]
    fn verdict_is_the_same_from_both_sides() {
        let cases = [
            (end(true, 8, 800), end(false, 20, 2000)),
            (end(true, 9, 800), end(true, 8, 5000)),
            (end(false, 5, 600), end(false, 5, 500)),
            (end(true, 8, 800), end(true, 8, 800)),
        ];
        for (a, b) in cases {
            assert_eq!(verdict(a, b), verdict(b, a).reverse());
        }
        assert_eq!(verdict(cases[0].0, cases[0].1), Ordering::Greater, "passing beats a longer fail");
        assert_eq!(verdict(cases[3].0, cases[3].1), Ordering::Equal);
    }

    #[test]
    fn a_round_counts_once() {
        let mut v = Versus::new(true, Stage::Match);
        v.mine = Some(end(true, 8, 800));
        v.theirs = Some(end(false, 3, 300));
        v.settle_round();
        v.settle_round();
        assert_eq!(v.wins, [1, 0]);
    }
}
