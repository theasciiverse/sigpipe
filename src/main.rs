//! SIGPIPE: lay pipe from the queue before the data arrives, and keep it flowing.
//! A spiritual successor to the 1989 pipe-laying classic; mechanics after it,
//! everything you see our own.
//!
//! Run: `cargo run --release [-- --level N] [--mute]` (`RUST_LOG=debug cargo run --release
//! 2>sigpipe.log` for a session log). Without a terminal: `cargo run -- --shot SECS
//! [--level N] [--seed S] [--moves ..] [--size 80x24] [--screen title]` plays SECS
//! seconds and prints the screen as text (`--cells` for `tools/shot.py`, which
//! draws it as a PNG). Moves: `U D L R` move the cursor, `P`
//! lays a piece, `F` fast flow, `.` waits a tenth of a second; `--deal "+7J"`
//! deals those pieces first (`- | L J F 7 +`, the corners by their shape).

mod audio;
mod engine;
mod input;
mod lan;
mod net;
mod proto;
mod render;
mod versus;

use audio::{Audio, Sfx};
use engine::{Event, Game, Level, Phase};
use input::{Act, Input};
use log::{debug, info};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::Style;
use net::Evt;
use proto::{Msg, RoundEnd};
use render::{bold, style, Hud, Lines, AMBER, DIM, GREEN, RED, TEXT};
use std::cmp::Ordering;
use versus::{Stage, Versus};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Title,
    /// The level's card over the board, before the countdown.
    Card,
    Play,
    Paused,
    /// The verdict over the board.
    Result,
    /// Versus: hosting, browsing the LAN, or dialling.
    Lobby,
}

struct App {
    screen: Screen,
    game: Game,
    hi: i64,
    last: Option<i64>,
    /// Seconds since start, for animation.
    t: f32,
    /// Whether the mouse or keys drive the cursor (the mouse hides it when idle).
    cursor: bool,
    quit: bool,
    area: Rect,
    audio: Audio,
    /// Segments heard this level, for the climbing notes.
    heard: u32,
    /// A versus match, from its lobby on.
    vs: Option<Versus>,
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

fn hi_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("sigpipe").join("hiscore"))
}

fn load_hi() -> i64 {
    hi_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn save_hi(hi: i64) {
    let Some(p) = hi_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&p, format!("{hi}\n")) {
        log::warn!("could not save the hi score to {}: {e}", p.display());
    }
}

impl App {
    fn new(level: u32, hi: i64, audio: Audio) -> App {
        App {
            screen: Screen::Title,
            game: Game::new(Level::new(level), seed(), 0),
            hi,
            last: None,
            t: 0.0,
            cursor: true,
            quit: false,
            area: Rect::new(0, 0, 80, 24),
            audio,
            heard: 0,
            vs: None,
        }
    }

    fn start_level(&mut self, n: u32, score: i64) {
        let s = seed();
        self.game = Game::new(Level::new(n), s, score);
        self.screen = Screen::Card;
        self.heard = 0;
        info!("level {n} (seed {s}): {:?}", self.game.level);
    }

    /// Back to the title, keeping the run's score.
    fn to_title(&mut self) {
        let score = self.game.score;
        self.last = Some(score);
        if score > self.hi {
            self.hi = score;
            save_hi(score);
            info!("new hi score {score}");
        }
        self.screen = Screen::Title;
        info!("to title, score {score}");
    }

    fn act(&mut self, a: Act) {
        if a != Act::Quit && !matches!(a, Act::Point { click: false, .. }) {
            debug!("act {a:?} on {:?}", self.screen);
        }
        if self.vs.is_some() && self.versus_act(a) {
            return;
        }
        match (self.screen, a) {
            (_, Act::Mute) => {
                self.audio.toggle_mute();
                info!("muted {}", self.audio.muted);
            }
            (_, Act::Quit) => {
                if self.vs.is_some() {
                    self.leave();
                } else if self.screen != Screen::Title {
                    self.to_title();
                }
                self.quit = true;
            }
            (Screen::Title, Act::Ok) => {
                self.audio.play(Sfx::Select);
                self.start_level(1, 0);
            }
            (Screen::Title, Act::Back) => self.quit = true,
            (Screen::Title, Act::Host) => {
                self.audio.play(Sfx::Select);
                self.vs = Some(Versus::host());
                self.screen = Screen::Lobby;
            }
            (Screen::Title, Act::Join) => {
                self.audio.play(Sfx::Select);
                self.vs = Some(Versus::browse());
                self.screen = Screen::Lobby;
            }
            (Screen::Card, Act::Ok | Act::Point { click: true, .. }) => {
                self.audio.play(Sfx::Select);
                self.screen = Screen::Play;
                info!("countdown starts");
            }
            (Screen::Card, Act::Back) => self.to_title(),
            (Screen::Play, Act::Move(d)) => {
                self.cursor = true;
                self.game.move_cursor(d);
            }
            (Screen::Play, Act::Ok) => {
                self.cursor = true;
                self.game.place();
            }
            (Screen::Play, Act::Fast) => self.game.toggle_fast(),
            (Screen::Play, Act::Pause | Act::Back) => {
                self.screen = Screen::Paused;
                info!("stopped");
            }
            (Screen::Play, Act::Point { x, y, click }) => {
                if let Some(pos) = render::tile_at(&render::layout(self.my_area()), x, y) {
                    self.cursor = true;
                    self.game.cursor = pos;
                    if click {
                        self.game.place();
                    }
                }
            }
            (Screen::Paused, Act::Pause | Act::Ok) => {
                self.screen = Screen::Play;
                info!("fg");
            }
            (Screen::Paused, Act::Back) => self.to_title(),
            (Screen::Result, Act::Ok | Act::Point { click: true, .. }) => match self.game.phase {
                Phase::Done { ok: true, .. } => self.start_level(self.game.level.n + 1, self.game.score),
                _ => self.to_title(),
            },
            (Screen::Result, Act::Back) => self.to_title(),
            _ => {}
        }
    }

    fn update(&mut self, dt: f32) {
        self.t += dt;
        self.versus_update();
        let before = self.game.phase;
        if self.screen == Screen::Play {
            self.game.tick(dt);
        }
        // the last three seconds of the countdown tick
        if let (Phase::Countdown { left: a }, Phase::Countdown { left: b }) = (before, self.game.phase)
            && a.ceil() != b.ceil()
            && b.ceil() <= 3.0
        {
            self.audio.play(Sfx::Tick);
        }
        for e in std::mem::take(&mut self.game.events) {
            match e {
                Event::Segment { .. } | Event::Placed { .. } | Event::Denied { .. } => debug!("{e:?}"),
                _ => info!("{e:?}"),
            }
            self.sound(e);
            self.versus_event(e);
            if let Event::Done { .. } = e {
                self.screen = Screen::Result;
            }
        }
        let view = matches!(self.screen, Screen::Play | Screen::Result).then(|| self.game.view());
        if let Some(vs) = &mut self.vs
            && vs.stage == Stage::Match
        {
            vs.keep_up(view);
        }
        let flowing = self.screen == Screen::Play && self.game.phase == Phase::Flowing;
        let rate = if flowing { 1.0 / self.game.segment_time() } else { 0.0 };
        self.audio.set_stream(rate, if flowing { self.game.fronts() } else { 0 });
    }

    fn sound(&mut self, e: Event) {
        let a = &mut self.audio;
        match e {
            Event::Placed { replaced, .. } => a.play(if replaced { Sfx::Replace } else { Sfx::Place }),
            Event::Denied { .. } => a.play(Sfx::Denied),
            Event::FlowStarts => a.play(Sfx::Start),
            Event::Fast(true) => a.play(Sfx::Fast),
            Event::Fast(false) => {}
            Event::Segment { looped, buffer, .. } => {
                self.heard += 1;
                a.segment(self.heard);
                if self.heard == self.game.level.required {
                    a.play(Sfx::Met);
                }
                if looped {
                    let five = self.game.crosses == engine::LOOPS_FOR_BONUS;
                    a.play(if five { Sfx::LoopsBonus } else { Sfx::Loop });
                }
                if buffer {
                    a.play(Sfx::Buffer);
                }
            }
            Event::Sink => a.play(Sfx::Sink),
            Event::Spill { .. } => a.play(Sfx::Spill),
            Event::Done { ok, .. } => a.play(if ok { Sfx::Pass } else { Sfx::Fail }),
        }
    }

    // ------------------------------------------------------------- versus

    /// The part of the screen with my board: the left half when the rival's
    /// board fits beside it.
    fn my_area(&self) -> Rect {
        let a = self.area;
        if self.vs.as_ref().is_some_and(|v| v.stage == Stage::Match) && a.width >= 2 * render::NEED_W + 2 {
            Rect::new(a.x, a.y, a.width / 2, a.height)
        } else {
            a
        }
    }

    /// Handle what a match handles differently; `false` passes `a` on to the
    /// solo handling (moving, laying, fast flow, the mouse).
    fn versus_act(&mut self, a: Act) -> bool {
        let Some(vs) = self.vs.as_mut() else { return false };
        match (self.screen, a) {
            (_, Act::Mute | Act::Quit) => false,
            (Screen::Lobby, Act::Move(engine::Dir::Up)) => {
                vs.pick = vs.pick.saturating_sub(1);
                true
            }
            (Screen::Lobby, Act::Move(engine::Dir::Down)) => {
                vs.pick = (vs.pick + 1).min(vs.found.len().saturating_sub(1));
                true
            }
            (Screen::Lobby, Act::Ok) => {
                if vs.stage == Stage::Browsing {
                    vs.join_picked();
                } else if vs.stage == Stage::Gone {
                    self.leave();
                }
                true
            }
            (Screen::Lobby, Act::Back) => {
                self.leave();
                true
            }
            (Screen::Lobby, _) => true,
            (Screen::Card, Act::Ok | Act::Point { click: true, .. }) => {
                if vs.stage == Stage::Gone {
                    self.leave();
                } else if !vs.me_ready {
                    vs.me_ready = true;
                    vs.send(Msg::Ready);
                    self.audio.play(Sfx::Select);
                    if vs.host && vs.rival_ready {
                        self.go();
                    }
                }
                true
            }
            (Screen::Card | Screen::Play | Screen::Result, Act::Back) => {
                self.leave();
                true
            }
            // no pausing a race
            (Screen::Play, Act::Pause) => true,
            (Screen::Result, Act::Ok | Act::Point { click: true, .. }) => {
                if vs.stage == Stage::Gone || vs.match_over() {
                    self.leave();
                } else if vs.host && vs.settle_round().is_some() {
                    self.host_next_round();
                }
                true
            }
            _ => false,
        }
    }

    /// Out of the match, back to the title; dropping the network closes it.
    fn leave(&mut self) {
        if let Some(vs) = self.vs.take() {
            info!("versus: leaving, wins {:?}", vs.wins);
        }
        self.screen = Screen::Title;
        self.audio.set_stream(0.0, 0);
    }

    fn begin_round(&mut self, round: u32, level: u32, seed: u64) {
        if let Some(vs) = &mut self.vs {
            vs.new_round(round);
        }
        self.game = Game::new(Level::new(level), seed, 0);
        self.heard = 0;
        self.screen = Screen::Card;
        info!("versus: round {round}, level {level}, seed {seed}");
    }

    fn host_next_round(&mut self) {
        let Some(vs) = self.vs.as_mut() else { return };
        let round = vs.round + 1;
        let s = seed();
        vs.send(Msg::Round { round, level: round, seed: s, wins: vs.wins });
        self.begin_round(round, round, s);
    }

    /// Host: both ready, start the round on both sides.
    fn go(&mut self) {
        if let Some(vs) = &mut self.vs {
            vs.send(Msg::Go);
        }
        self.screen = Screen::Play;
        info!("versus: go");
    }

    fn versus_update(&mut self) {
        let Some(vs) = self.vs.as_mut() else { return };
        if vs.stage == Stage::Browsing {
            vs.refresh();
        }
        let mut evts = Vec::new();
        while let Some(e) = vs.poll() {
            evts.push(e);
        }
        for e in evts {
            self.on_net(e);
        }
    }

    fn on_net(&mut self, e: Evt) {
        match &e {
            Evt::ClientMsg { msg: Msg::View(_) | Msg::Ping } | Evt::HostMsg(Msg::View(_) | Msg::Ping) => {}
            _ => info!("net {e:?}"),
        }
        let Some(vs) = self.vs.as_mut() else { return };
        match e {
            Evt::Hosting => vs.note = "waiting for a rival on the LAN...".into(),
            Evt::Connected => vs.note = "connected, waiting for the host...".into(),
            Evt::Failed(why) => {
                vs.stage = Stage::Gone;
                vs.note = why;
            }
            Evt::ClientJoined { name } => {
                vs.rival = Some(name);
                vs.stage = Stage::Match;
                if let Some(n) = &vs.net {
                    n.send(net::Cmd::Beacon { players: 2, in_game: true });
                }
                let me = vs.me.clone();
                vs.send(Msg::Welcome { name: me });
                self.host_next_round();
            }
            Evt::ClientMsg { msg } | Evt::HostMsg(msg) => self.on_msg(msg),
            Evt::ClientLeft | Evt::Disconnected => {
                if vs.stage != Stage::Gone {
                    vs.note = format!("{} left the match", vs.rival_name());
                    vs.stage = Stage::Gone;
                }
                if matches!(self.screen, Screen::Card | Screen::Play) {
                    self.screen = Screen::Result;
                }
            }
        }
    }

    fn on_msg(&mut self, m: Msg) {
        let Some(vs) = self.vs.as_mut() else { return };
        match m {
            Msg::Welcome { name } => {
                vs.rival = Some(name);
                vs.stage = Stage::Match;
            }
            Msg::Reject { reason } => {
                vs.stage = Stage::Gone;
                vs.note = reason;
            }
            Msg::Round { round, level, seed, wins } => {
                vs.wins = [wins[1], wins[0]];
                vs.stage = Stage::Match;
                self.begin_round(round, level, seed);
            }
            Msg::Ready => {
                vs.rival_ready = true;
                if vs.host && vs.me_ready && self.screen == Screen::Card {
                    self.go();
                }
            }
            Msg::Go => {
                if self.screen == Screen::Card {
                    self.screen = Screen::Play;
                }
            }
            Msg::View(v) => vs.rival_view = Some(v),
            Msg::Junk { count, seed } => {
                if self.screen == Screen::Play && !self.game.over() {
                    let hit = self.game.junk(count, seed);
                    info!("versus: junk landed on {hit:?}");
                    if !hit.is_empty() {
                        self.audio.play(Sfx::Junk);
                    }
                }
            }
            Msg::Done(end) => {
                vs.theirs = Some(end);
                vs.settle_round();
            }
            Msg::Hello { .. } | Msg::Ping => {}
        }
    }

    /// My board's events, as they concern the rival: junk and the round's end.
    fn versus_event(&mut self, e: Event) {
        let Some(vs) = self.vs.as_mut() else { return };
        match e {
            Event::Segment { looped, .. } => {
                vs.toward_junk += 1;
                let mut count = 0;
                if vs.toward_junk >= versus::JUNK_EVERY {
                    vs.toward_junk = 0;
                    count += 1;
                }
                if looped {
                    count += 1;
                }
                if count > 0 {
                    vs.send(Msg::Junk { count, seed: seed() });
                }
            }
            Event::Done { ok, .. } => {
                let end = RoundEnd { ok, piped: self.game.piped, score: self.game.score };
                vs.mine = Some(end);
                vs.send(Msg::Done(end));
                vs.settle_round();
            }
            _ => {}
        }
    }

    fn lobby_lines(&self) -> Lines {
        let Some(vs) = &self.vs else { return Vec::new() };
        let line = |s: &str, st: Style| (s.to_string(), st);
        let mut l = Vec::new();
        match vs.stage {
            Stage::Hosting => {
                l.push(line("host a versus match", bold(GREEN)));
                l.push(line("", style(TEXT)));
                l.push(line(&vs.note, style(TEXT)));
                l.push(line(&format!("others see it as \"{}\" under C", vs.me), style(DIM)));
            }
            Stage::Browsing => {
                l.push(line("versus matches on the LAN", bold(GREEN)));
                l.push(line("", style(TEXT)));
                if vs.found.is_empty() {
                    l.push(line("searching...", style(DIM)));
                    l.push(line("(someone presses V on their title screen)", style(DIM)));
                }
                for (i, f) in vs.found.iter().enumerate() {
                    let busy = if f.beacon.in_game || f.beacon.players >= 2 { "  (playing)" } else { "" };
                    let mark = if i == vs.pick { "▶ " } else { "  " };
                    let st = if i == vs.pick { bold(AMBER) } else { style(TEXT) };
                    l.push(line(&format!("{mark}{}{busy}", f.beacon.name), st));
                }
                if !vs.note.is_empty() {
                    l.push(line(&vs.note, style(RED)));
                }
            }
            Stage::Joining | Stage::Match => {
                l.push(line("versus", bold(GREEN)));
                l.push(line("", style(TEXT)));
                l.push(line(&vs.note, style(TEXT)));
            }
            Stage::Gone => {
                l.push(line("no match", bold(RED)));
                l.push(line("", style(TEXT)));
                l.push(line(&vs.note, style(TEXT)));
            }
        }
        l.push(line("", style(TEXT)));
        let keys = match vs.stage {
            Stage::Browsing if !vs.found.is_empty() => "Enter: join      Esc: back",
            Stage::Gone => "Enter or Esc: back",
            _ => "Esc: back",
        };
        l.push(line(keys, bold(AMBER)));
        l
    }

    /// The card and verdict of a versus round.
    fn versus_overlay(&self, vs: &Versus) -> Option<Lines> {
        let g = &self.game;
        let lv = &g.level;
        let rival = vs.rival_name();
        let line = |s: &str, st: Style| (s.to_string(), st);
        let score = format!("match {} : {}   first to {}", vs.wins[0], vs.wins[1], versus::WINS_NEEDED);
        match self.screen {
            Screen::Card => {
                let mut l = vec![
                    line(&format!("round {}  vs {rival}", vs.round), bold(GREEN)),
                    line(&score, style(DIM)),
                    line("", style(TEXT)),
                    line(&format!("$ {} | ./level {}", lv.cmd, lv.n), style(TEXT)),
                    line(&format!("pipe {} segments; the same board for both", lv.required), style(TEXT)),
                    line(&format!("every {} segments you pipe lock a cell on {rival}'s", versus::JUNK_EVERY), style(DIM)),
                    line("", style(TEXT)),
                ];
                let ready = if vs.stage == Stage::Gone {
                    line(&vs.note, bold(RED))
                } else if !vs.me_ready {
                    line(if vs.rival_ready { "Enter when ready (rival is ready)" } else { "Enter when ready" }, bold(AMBER))
                } else {
                    line(&format!("waiting for {rival}..."), style(AMBER))
                };
                l.push(ready);
                Some(l)
            }
            Screen::Result => {
                let mut l = Vec::new();
                let end = |e: Option<RoundEnd>| match e {
                    Some(RoundEnd { ok: true, piped, .. }) => format!("[exit 0]  {piped} segments"),
                    Some(RoundEnd { ok: false, piped, .. }) => format!("Broken pipe  {piped} of {}", lv.required),
                    None => "still piping...".into(),
                };
                let mine_ok = vs.mine.is_some_and(|e| e.ok);
                l.push(line(&format!("you:  {}", end(vs.mine)), bold(if mine_ok { GREEN } else { RED })));
                l.push(line(&format!("{rival}:  {}", end(vs.theirs)), style(TEXT)));
                l.push(line("", style(TEXT)));
                if let (Some(m), Some(t)) = (vs.mine, vs.theirs) {
                    let (s, c) = match versus::verdict(m, t) {
                        Ordering::Greater => ("round to you".to_string(), GREEN),
                        Ordering::Less => (format!("round to {rival}"), RED),
                        Ordering::Equal => ("a draw".to_string(), AMBER),
                    };
                    l.push(line(&s, bold(c)));
                }
                l.push(line(&score, style(TEXT)));
                if vs.match_over() {
                    let s = if vs.wins[0] > vs.wins[1] { "you take the match".to_string() } else { format!("{rival} takes the match") };
                    l.push(line(&s, bold(if vs.wins[0] > vs.wins[1] { GREEN } else { RED })));
                }
                l.push(line("", style(TEXT)));
                let foot = if vs.stage == Stage::Gone {
                    l.insert(0, line(&vs.note, bold(RED)));
                    "Enter: back to the prompt".to_string()
                } else if vs.match_over() {
                    "Enter: back to the prompt".to_string()
                } else if vs.mine.is_none() || vs.theirs.is_none() {
                    format!("waiting for the round to end...")
                } else if vs.host {
                    "Enter: next round".to_string()
                } else {
                    format!("waiting for {rival} to start the next round")
                };
                l.push(line(&foot, bold(AMBER)));
                Some(l)
            }
            _ => None,
        }
    }

    fn snapshot_log(&self) {
        let g = &self.game;
        debug!(
            "snap screen={:?} level={} phase={:?} cursor={:?} clock={:.2} piped={}/{} score={} fast={} lock={:.2} queue={:?}",
            self.screen, g.level.n, g.phase, g.cursor, g.clock, g.piped, g.level.required, g.score, g.fast, g.lock, g.queue
        );
    }

    fn overlay(&self) -> Option<Lines> {
        if let Some(vs) = &self.vs {
            return self.versus_overlay(vs);
        }
        let g = &self.game;
        let lv = &g.level;
        let line = |s: &str, st: Style| (s.to_string(), st);
        match self.screen {
            Screen::Card => {
                let mut l = vec![
                    line(&format!("$ {} | ./level {}", lv.cmd, lv.n), bold(GREEN)),
                    line("", style(TEXT)),
                    line(&format!("pipe {} segments before the pipe breaks", lv.required), style(TEXT)),
                    line(&format!("data starts in {:.0}s", lv.countdown), style(DIM)),
                ];
                if lv.sink {
                    l.push(line("end it in nul (/dev/null) for a bonus", style(DIM)));
                }
                let new = match lv.n {
                    engine::TEES_FROM => Some("new: the tee splits the stream; seal both ends"),
                    engine::BUFFERS_FROM => Some("new: buf, a slow tank; fill it for a bonus"),
                    engine::VALVES_FROM => Some("new: valves, data runs only the arrows' way"),
                    _ => None,
                };
                if let Some(new) = new {
                    l.push(line(new, style(AMBER)));
                }
                l.push(line("", style(TEXT)));
                l.push(line("Enter to start", bold(AMBER)));
                Some(l)
            }
            Screen::Paused => Some(vec![
                line(&format!("[1]+  Stopped        {} | ./level {}", lv.cmd, lv.n), bold(AMBER)),
                line("", style(TEXT)),
                line("P or Enter: fg        Esc: kill %1", style(DIM)),
            ]),
            Screen::Result => {
                let Phase::Done { ok, sink, sealed } = g.phase else { return None };
                let mut l = if ok {
                    let how = if sealed { "every end sealed" } else { "segments piped" };
                    vec![
                        line(&format!("[exit 0]  {} {how}", g.piped), bold(GREEN)),
                        line("", style(TEXT)),
                    ]
                } else if sealed {
                    vec![
                        line(&format!("{}: short write", lv.cmd), bold(RED)),
                        line(&format!("[exit 1]  {} of {} segments", g.piped, lv.required), style(TEXT)),
                        line("", style(TEXT)),
                    ]
                } else {
                    vec![
                        line(&format!("{}: write error: Broken pipe", lv.cmd), bold(RED)),
                        line(&format!("[exit 141]  {} of {} segments", g.piped, lv.required), style(TEXT)),
                        line("", style(TEXT)),
                    ]
                };
                if g.crosses > 0 {
                    l.push(line(&format!("{} loops through a crossover    +{}", g.crosses, g.crosses as i64 * engine::CROSS_BONUS), style(GREEN)));
                }
                if ok && g.crosses >= engine::LOOPS_FOR_BONUS {
                    l.push(line(&format!("{} loops in one level    +{}", g.crosses, engine::LOOPS_BONUS), bold(GREEN)));
                }
                if sink && ok {
                    l.push(line(&format!("nul swallowed it all    +{}", engine::SINK_BONUS), style(GREEN)));
                }
                if g.unused > 0 {
                    l.push(line(&format!("{} pieces never used    -{}", g.unused, g.unused as i64 * engine::UNUSED_COST), style(DIM)));
                }
                l.push(line(&format!("score {}", g.score), bold(TEXT)));
                l.push(line("", style(TEXT)));
                l.push(line(if ok { "Enter: next level" } else { "Enter: back to the prompt" }, bold(AMBER)));
                Some(l)
            }
            _ => None,
        }
    }

    fn draw(&self, buf: &mut Buffer) {
        let area = buf.area;
        if self.screen == Screen::Title {
            render::draw_title(buf, area, self.hi, self.last, self.t);
            return;
        }
        if self.screen == Screen::Lobby {
            render::draw_lobby(buf, area, &self.lobby_lines(), self.t);
            return;
        }
        let cursor = self.cursor && self.screen == Screen::Play;
        let Some(vs) = &self.vs else {
            let hud = Hud { title: "SIGPIPE".into(), hi: Some(self.hi), extra: Vec::new(), paused: self.screen == Screen::Paused, cursor };
            render::draw_play(buf, area, &self.game, &hud, self.overlay().as_ref(), self.t);
            return;
        };
        let rival = vs.rival_name().to_string();
        let mut extra = vec![("match".to_string(), format!("{}:{}", vs.wins[0], vs.wins[1]))];
        let mine = self.my_area();
        let side = mine != area;
        if !side && let Some(v) = &vs.rival_view {
            extra.insert(0, (rival.chars().take(10).collect(), format!("{}/{}", v.piped, self.game.level.required)));
        }
        let hud = Hud { title: "SIGPIPE".into(), hi: None, extra, paused: false, cursor };
        render::draw_play(buf, mine, &self.game, &hud, self.overlay().as_ref(), self.t);
        if side {
            let theirs = Rect::new(area.x + mine.width, area.y, area.width - mine.width, area.height);
            match &vs.rival_view {
                Some(v) => {
                    let g = Game::from_view(v);
                    let hud = Hud { title: rival, hi: None, extra: Vec::new(), paused: false, cursor: true };
                    render::draw_play(buf, theirs, &g, &hud, None, self.t);
                }
                None => render::draw_waiting(buf, theirs, &format!("{rival}'s board shows here once the round starts")),
            }
        }
    }
}

fn main() -> io::Result<()> {
    env_logger::Builder::from_default_env().format_timestamp_millis().init();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
    let level = arg("--level").and_then(|v| v.parse().ok()).unwrap_or(1u32).max(1);
    if let Some(secs) = arg("--shot") {
        let size = arg("--size").and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
        let seed = arg("--seed").and_then(|v| v.parse().ok()).unwrap_or(1);
        let cells = args.iter().any(|a| a == "--cells");
        let deal: Vec<_> = arg("--deal").unwrap_or("").chars().filter_map(engine::Piece::from_char).collect();
        shot(secs.parse().unwrap_or(0.0), level, seed, (arg("--moves").unwrap_or(""), &deal), size.unwrap_or((80, 24)), arg("--screen"), cells);
        return Ok(());
    }
    run(level)
}

fn run(level: u32) -> io::Result<()> {
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture, EnableFocusChange)?;
    info!("start, level {level}");
    let muted = std::env::args().any(|a| a == "--mute");
    let mut app = App::new(level, load_hi(), Audio::new(muted));
    // --level skips the title
    if level > 1 {
        app.start_level(level, 0);
    }
    let mut input = Input::new();
    let mut last = Instant::now();
    let mut last_log = Instant::now();
    let result = loop {
        let deadline = last + FRAME;
        while event::poll(deadline.saturating_duration_since(Instant::now()))? {
            input.on_event(event::read()?);
        }
        let now = Instant::now();
        input.poll_pads(now);
        for a in std::mem::take(&mut input.acts) {
            app.act(a);
        }
        if app.quit {
            break Ok(());
        }
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;
        app.update(dt);
        if now.duration_since(last_log) >= Duration::from_millis(500) {
            last_log = now;
            app.snapshot_log();
        }
        if let Err(e) = terminal.draw(|f| {
            app.area = f.area();
            app.draw(f.buffer_mut());
        }) {
            break Err(e);
        }
    };
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableFocusChange);
    ratatui::restore();
    info!("quit");
    result
}

/// Play `secs` seconds without a terminal and print the screen as text.
/// With `cells`, one line per cell instead: `x y symbol fg bg`, colours as hex,
/// for `tools/shot.py` to draw as an image.
fn shot(secs: f32, level: u32, seed: u64, (moves, deal): (&str, &[engine::Piece]), (w, h): (u16, u16), screen: Option<&str>, cells: bool) {
    let mut app = App::new(level, 0, Audio::new(true));
    app.area = Rect::new(0, 0, w, h);
    app.game = Game::new(Level::new(level), seed, 0);
    if !deal.is_empty() {
        app.game.script(deal);
    }
    app.screen = match screen {
        Some("title") => Screen::Title,
        Some("card") => Screen::Card,
        Some("pause") => Screen::Paused,
        _ => Screen::Play,
    };
    let dt = 1.0 / 60.0;
    let mut elapsed = 0.0;
    for c in moves.chars() {
        let a = match c.to_ascii_uppercase() {
            'U' => Some(Act::Move(engine::Dir::Up)),
            'D' => Some(Act::Move(engine::Dir::Down)),
            'L' => Some(Act::Move(engine::Dir::Left)),
            'R' => Some(Act::Move(engine::Dir::Right)),
            'P' => Some(Act::Ok),
            'F' => Some(Act::Fast),
            _ => None,
        };
        match a {
            Some(a) => app.act(a),
            None => {
                for _ in 0..6 {
                    app.update(dt);
                }
                elapsed += 0.1;
            }
        }
    }
    while elapsed < secs {
        app.update(dt);
        elapsed += dt;
    }
    let mut buf = Buffer::empty(app.area);
    app.draw(&mut buf);
    if cells {
        let hex = |c: ratatui::style::Color| match c {
            ratatui::style::Color::Rgb(r, g, b) => format!("{r:02x}{g:02x}{b:02x}"),
            _ => "000000".into(),
        };
        println!("{w} {h}");
        for y in 0..h {
            for x in 0..w {
                let c = &buf[(x, y)];
                println!("{x} {y} {} {} {}", hex(c.fg), hex(c.bg), c.symbol().escape_unicode());
            }
        }
        return;
    }
    for y in 0..h {
        let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
        println!("{}", row.trim_end());
    }
}
