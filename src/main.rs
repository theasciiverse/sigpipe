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
mod render;

use audio::{Audio, Sfx};
use engine::{Event, Game, Level, Phase};
use input::{Act, Input};
use log::{debug, info};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::Style;
use render::{bold, style, Hud, Lines, AMBER, DIM, GREEN, RED, TEXT};
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
        match (self.screen, a) {
            (_, Act::Mute) => {
                self.audio.toggle_mute();
                info!("muted {}", self.audio.muted);
            }
            (_, Act::Quit) => {
                if self.screen != Screen::Title {
                    self.to_title();
                }
                self.quit = true;
            }
            (Screen::Title, Act::Ok) => {
                self.audio.play(Sfx::Select);
                self.start_level(1, 0);
            }
            (Screen::Title, Act::Back) => self.quit = true,
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
                if let Some(pos) = render::tile_at(&render::layout(self.area), x, y) {
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
            if let Event::Done { .. } = e {
                self.screen = Screen::Result;
            }
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

    fn snapshot_log(&self) {
        let g = &self.game;
        debug!(
            "snap screen={:?} level={} phase={:?} cursor={:?} clock={:.2} piped={}/{} score={} fast={} lock={:.2} queue={:?}",
            self.screen, g.level.n, g.phase, g.cursor, g.clock, g.piped, g.level.required, g.score, g.fast, g.lock, g.queue
        );
    }

    fn overlay(&self) -> Option<Lines> {
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
        let hud = Hud { hi: self.hi, paused: self.screen == Screen::Paused, cursor: self.cursor && self.screen == Screen::Play };
        render::draw_play(buf, area, &self.game, &hud, self.overlay().as_ref(), self.t);
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
    execute!(io::stdout(), EnableMouseCapture)?;
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
    let _ = execute!(io::stdout(), DisableMouseCapture);
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
    app.game.script(deal);
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
