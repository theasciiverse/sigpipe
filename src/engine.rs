//! The board and its rules: pieces, the queue, the countdown and the flood.
//! No terminal in here; `tick` moves time on, and the rules are tested below.
//!
//! A process writes data into the board from its source cell. Until the countdown
//! runs out you lay pieces from the queue; then the data runs through them. A
//! crossover carries it straight on (and pays when it loops back through the
//! other way, as in the 1989 original); a tee splits it into both its other
//! arms, and from there it floods like water wherever a pathway is open. The
//! moment any of it reaches an opening that leads nowhere the pipe is broken. Pipe at least `required` segments by
//! then and the level is passed; seal every end (loops and `nul`) and it ends
//! cleanly.
//!
//! Each piece is two to four arms, half a segment each, meeting in the middle.
//! Data runs in along one arm, wets the middle, then out along every other arm
//! at once (a crossover: only the arm straight across); an arm that reaches its edge pours into the neighbour. Time runs on a
//! flow clock counted in segments, so fast flow and frame rate change nothing but
//! the speed.

pub const W: usize = 10;
pub const H: usize = 7;
pub const QUEUE: usize = 5;
/// Seconds per segment with fast flow on.
pub const FAST_TIME: f32 = 0.09;
/// A replaced piece is blown out of its cell; no piece goes down for this long.
pub const REPLACE_LOCK: f32 = 0.45;
/// Seconds the data spills before the verdict.
pub const SPILL_TIME: f32 = 1.4;
/// An arm is half a segment of flow clock.
pub const ARM: f32 = 0.5;

pub const SEGMENT_POINTS: i64 = 100;
pub const FAST_POINTS: i64 = 200;
/// Data looping back through a crossover the other way.
pub const CROSS_BONUS: i64 = 500;
/// Loops back through crossovers in one level for the big bonus, after the
/// original's bonus for five.
pub const LOOPS_FOR_BONUS: u32 = 5;
pub const LOOPS_BONUS: i64 = 5000;
/// Data filling a buffer.
pub const BUFFER_BONUS: i64 = 300;
/// A buffer's arm takes this many segments of flow, so four across.
pub const BUFFER_ARM: f32 = 2.0;
pub const REPLACE_COST: i64 = 50;
/// Each laid piece the data never reached, at the end of a level.
pub const UNUSED_COST: i64 = 50;
pub const SINK_BONUS: i64 = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Right,
    Down,
    Left,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Up, Dir::Right, Dir::Down, Dir::Left];

    pub fn bit(self) -> u8 {
        1 << self as u8
    }

    pub fn opposite(self) -> Dir {
        Dir::ALL[(self as usize + 2) % 4]
    }

    pub fn delta(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -1),
            Dir::Right => (1, 0),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
        }
    }

    pub fn horizontal(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }
}

/// The neighbour of `pos` toward `dir`, if it is on the board.
pub fn step(pos: (usize, usize), dir: Dir) -> Option<(usize, usize)> {
    let (dx, dy) = dir.delta();
    let (x, y) = (pos.0 as i32 + dx, pos.1 as i32 + dy);
    (x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H).then_some((x as usize, y as usize))
}

/// The pieces the queue hands out, named by the sides they open to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Piece {
    LeftRight,
    UpDown,
    UpRight,
    UpLeft,
    DownRight,
    DownLeft,
    /// Straight on only, each way: one pipe bridging another.
    Cross,
    /// Tees, named by the side they are closed on: data splits into both others.
    TeeNoUp,
    TeeNoDown,
    TeeNoLeft,
    TeeNoRight,
}

impl Piece {
    /// One character per piece, for scripts: `- | L J F 7 +`, and the tees by
    /// where their stem points, `v ^ > <`.
    pub fn from_char(c: char) -> Option<Piece> {
        Some(match c {
            '-' => Piece::LeftRight,
            '|' => Piece::UpDown,
            'L' => Piece::UpRight,
            'J' => Piece::UpLeft,
            'F' => Piece::DownRight,
            '7' => Piece::DownLeft,
            '+' => Piece::Cross,
            'v' => Piece::TeeNoUp,
            '^' => Piece::TeeNoDown,
            '>' => Piece::TeeNoLeft,
            '<' => Piece::TeeNoRight,
            _ => return None,
        })
    }

    pub fn mask(self) -> u8 {
        use Dir::*;
        match self {
            Piece::LeftRight => Left.bit() | Right.bit(),
            Piece::UpDown => Up.bit() | Down.bit(),
            Piece::UpRight => Up.bit() | Right.bit(),
            Piece::UpLeft => Up.bit() | Left.bit(),
            Piece::DownRight => Down.bit() | Right.bit(),
            Piece::DownLeft => Down.bit() | Left.bit(),
            Piece::Cross => 15,
            Piece::TeeNoUp => 15 & !Up.bit(),
            Piece::TeeNoDown => 15 & !Down.bit(),
            Piece::TeeNoLeft => 15 & !Left.bit(),
            Piece::TeeNoRight => 15 & !Right.bit(),
        }
    }

    pub fn opens(self, side: Dir) -> bool {
        self.mask() & side.bit() != 0
    }
}

/// Data in one arm of a piece: since `start` on the flow clock, running in from
/// the edge or out from the middle; `done` once it has reached the far end.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Arm {
    pub start: f32,
    /// Flow time to fill it.
    pub len: f32,
    pub inward: bool,
    pub done: bool,
}

impl Arm {
    /// How full the arm is at flow time `clock`, 0 to 1.
    pub fn fill(&self, clock: f32) -> f32 {
        ((clock - self.start) / self.len).clamp(0.0, 1.0)
    }
}

/// What the board came with in a pipe cell: nothing (a piece you laid), or a
/// fixed straight that can't be replaced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fixed {
    No,
    /// `buf`: a tank the data takes four segments' time to fill; pays a bonus.
    Buffer,
    /// A one-way valve: data only runs toward this way.
    Valve(Dir),
}

impl Fixed {
    /// Whether data may come in through `side`.
    pub fn admits(self, side: Dir) -> bool {
        match self {
            Fixed::Valve(to) => side == to.opposite(),
            _ => true,
        }
    }

    pub fn arm_len(self) -> f32 {
        if self == Fixed::Buffer { BUFFER_ARM } else { ARM }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cell {
    Empty,
    /// Nothing can be laid here.
    Locked,
    /// `arms` by `Dir`; `wet` once data has reached the middle (on a crossover:
    /// through it either way).
    Pipe { piece: Piece, arms: [Option<Arm>; 4], wet: bool, fixed: Fixed },
    /// The writing process; `fill` shows the countdown.
    Source { out: Dir, fill: f32 },
    /// `/dev/null`: swallows whatever reaches it, which seals that end.
    Sink { inlet: Dir, arm: Option<Arm> },
}

impl Cell {
    fn pipe(piece: Piece) -> Cell {
        Cell::Pipe { piece, arms: [None; 4], wet: false, fixed: Fixed::No }
    }

    /// A piece the player laid that the data hasn't reached.
    fn dry(&self) -> bool {
        matches!(self, Cell::Pipe { arms: [None, None, None, None], fixed: Fixed::No, .. })
    }
}

/// One level's numbers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Level {
    pub n: u32,
    /// Segments to pipe before the data spills.
    pub required: u32,
    /// Seconds before the data starts.
    pub countdown: f32,
    /// Seconds the data takes through one segment.
    pub segment_time: f32,
    pub locked: usize,
    pub sink: bool,
    pub buffers: usize,
    pub valves: usize,
    /// The source process's name, three letters at most.
    pub cmd: &'static str,
}

pub const BUFFERS_FROM: u32 = 4;
pub const VALVES_FROM: u32 = 5;

const CMDS: [&str; 12] = ["cat", "yes", "seq", "tar", "dd", "ls", "cut", "awk", "sed", "zip", "git", "od"];

impl Level {
    pub fn new(n: u32) -> Level {
        let n = n.max(1);
        let k = (n - 1) as f32;
        Level {
            n,
            required: (8 + 2 * (n - 1)).min(30),
            countdown: (24.0 - 2.0 * k).max(8.0),
            segment_time: (2.2 * 0.9f32.powf(k)).max(0.6),
            locked: (n as usize - 1).min(8),
            sink: n >= 3,
            buffers: if n >= 7 { 2 } else { (n >= BUFFERS_FROM) as usize },
            valves: if n >= 8 { 2 } else { (n >= VALVES_FROM) as usize },
            cmd: CMDS[(n as usize - 1) % CMDS.len()],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Countdown { left: f32 },
    Flowing,
    /// The data ran out of pipe leaving `at` toward `dir`.
    Spill { at: (usize, usize), dir: Dir, t: f32 },
    /// `sealed`: every end closed, nothing spilled.
    Done { ok: bool, sink: bool, sealed: bool },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    Placed { piece: Piece, at: (usize, usize), replaced: bool },
    Denied { at: (usize, usize) },
    FlowStarts,
    Segment { at: (usize, usize), looped: bool, buffer: bool },
    /// `nul` swallowed a stream.
    Sink,
    Fast(bool),
    Spill { at: (usize, usize), dir: Dir },
    Done { ok: bool, sink: bool, sealed: bool, unused: u32, crosses: u32 },
}

/// One bag of pieces: two of each straight and corner, one crossing.
const BAG: [Piece; 13] = [
    Piece::LeftRight,
    Piece::LeftRight,
    Piece::UpDown,
    Piece::UpDown,
    Piece::UpRight,
    Piece::UpRight,
    Piece::UpLeft,
    Piece::UpLeft,
    Piece::DownRight,
    Piece::DownRight,
    Piece::DownLeft,
    Piece::DownLeft,
    Piece::Cross,
];

/// Tees join the bag from this level, one of each.
pub const TEES_FROM: u32 = 2;
const TEES: [Piece; 4] = [Piece::TeeNoUp, Piece::TeeNoDown, Piece::TeeNoLeft, Piece::TeeNoRight];

/// Small and seedable, so a level can be replayed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng((seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() >> 33) as usize % n
    }
}

pub struct Game {
    pub level: Level,
    pub cells: Vec<Cell>,
    /// `queue[0]` goes down next.
    pub queue: [Piece; QUEUE],
    pub cursor: (usize, usize),
    pub phase: Phase,
    pub source: (usize, usize),
    /// Flow time in segments since the data started.
    pub clock: f32,
    pub piped: u32,
    pub score: i64,
    pub fast: bool,
    /// Seconds before another piece can go down.
    pub lock: f32,
    /// A cell just blown out by a replacement, and how long it still flashes.
    pub boom: Option<((usize, usize), f32)>,
    /// Laid pieces the data never reached, counted when the level ends.
    pub unused: u32,
    /// Loops back through a crossover.
    pub crosses: u32,
    pub events: Vec<Event>,
    rng: Rng,
    /// Pieces to deal before the random ones (headless shots and tests).
    script: Vec<Piece>,
    bag: Vec<Piece>,
}

impl Game {
    /// A fresh level carrying the session's `score`.
    pub fn new(level: Level, seed: u64, score: i64) -> Game {
        let mut rng = Rng::new(seed);
        let src = (1 + rng.below(W - 2), 1 + rng.below(H - 2));
        let out = Dir::ALL[rng.below(4)];
        let mut g = Game::bare(level, src, out, rng);
        g.score = score;
        let front = step(src, out).expect("the source sits off the edge");
        let mut reserved = vec![src, front];
        if level.sink {
            loop {
                let pos = (g.rng.below(W), g.rng.below(H));
                let inlet = Dir::ALL[g.rng.below(4)];
                let far = pos.0.abs_diff(src.0) + pos.1.abs_diff(src.1) >= 5;
                let Some(mouth) = step(pos, inlet) else { continue };
                if far && !reserved.contains(&pos) && !reserved.contains(&mouth) {
                    g.cells[pos.1 * W + pos.0] = Cell::Sink { inlet, arm: None };
                    reserved.extend([pos, mouth]);
                    break;
                }
            }
        }
        // buffers and valves: straights with both ends on the board and free
        let mut fixed: Vec<Fixed> = vec![Fixed::Buffer; level.buffers];
        for _ in 0..level.valves {
            fixed.push(Fixed::Valve(Dir::ALL[g.rng.below(4)]));
        }
        for f in fixed {
            loop {
                let pos = (g.rng.below(W), g.rng.below(H));
                let horizontal = match f {
                    Fixed::Valve(d) => d.horizontal(),
                    _ => g.rng.below(2) == 0,
                };
                let (a, b) = if horizontal { (Dir::Left, Dir::Right) } else { (Dir::Up, Dir::Down) };
                let (Some(na), Some(nb)) = (step(pos, a), step(pos, b)) else { continue };
                if [pos, na, nb].iter().any(|p| reserved.contains(p)) || g.cell(pos) != Cell::Empty {
                    continue;
                }
                let piece = if horizontal { Piece::LeftRight } else { Piece::UpDown };
                *g.cell_mut(pos) = Cell::Pipe { piece, arms: [None; 4], wet: false, fixed: f };
                reserved.extend([pos, na, nb]);
                break;
            }
        }
        let mut locked = 0;
        while locked < level.locked {
            let pos = (g.rng.below(W), g.rng.below(H));
            if !reserved.contains(&pos) && g.cells[pos.1 * W + pos.0] == Cell::Empty {
                g.cells[pos.1 * W + pos.0] = Cell::Locked;
                locked += 1;
            }
        }
        g
    }

    /// An empty board with only the source on it.
    pub fn bare(level: Level, src: (usize, usize), out: Dir, rng: Rng) -> Game {
        let mut cells = vec![Cell::Empty; W * H];
        cells[src.1 * W + src.0] = Cell::Source { out, fill: 0.0 };
        let mut g = Game {
            level,
            cells,
            queue: [Piece::Cross; QUEUE],
            cursor: step(src, out).unwrap_or(src),
            phase: Phase::Countdown { left: level.countdown },
            source: src,
            clock: 0.0,
            piped: 0,
            score: 0,
            fast: false,
            lock: 0.0,
            boom: None,
            unused: 0,
            crosses: 0,
            events: Vec::new(),
            rng,
            script: Vec::new(),
            bag: Vec::new(),
        };
        for i in 0..QUEUE {
            g.queue[i] = g.deal();
        }
        g
    }

    /// Deal `pieces` first, from a fresh queue.
    pub fn script(&mut self, pieces: &[Piece]) {
        self.script = pieces.iter().rev().copied().collect();
        for i in 0..QUEUE {
            self.queue[i] = self.deal();
        }
    }

    /// From a shuffled bag, refilled when empty, so no shape is ever long in
    /// coming: a plain random draw left players waiting on one corner for ages.
    fn deal(&mut self) -> Piece {
        if let Some(p) = self.script.pop() {
            return p;
        }
        if self.bag.is_empty() {
            self.bag.extend(BAG);
            if self.level.n >= TEES_FROM {
                self.bag.extend(TEES);
            }
            for i in (1..self.bag.len()).rev() {
                let j = self.rng.below(i + 1);
                self.bag.swap(i, j);
            }
        }
        self.bag.pop().expect("the bag was just filled")
    }

    pub fn cell(&self, pos: (usize, usize)) -> Cell {
        self.cells[pos.1 * W + pos.0]
    }

    fn cell_mut(&mut self, pos: (usize, usize)) -> &mut Cell {
        &mut self.cells[pos.1 * W + pos.0]
    }

    pub fn over(&self) -> bool {
        matches!(self.phase, Phase::Spill { .. } | Phase::Done { .. })
    }

    pub fn move_cursor(&mut self, dir: Dir) {
        if let Some(p) = step(self.cursor, dir) {
            self.cursor = p;
        }
    }

    /// Lay the next piece at the cursor. An empty cell takes it; a piece the data
    /// has not reached is blown out and replaced, at a cost.
    pub fn place(&mut self) -> bool {
        let at = self.cursor;
        if self.over() {
            return false;
        }
        let replaced = match self.cell(at) {
            Cell::Empty => false,
            c if c.dry() => true,
            _ => {
                self.events.push(Event::Denied { at });
                return false;
            }
        };
        if self.lock > 0.0 {
            self.events.push(Event::Denied { at });
            return false;
        }
        if replaced {
            self.score -= REPLACE_COST;
            self.lock = REPLACE_LOCK;
            self.boom = Some((at, REPLACE_LOCK));
        }
        let piece = self.queue[0];
        *self.cell_mut(at) = Cell::pipe(piece);
        self.queue.rotate_left(1);
        self.queue[QUEUE - 1] = self.deal();
        self.events.push(Event::Placed { piece, at, replaced });
        true
    }

    /// Fast flow on or off. During the countdown it also starts the data at once.
    pub fn toggle_fast(&mut self) {
        if self.over() {
            return;
        }
        self.fast = !self.fast;
        self.events.push(Event::Fast(self.fast));
    }

    /// Arms with data still running through them.
    pub fn fronts(&self) -> u32 {
        let running = |a: &Option<Arm>| a.is_some_and(|a| !a.done);
        self.cells
            .iter()
            .map(|c| match c {
                Cell::Pipe { arms, .. } => arms.iter().filter(|a| running(a)).count() as u32,
                Cell::Sink { arm, .. } => running(arm) as u32,
                _ => 0,
            })
            .sum()
    }

    pub fn segment_time(&self) -> f32 {
        if self.fast { FAST_TIME } else { self.level.segment_time }
    }

    pub fn tick(&mut self, dt: f32) {
        self.lock = (self.lock - dt).max(0.0);
        if let Some((_, t)) = &mut self.boom {
            *t -= dt;
            if *t <= 0.0 {
                self.boom = None;
            }
        }
        match self.phase {
            Phase::Countdown { left } => {
                let left = if self.fast { 0.0 } else { left - dt };
                let fill = 1.0 - (left / self.level.countdown).clamp(0.0, 1.0);
                let src = self.source;
                if let Cell::Source { fill: f, .. } = self.cell_mut(src) {
                    *f = fill;
                }
                if left <= 0.0 {
                    self.phase = Phase::Flowing;
                    self.events.push(Event::FlowStarts);
                    // the part of this tick after the countdown ran out
                    self.clock = if self.fast { 0.0 } else { -left / self.segment_time() };
                    let Cell::Source { out, .. } = self.cell(src) else { unreachable!() };
                    self.pour(src, out, 0.0);
                    self.settle();
                } else {
                    self.phase = Phase::Countdown { left };
                }
            }
            Phase::Flowing => {
                self.clock += dt / self.segment_time();
                self.settle();
            }
            Phase::Spill { at, dir, t } => {
                let t = t + dt;
                self.phase = Phase::Spill { at, dir, t };
                if t >= SPILL_TIME {
                    self.finish(false);
                }
            }
            Phase::Done { .. } => {}
        }
    }

    /// Run every arm that ends by now on the flow clock, earliest first, so the
    /// flood comes out the same whatever the frame rate. Ends the level if the
    /// data spilled, or if no arm is still running.
    fn settle(&mut self) {
        while self.phase == Phase::Flowing {
            let mut next: Option<(f32, usize, usize)> = None;
            let mut running = false;
            for (i, c) in self.cells.iter().enumerate() {
                let arms: &[Option<Arm>] = match c {
                    Cell::Pipe { arms, .. } => arms,
                    Cell::Sink { arm, .. } => std::slice::from_ref(arm),
                    _ => continue,
                };
                for (k, a) in arms.iter().enumerate() {
                    let Some(a) = a else { continue };
                    if a.done {
                        continue;
                    }
                    running = true;
                    let end = a.start + a.len;
                    if end <= self.clock && next.is_none_or(|(t, ..)| end < t) {
                        next = Some((end, i, k));
                    }
                }
            }
            let Some((t, i, k)) = next else {
                if !running {
                    self.finish(true);
                }
                return;
            };
            let pos = (i % W, i / W);
            match &mut self.cells[i] {
                Cell::Sink { arm: Some(a), .. } => {
                    a.done = true;
                    self.events.push(Event::Sink);
                }
                Cell::Pipe { piece, arms, wet, fixed } => {
                    let len = fixed.arm_len();
                    let buffer = *fixed == Fixed::Buffer;
                    let a = arms[k].as_mut().expect("a running arm");
                    a.done = true;
                    let dir = Dir::ALL[k];
                    if !a.inward {
                        self.pour(pos, dir, t);
                    } else if *piece == Piece::Cross {
                        // straight across, one pass each way
                        let across = dir.opposite() as usize;
                        if arms[across].is_none() {
                            arms[across] = Some(Arm { start: t, len, inward: false, done: false });
                            let looped = *wet;
                            *wet = true;
                            self.segment(pos, looped, false);
                        }
                    } else if !*wet {
                        *wet = true;
                        for d in Dir::ALL {
                            if piece.opens(d) && arms[d as usize].is_none() {
                                arms[d as usize] = Some(Arm { start: t, len, inward: false, done: false });
                            }
                        }
                        self.segment(pos, false, buffer);
                    }
                }
                _ => unreachable!("arms are only on pipes and the sink"),
            }
        }
    }

    fn segment(&mut self, at: (usize, usize), looped: bool, buffer: bool) {
        self.piped += 1;
        self.score += if self.fast { FAST_POINTS } else { SEGMENT_POINTS };
        if looped {
            self.crosses += 1;
            self.score += CROSS_BONUS;
        }
        if buffer {
            self.score += BUFFER_BONUS;
        }
        self.events.push(Event::Segment { at, looped, buffer });
    }

    /// Data leaves `from` toward `dir` at flow time `t`: into the neighbour if it
    /// opens this way (and has no data on that side already), otherwise it spills.
    fn pour(&mut self, from: (usize, usize), dir: Dir, t: f32) {
        let side = dir.opposite();
        if let Some(pos) = step(from, dir) {
            match self.cell_mut(pos) {
                Cell::Pipe { piece, arms, fixed, .. } if piece.opens(side) && fixed.admits(side) => {
                    // data already there from the other end just meets this
                    let len = fixed.arm_len();
                    arms[side as usize].get_or_insert(Arm { start: t, len, inward: true, done: false });
                    return;
                }
                Cell::Sink { inlet, arm } if *inlet == side => {
                    arm.get_or_insert(Arm { start: t, len: ARM, inward: true, done: false });
                    return;
                }
                _ => {}
            }
        }
        self.phase = Phase::Spill { at: from, dir, t: 0.0 };
        self.events.push(Event::Spill { at: from, dir });
    }

    fn finish(&mut self, sealed: bool) {
        let ok = self.piped >= self.level.required;
        self.unused = self.cells.iter().filter(|c| c.dry()).count() as u32;
        let sink = self.cells.iter().any(|c| matches!(c, Cell::Sink { arm: Some(Arm { done: true, .. }), .. }));
        self.score -= self.unused as i64 * UNUSED_COST;
        if ok && sink {
            self.score += SINK_BONUS;
        }
        if ok && self.crosses >= LOOPS_FOR_BONUS {
            self.score += LOOPS_BONUS;
        }
        self.phase = Phase::Done { ok, sink, sealed };
        self.events.push(Event::Done { ok, sink, sealed, unused: self.unused, crosses: self.crosses });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level() -> Level {
        Level { n: 1, required: 3, countdown: 1.0, segment_time: 0.5, locked: 0, sink: false, buffers: 0, valves: 0, cmd: "cat" }
    }

    fn lay(g: &mut Game, at: (usize, usize), piece: Piece) {
        g.queue[0] = piece;
        g.cursor = at;
        assert!(g.place());
    }

    fn run(g: &mut Game, secs: f32) {
        for _ in 0..(secs * 100.0).round() as u32 {
            g.tick(0.01);
        }
    }

    fn wet(g: &Game, pos: (usize, usize)) -> bool {
        matches!(g.cell(pos), Cell::Pipe { wet: true, .. })
    }

    #[test]
    fn short_pipe_breaks() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 3), Piece::LeftRight);
        lay(&mut g, (3, 3), Piece::LeftRight);
        run(&mut g, 1.0 + 0.5 * 2.0 + 0.05);
        assert_eq!(g.piped, 2);
        assert!(matches!(g.phase, Phase::Spill { at: (3, 3), dir: Dir::Right, .. }));
        run(&mut g, SPILL_TIME);
        assert_eq!(g.phase, Phase::Done { ok: false, sink: false, sealed: false });
        assert_eq!(g.score, 2 * SEGMENT_POINTS);
    }

    #[test]
    fn long_enough_passes() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        for x in 2..6 {
            lay(&mut g, (x, 3), Piece::LeftRight);
        }
        lay(&mut g, (8, 0), Piece::UpDown);
        run(&mut g, 1.0 + 0.5 * 4.0 + SPILL_TIME + 0.1);
        assert_eq!(g.phase, Phase::Done { ok: true, sink: false, sealed: false });
        assert_eq!(g.unused, 1);
        assert_eq!(g.score, 4 * SEGMENT_POINTS - UNUSED_COST);
    }

    #[test]
    fn crossover_goes_straight_and_pays_on_the_loop() {
        // source → cross → round the block → back up through the cross
        let mut g = Game::bare(level(), (1, 1), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 1), Piece::Cross);
        lay(&mut g, (3, 1), Piece::DownLeft);
        lay(&mut g, (3, 2), Piece::UpLeft);
        lay(&mut g, (2, 2), Piece::UpRight);
        run(&mut g, 1.0 + 0.5 * 5.0 + 0.05);
        assert_eq!(g.piped, 5, "the cross counts once per pass");
        assert_eq!(g.crosses, 1);
        assert!(matches!(g.phase, Phase::Spill { at: (2, 1), dir: Dir::Up, .. }));
        assert_eq!(g.score, 5 * SEGMENT_POINTS + CROSS_BONUS);
    }

    #[test]
    fn crossover_ignores_its_side_arms() {
        // nothing above or below the cross, and it doesn't matter
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 3), Piece::Cross);
        lay(&mut g, (3, 3), Piece::LeftRight);
        run(&mut g, 1.0 + 0.5 * 1.8);
        assert_eq!(g.phase, Phase::Flowing);
        assert!(wet(&g, (3, 3)));
    }

    #[test]
    fn tee_splits_both_ways() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 3), Piece::TeeNoRight);
        lay(&mut g, (2, 2), Piece::UpDown);
        lay(&mut g, (2, 4), Piece::UpDown);
        // the tee is wet at 0.5 segments, both branches together at 1.5
        run(&mut g, 1.0 + 0.5 * 1.5 + 0.02);
        assert!(wet(&g, (2, 2)) && wet(&g, (2, 4)));
        assert_eq!(g.piped, 3);
        run(&mut g, 0.5);
        assert!(matches!(g.phase, Phase::Spill { .. }));
    }

    #[test]
    fn a_tee_branch_with_no_pipe_spills() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 3), Piece::TeeNoUp);
        for x in 3..9 {
            lay(&mut g, (x, 3), Piece::LeftRight);
        }
        run(&mut g, 1.0 + 0.5 * 1.0 + 0.02);
        assert!(matches!(g.phase, Phase::Spill { at: (2, 3), dir: Dir::Down, .. }));
    }

    #[test]
    fn sealed_loop_ends_cleanly() {
        // source → tee; one branch round a loop into the other: every end closed
        let mut g = Game::bare(level(), (1, 1), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 1), Piece::TeeNoUp);
        lay(&mut g, (3, 1), Piece::DownLeft);
        lay(&mut g, (3, 2), Piece::UpLeft);
        lay(&mut g, (2, 2), Piece::UpRight);
        run(&mut g, 1.0 + 0.5 * 6.0);
        assert_eq!(g.phase, Phase::Done { ok: true, sink: false, sealed: true });
        assert_eq!(g.piped, 4);
        assert_eq!(g.score, 4 * SEGMENT_POINTS);
    }

    fn fix(g: &mut Game, at: (usize, usize), piece: Piece, fixed: Fixed) {
        *g.cell_mut(at) = Cell::Pipe { piece, arms: [None; 4], wet: false, fixed };
    }

    #[test]
    fn buffer_is_slow_and_pays() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        fix(&mut g, (2, 3), Piece::LeftRight, Fixed::Buffer);
        lay(&mut g, (3, 3), Piece::LeftRight);
        // in 2 segments, out 2, then the straight: wet at 4.5
        run(&mut g, 1.0 + 0.5 * 4.4);
        assert!(!wet(&g, (3, 3)));
        run(&mut g, 0.5 * 0.2);
        assert!(wet(&g, (3, 3)));
        assert_eq!(g.score, 2 * SEGMENT_POINTS + BUFFER_BONUS);
        g.cursor = (2, 3);
        assert!(!g.place(), "the board's own pieces stay");
    }

    #[test]
    fn valve_only_one_way() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        fix(&mut g, (2, 3), Piece::LeftRight, Fixed::Valve(Dir::Left));
        run(&mut g, 1.02);
        assert!(matches!(g.phase, Phase::Spill { at: (1, 3), dir: Dir::Right, .. }), "against the valve");
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        fix(&mut g, (2, 3), Piece::LeftRight, Fixed::Valve(Dir::Right));
        run(&mut g, 1.6);
        assert!(wet(&g, (2, 3)));
    }

    #[test]
    fn five_loops_pay_big() {
        // five real loops take most of a board; the loop itself is tested above,
        // so this checks the bonus on finishing
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        g.piped = 10;
        g.crosses = LOOPS_FOR_BONUS;
        g.finish(false);
        assert_eq!(g.score, LOOPS_BONUS);
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        g.piped = 10;
        g.crosses = LOOPS_FOR_BONUS - 1;
        g.finish(false);
        assert_eq!(g.score, 0);
    }

    #[test]
    fn replacing_costs_and_locks() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        lay(&mut g, (5, 5), Piece::UpDown);
        lay(&mut g, (5, 5), Piece::LeftRight);
        assert_eq!(g.score, -REPLACE_COST);
        assert!(g.lock > 0.0);
        g.cursor = (6, 5);
        assert!(!g.place(), "locked right after a replacement");
        run(&mut g, REPLACE_LOCK + 0.02);
        assert!(g.place());
    }

    #[test]
    fn wet_pipe_stays() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        lay(&mut g, (2, 3), Piece::LeftRight);
        run(&mut g, 1.1);
        g.cursor = (2, 3);
        assert!(!g.place(), "data has reached it");
        g.cursor = (1, 3);
        assert!(!g.place(), "the source cannot be built over");
    }

    #[test]
    fn sink_ends_cleanly() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        *g.cell_mut((5, 3)) = Cell::Sink { inlet: Dir::Left, arm: None };
        for x in 2..5 {
            lay(&mut g, (x, 3), Piece::LeftRight);
        }
        run(&mut g, 1.0 + 0.5 * 3.5 + 0.05);
        assert_eq!(g.phase, Phase::Done { ok: true, sink: true, sealed: true });
        assert_eq!(g.score, 3 * SEGMENT_POINTS + SINK_BONUS);
    }

    #[test]
    fn frame_rate_does_not_matter() {
        let build = || {
            let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
            lay(&mut g, (2, 3), Piece::TeeNoRight);
            lay(&mut g, (2, 2), Piece::UpDown);
            lay(&mut g, (2, 4), Piece::UpDown);
            g
        };
        let (mut a, mut b) = (build(), build());
        for _ in 0..190 {
            a.tick(0.01);
        }
        for _ in 0..19 {
            b.tick(0.1);
        }
        assert_eq!(a.piped, b.piped);
        assert_eq!(std::mem::discriminant(&a.phase), std::mem::discriminant(&b.phase));
    }

    #[test]
    fn fast_skips_the_countdown() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(1));
        g.toggle_fast();
        g.tick(0.01);
        assert!(matches!(g.phase, Phase::Spill { .. }), "nothing laid, so it breaks at once");
    }

    #[test]
    fn every_bag_holds_every_shape() {
        let mut g = Game::bare(level(), (1, 3), Dir::Right, Rng::new(9));
        g.bag.clear();
        g.level.n = 1;
        let deals: Vec<Piece> = (0..13 * 40).map(|_| g.deal()).collect();
        for bag in deals.chunks(13) {
            for p in BAG {
                assert_eq!(bag.iter().filter(|q| **q == p).count(), BAG.iter().filter(|q| **q == p).count(), "{p:?}");
            }
        }
        // worst case for a corner: first in one bag, last two of the next
        for p in [Piece::UpRight, Piece::UpLeft, Piece::DownRight, Piece::DownLeft] {
            let at: Vec<usize> = deals.iter().enumerate().filter(|(_, q)| **q == p).map(|(i, _)| i).collect();
            assert!(at.windows(2).all(|w| w[1] - w[0] <= 23), "{p:?} drought");
        }
        g.level.n = TEES_FROM;
        g.bag.clear();
        let deals: Vec<Piece> = (0..17 * 10).map(|_| g.deal()).collect();
        for bag in deals.chunks(17) {
            for t in TEES {
                assert_eq!(bag.iter().filter(|q| **q == t).count(), 1, "{t:?}");
            }
        }
    }

    #[test]
    fn levels_generate() {
        for n in 1..20 {
            for seed in 0..50 {
                let g = Game::new(Level::new(n), seed, 0);
                let locked = g.cells.iter().filter(|c| **c == Cell::Locked).count();
                assert_eq!(locked, g.level.locked);
                assert_eq!(g.cells.iter().filter(|c| matches!(c, Cell::Sink { .. })).count(), g.level.sink as usize);
                let Cell::Source { out, .. } = g.cell(g.source) else { panic!() };
                assert_eq!(g.cell(step(g.source, out).unwrap()), Cell::Empty);
                let fixed = |f: fn(Fixed) -> bool| g.cells.iter().filter(|c| matches!(c, Cell::Pipe { fixed, .. } if f(*fixed))).count();
                assert_eq!(fixed(|f| f == Fixed::Buffer), g.level.buffers);
                assert_eq!(fixed(|f| matches!(f, Fixed::Valve(_))), g.level.valves);
            }
        }
    }
}
