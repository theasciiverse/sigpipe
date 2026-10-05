//! Drawing: box-drawing walls for the pipes, half-blocks for the data inside them.
//!
//! A board cell is a tile of 6 by 3 characters, so it is square on screen. Each
//! piece is drawn from hand-made art where `·` marks the inside of the pipe; every
//! such character is two half-block pixels, stacked, and a pixel lights up once the
//! data front has passed it. The insides line up across tiles: the horizontal
//! channel is the middle row, the vertical one columns 2 and 3, so the inside of a
//! tile is a 6 by 6 grid of pixels with the two channels crossing in the middle.

use crate::engine::{Arm, Cell, Dir, Fixed, Game, Phase, Piece, H, QUEUE, SPILL_TIME, W};
use crate::input::Act;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

pub const TW: u16 = 6;
pub const TH: u16 = 3;
/// The whole play screen: queue panel, a gap, the boxed board; status line on top.
pub const NEED_W: u16 = 8 + 1 + W as u16 * TW + 2;
pub const NEED_H: u16 = 1 + H as u16 * TH + 2;

pub const AMBER: Color = Color::Rgb(255, 176, 0);
pub const GREEN: Color = Color::Rgb(80, 255, 120);
pub const RED: Color = Color::Rgb(255, 72, 52);
pub const DIM: Color = Color::Rgb(115, 122, 118);
pub const TEXT: Color = Color::Rgb(200, 208, 204);
const WALL: Color = Color::Rgb(160, 172, 166);
const BG: Color = Color::Rgb(8, 11, 9);
/// The inside of a pipe with no data in it yet.
const HOLLOW: Color = Color::Rgb(26, 34, 30);
const GRID: Color = Color::Rgb(48, 56, 52);
const LOCKED: Color = Color::Rgb(56, 58, 66);
const CURSOR_BG: Color = Color::Rgb(58, 40, 6);
const GHOST: Color = Color::Rgb(150, 104, 20);
const PANEL_BG: Color = Color::Rgb(12, 15, 13);

pub type Lines = Vec<(String, Style)>;

pub fn style(fg: Color) -> Style {
    Style::new().fg(fg).bg(BG)
}

pub fn bold(fg: Color) -> Style {
    style(fg).add_modifier(Modifier::BOLD)
}

fn piece_art(p: Piece) -> [&'static str; 3] {
    match p {
        Piece::LeftRight => ["══════", "······", "══════"],
        Piece::UpDown => [" ║··║ ", " ║··║ ", " ║··║ "],
        Piece::UpRight => [" ║··╚═", " ║····", " ╚════"],
        Piece::UpLeft => ["═╝··║ ", "····║ ", "════╝ "],
        Piece::DownRight => [" ╔════", " ║····", " ║··╔═"],
        Piece::DownLeft => ["════╗ ", "····║ ", "═╗··║ "],
        // a bridge: the vertical pipe's walls run across the horizontal one as
        // thin rails (`¦`), with its data showing through underneath
        Piece::Cross => ["═╣··╠═", "·¦··¦·", "═╣··╠═"],
        Piece::TeeNoUp => ["══════", "······", "═╗··╔═"],
        Piece::TeeNoDown => ["═╝··╚═", "······", "══════"],
        Piece::TeeNoLeft => [" ║··╚═", " ║····", " ║··╔═"],
        Piece::TeeNoRight => ["═╝··║ ", "····║ ", "═╗··║ "],
    }
}

/// The board's fixed straights: a buffer is labelled in its wall, a valve has
/// arrows in its walls pointing the way the data may run.
fn fixed_art(piece: Piece, fixed: Fixed) -> [&'static str; 3] {
    let h = piece == Piece::LeftRight;
    match fixed {
        Fixed::Buffer if h => ["═@@@══", "······", "══════"],
        Fixed::Buffer => ["@║··║ ", "@║··║ ", "@║··║ "],
        Fixed::Valve(Dir::Right) => ["═▸══▸═", "······", "═▸══▸═"],
        Fixed::Valve(Dir::Left) => ["═◂══◂═", "······", "═◂══◂═"],
        Fixed::Valve(Dir::Up) => [" ║··║ ", " ▴··▴ ", " ║··║ "],
        Fixed::Valve(Dir::Down) => [" ║··║ ", " ▾··▾ ", " ║··║ "],
        Fixed::No => piece_art(piece),
    }
}

/// A process box (source or sink) with its one opening; `@` is the label.
fn box_art(open: Dir) -> [&'static str; 3] {
    match open {
        Dir::Right => ["╔═════", "║@@@··", "╚═════"],
        Dir::Left => ["═════╗", "··@@@║", "═════╝"],
        Dir::Up => ["╔╝··╚╗", "║@@@ ║", "╚════╝"],
        Dir::Down => ["╔════╗", "║@@@ ║", "╚╗··╔╝"],
    }
}

/// How far pixel (x, y) of a tile lies from its `side` edge, 0 to 5.
fn depth(x: i32, y: i32, side: Dir) -> i32 {
    match side {
        Dir::Left => x,
        Dir::Right => 5 - x,
        Dir::Up => y,
        Dir::Down => 5 - y,
    }
}

fn in_channel(x: i32, y: i32, horizontal: bool) -> bool {
    if horizontal { (2..4).contains(&y) } else { (2..4).contains(&x) }
}

/// Whether pixel (x, y) of a pipe holds data. Each arm is the half-channel from
/// its edge to the middle (depths 0 to 2), filled from the edge if the data runs
/// in and from the middle if it runs out; once wet, the whole middle is lit (a
/// crossover's middle: by the arms through it, either way).
fn lit_pipe(x: i32, y: i32, piece: Piece, arms: &[Option<Arm>; 4], wet: bool, clock: f32) -> bool {
    let middle = (2..4).contains(&x) && (2..4).contains(&y);
    if wet && middle && piece != Piece::Cross {
        return true;
    }
    Dir::ALL.into_iter().any(|d| {
        let Some(a) = arms[d as usize] else { return false };
        let dep = depth(x, y, d);
        if !in_channel(x, y, d.horizontal()) || dep > 2 {
            return false;
        }
        let f = a.fill(clock) * 3.0;
        if a.inward { (dep as f32) < f } else { ((2 - dep) as f32) < f }
    })
}

/// Data colour: green with a ripple running along the stream.
fn data(gx: i32, gy: i32, t: f32, base: Color) -> Color {
    let Color::Rgb(r, g, b) = base else { return base };
    let w = ((gx as f32 * 0.9 + gy as f32 * 0.55 - t * 7.0).sin() * 0.5 + 0.5) * 0.35 + 0.65;
    Color::Rgb((r as f32 * w) as u8, (g as f32 * w) as u8, (b as f32 * w) as u8)
}

fn mix(a: Color, b: Color, k: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => b,
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, sym: &str, st: Style) {
    if buf.area.contains((x, y).into()) {
        buf[(x, y)].set_symbol(sym).set_style(st);
    }
}

pub fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style) {
    for (i, ch) in s.chars().enumerate() {
        put(buf, x + i as u16, y, ch.encode_utf8(&mut [0; 4]), st);
    }
}

fn centered(buf: &mut Buffer, area: Rect, y: u16, s: &str, st: Style) {
    let w = s.chars().count() as u16;
    text(buf, area.x + area.width.saturating_sub(w) / 2, y, s, st);
}

fn fill(buf: &mut Buffer, r: Rect, bg: Color) {
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            put(buf, x, y, " ", Style::new().bg(bg));
        }
    }
}

/// A rounded single-line box, so it never reads as pipe.
fn frame(buf: &mut Buffer, r: Rect, title: &str, line: Color, bg: Color) {
    let st = Style::new().fg(line).bg(bg);
    for x in r.left() + 1..r.right() - 1 {
        put(buf, x, r.top(), "─", st);
        put(buf, x, r.bottom() - 1, "─", st);
    }
    for y in r.top() + 1..r.bottom() - 1 {
        put(buf, r.left(), y, "│", st);
        put(buf, r.right() - 1, y, "│", st);
    }
    put(buf, r.left(), r.top(), "╭", st);
    put(buf, r.right() - 1, r.top(), "╮", st);
    put(buf, r.left(), r.bottom() - 1, "╰", st);
    put(buf, r.right() - 1, r.bottom() - 1, "╯", st);
    if !title.is_empty() {
        let padded = if title.chars().count() + 6 <= r.width as usize { format!(" {title} ") } else { title.to_string() };
        text(buf, r.left() + 2, r.top(), &padded, st.add_modifier(Modifier::BOLD));
    }
}

/// Where everything goes on a screen of `area`.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub status: (u16, u16),
    pub queue: Rect,
    pub board_box: Rect,
    /// The board's first tile, top-left.
    pub board: (u16, u16),
    pub fits: bool,
}

pub fn layout(area: Rect) -> Layout {
    let ox = area.x + area.width.saturating_sub(NEED_W) / 2;
    let oy = area.y + area.height.saturating_sub(NEED_H) / 2;
    let board_box = Rect::new(ox + 9, oy + 1, W as u16 * TW + 2, H as u16 * TH + 2);
    Layout {
        status: (ox, oy),
        queue: Rect::new(ox, oy + 1, 8, QUEUE as u16 * TH + 2),
        board_box,
        board: (board_box.x + 1, board_box.y + 1),
        fits: area.width >= NEED_W && area.height >= NEED_H,
    }
}

/// The board cell under screen cell (x, y).
pub fn tile_at(l: &Layout, x: u16, y: u16) -> Option<(usize, usize)> {
    let (bx, by) = l.board;
    let (tx, ty) = ((x.checked_sub(bx)? / TW) as usize, (y.checked_sub(by)? / TH) as usize);
    (tx < W && ty < H).then_some((tx, ty))
}

struct Tile<'a> {
    art: [&'a str; 3],
    label: &'a str,
    wall: Color,
    bg: Color,
    liquid: Color,
}

/// Draw one tile at screen (x0, y0); `lit(x, y)` says which inside pixels hold data.
fn draw_tile(buf: &mut Buffer, x0: u16, y0: u16, tile: &Tile, t: f32, lit: &dyn Fn(i32, i32) -> bool) {
    let mut label = tile.label.chars().chain(std::iter::repeat(' '));
    for (r, row) in tile.art.iter().enumerate() {
        for (c, ch) in row.chars().enumerate() {
            let (sx, sy) = (x0 + c as u16, y0 + r as u16);
            match ch {
                '·' => {
                    let (x, y) = (c as i32, 2 * r as i32);
                    let px = |y: i32| if lit(x, y) { data(sx as i32, sy as i32 * 2 + y % 2, t, tile.liquid) } else { HOLLOW };
                    put(buf, sx, sy, "▀", Style::new().fg(px(y)).bg(px(y + 1)));
                }
                // a rail over a channel: the wall line, the data under it
                '¦' => {
                    let (x, y) = (c as i32, 2 * r as i32);
                    let under = if lit(x, y) { data(sx as i32, sy as i32 * 2, t, tile.liquid) } else { HOLLOW };
                    put(buf, sx, sy, "│", Style::new().fg(tile.wall).bg(under));
                }
                '@' => {
                    let l = label.next().unwrap_or(' ');
                    put(buf, sx, sy, l.encode_utf8(&mut [0; 4]), Style::new().fg(AMBER).bg(tile.bg).add_modifier(Modifier::BOLD));
                }
                ' ' => put(buf, sx, sy, " ", Style::new().bg(tile.bg)),
                _ => put(buf, sx, sy, ch.encode_utf8(&mut [0; 4]), Style::new().fg(tile.wall).bg(tile.bg)),
            }
        }
    }
}

fn draw_piece_cell(buf: &mut Buffer, x0: u16, y0: u16, cell: Cell, cmd: &str, (wall, bg): (Color, Color), clock: f32, t: f32) {
    match cell {
        Cell::Empty => {
            for r in 0..TH {
                for c in 0..TW {
                    put(buf, x0 + c, y0 + r, " ", Style::new().bg(bg));
                }
            }
            put(buf, x0, y0, "·", Style::new().fg(GRID).bg(bg));
        }
        Cell::Locked => {
            for r in 0..TH {
                for c in 0..TW {
                    put(buf, x0 + c, y0 + r, "▒", Style::new().fg(LOCKED).bg(bg));
                }
            }
        }
        Cell::Pipe { piece, arms, wet, fixed } => {
            // the board's own pieces are amber, like the process boxes
            let wall = if fixed == Fixed::No || wall == AMBER { wall } else { mix(AMBER, BG, 0.15) };
            let tile = Tile { art: fixed_art(piece, fixed), label: "buf", wall, bg, liquid: GREEN };
            draw_tile(buf, x0, y0, &tile, t, &|x, y| lit_pipe(x, y, piece, &arms, wet, clock));
        }
        Cell::Source { out, fill } => {
            let tile = Tile { art: box_art(out), label: cmd, wall: AMBER, bg, liquid: GREEN };
            draw_tile(buf, x0, y0, &tile, t, &|x, y| ((1 - depth(x, y, out)) as f32) < fill * 2.0);
        }
        Cell::Sink { inlet, arm } => {
            let fill = arm.map_or(0.0, |a| a.fill(clock));
            let tile = Tile { art: box_art(inlet), label: "nul", wall: AMBER, bg, liquid: GREEN };
            draw_tile(buf, x0, y0, &tile, t, &|x, y| (depth(x, y, inlet) as f32) < fill * 2.0);
        }
    }
}

/// The board, the cursor and any spill. `hide` blanks it (paused).
fn draw_board(buf: &mut Buffer, l: &Layout, g: &Game, t: f32, cursor: bool, hide: bool) {
    frame(buf, l.board_box, "", DIM, BG);
    let (bx, by) = l.board;
    for ty in 0..H {
        for tx in 0..W {
            let (x0, y0) = (bx + tx as u16 * TW, by + ty as u16 * TH);
            let cell = if hide { Cell::Empty } else { g.cell((tx, ty)) };
            let here = cursor && !hide && g.cursor == (tx, ty);
            let (mut wall, mut bg) = if here { (AMBER, CURSOR_BG) } else { (WALL, BG) };
            if let Some((at, left)) = g.boom
                && at == (tx, ty)
            {
                let k = left / crate::engine::REPLACE_LOCK;
                wall = mix(wall, RED, k);
                bg = mix(bg, Color::Rgb(90, 20, 8), k);
            }
            if let Some((_, left)) = g.junked.iter().find(|(p, _)| *p == (tx, ty)) {
                bg = mix(bg, Color::Rgb(120, 24, 10), left / crate::engine::JUNK_FLASH);
            }
            if here && g.lock > 0.0 {
                bg = mix(bg, Color::Rgb(90, 20, 8), 0.6);
            }
            if here && cell == Cell::Empty {
                let ghost = Tile { art: piece_art(g.queue[0]), label: "", wall: GHOST, bg, liquid: GREEN };
                draw_tile(buf, x0, y0, &ghost, t, &|_, _| false);
            } else {
                draw_piece_cell(buf, x0, y0, cell, g.level.cmd, (wall, bg), g.clock, t);
            }
        }
    }
    if let Phase::Spill { at, dir, t: st } = g.phase
        && !hide
    {
        draw_spill(buf, l, at, dir, st, if g.piped >= g.level.required { GREEN } else { RED }, t);
    }
}

/// Data pouring out of an open end: a half-disc of pixels growing past the edge.
fn draw_spill(buf: &mut Buffer, l: &Layout, at: (usize, usize), dir: Dir, st: f32, base: Color, t: f32) {
    let (dx, dy) = dir.delta();
    let ex = at.0 as f32 * 6.0 + 3.0 + dx as f32 * 3.0;
    let ey = at.1 as f32 * 6.0 + 3.0 + dy as f32 * 3.0;
    let r = 1.5 + 5.5 * (st / SPILL_TIME).min(1.0).sqrt();
    let wet = |px: i32, py: i32| {
        let (cx, cy) = (px as f32 + 0.5 - ex, py as f32 + 0.5 - ey);
        cx * dx as f32 + cy * dy as f32 >= 0.0 && cx.hypot(cy) <= r
    };
    let (bx, by) = (l.board.0 as i32, l.board.1 as i32);
    let bb = l.board_box;
    let ri = r.ceil() as i32 + 1;
    let (cx0, cx1) = (ex as i32 - ri, ex as i32 + ri);
    let (cy0, cy1) = ((ey as i32 - ri).div_euclid(2), (ey as i32 + ri).div_euclid(2));
    for cy in cy0..=cy1 {
        for px in cx0..=cx1 {
            let (sx, sy) = (bx + px, by + cy);
            if sx < bb.left() as i32 || sx >= bb.right() as i32 || sy < bb.top() as i32 || sy >= bb.bottom() as i32 {
                continue;
            }
            let (top, bot) = (wet(px, cy * 2), wet(px, cy * 2 + 1));
            let col = data(sx, sy * 2, t, base);
            let (sx, sy) = (sx as u16, sy as u16);
            let under = buf[(sx, sy)].bg;
            match (top, bot) {
                (true, true) => put(buf, sx, sy, "█", Style::new().fg(col).bg(under)),
                (true, false) => put(buf, sx, sy, "▀", Style::new().fg(col).bg(under)),
                (false, true) => put(buf, sx, sy, "▄", Style::new().fg(col).bg(under)),
                _ => {}
            }
        }
    }
}

pub struct Hud {
    /// Top left of the status line: the game's name, or the rival's.
    pub title: String,
    /// The hi score (solo).
    pub hi: Option<i64>,
    /// More status items at the end (versus: the rival, the match).
    pub extra: Vec<(String, String)>,
    pub paused: bool,
    pub cursor: bool,
}

pub fn draw_play(buf: &mut Buffer, area: Rect, g: &Game, hud: &Hud, overlay: Option<&Lines>, t: f32) {
    fill(buf, area, BG);
    let l = layout(area);
    if !l.fits {
        too_small(buf, area);
        return;
    }
    // status line
    let (sx, sy) = l.status;
    text(buf, sx, sy, &hud.title.chars().take(8).collect::<String>(), bold(AMBER));
    let mut x = l.board_box.x + 1;
    let mut item = |label: &str, value: String, col: Color| {
        text(buf, x, sy, label, style(DIM));
        x += label.chars().count() as u16 + 1;
        text(buf, x, sy, &value, bold(col));
        x += value.chars().count() as u16 + 3;
    };
    item("level", g.level.n.to_string(), TEXT);
    let met = g.piped >= g.level.required;
    item("piped", format!("{}/{}", g.piped, g.level.required), if met { GREEN } else { TEXT });
    item("score", g.score.to_string(), TEXT);
    if g.crosses > 0 {
        let col = if g.crosses >= crate::engine::LOOPS_FOR_BONUS { GREEN } else { TEXT };
        item("loops", format!("{}/{}", g.crosses, crate::engine::LOOPS_FOR_BONUS), col);
    }
    if let Some(hi) = hud.hi {
        item("hi", hi.max(g.score).to_string(), DIM);
    }
    for (label, value) in &hud.extra {
        item(label, value.clone(), AMBER);
    }

    // the queue, next piece on top
    frame(buf, l.queue, "next", DIM, BG);
    for i in 0..QUEUE {
        let (x0, y0) = (l.queue.x + 1, l.queue.y + 1 + i as u16 * TH);
        let wall = if hud.paused {
            BG
        } else if i == 0 {
            AMBER
        } else {
            mix(WALL, BG, i as f32 * 0.14)
        };
        draw_tile(buf, x0, y0, &Tile { art: piece_art(g.queue[i]), label: "", wall, bg: BG, liquid: GREEN }, t, &|_, _| false);
    }

    // the flow, under the queue
    let fy = l.queue.bottom() + 1;
    let fx = l.queue.x + 1;
    match g.phase {
        Phase::Countdown { left } => {
            text(buf, fx, fy, "data in", style(DIM));
            text(buf, fx, fy + 1, &format!("{:>4.0}s", left.ceil()), bold(AMBER));
        }
        Phase::Flowing => {
            text(buf, fx, fy, "data", style(DIM));
            let s = if g.fast { "▶▶ fast" } else { "▶ live" };
            text(buf, fx, fy + 1, s, bold(GREEN));
        }
        Phase::Spill { .. } | Phase::Done { .. } => {
            text(buf, fx, fy, "data", style(DIM));
            text(buf, fx, fy + 1, "EPIPE", bold(if met { GREEN } else { RED }));
        }
    }
    text(buf, fx, fy + 3, "F fast", style(DIM));
    text(buf, fx, fy + 4, "P pause", style(DIM));

    draw_board(buf, &l, g, t, hud.cursor, hud.paused);
    if let Some(lines) = overlay {
        panel(buf, l.board_box, lines);
    }
}

/// A box of text centred over `over`.
pub fn panel(buf: &mut Buffer, over: Rect, lines: &Lines) {
    let w = lines.iter().map(|(s, _)| s.chars().count()).max().unwrap_or(0) as u16 + 6;
    let h = lines.len() as u16 + 4;
    let r = Rect::new(over.x + over.width.saturating_sub(w) / 2, over.y + over.height.saturating_sub(h) / 2, w, h);
    fill(buf, r, PANEL_BG);
    frame(buf, r, "", AMBER, PANEL_BG);
    for (i, (s, st)) in lines.iter().enumerate() {
        text(buf, r.x + 3, r.y + 2 + i as u16, s, st.bg(PANEL_BG));
    }
}

fn too_small(buf: &mut Buffer, area: Rect) {
    let y = area.y + area.height / 2;
    centered(buf, area, y.saturating_sub(1), "SIGPIPE needs a bigger terminal", bold(AMBER));
    let s = format!("{}x{} at least, this one is {}x{}", NEED_W, NEED_H, area.width, area.height);
    centered(buf, area, y + 1, &s, style(DIM));
}

/// A screen with the logo and a panel of text (the versus lobby).
pub fn draw_lobby(buf: &mut Buffer, area: Rect, lines: &Lines, t: f32) {
    fill(buf, area, BG);
    let top = area.y + 1;
    for (i, row) in LOGO.iter().enumerate() {
        centered(buf, area, top + i as u16, row, bold(AMBER));
    }
    let sw = 44.min(area.width.saturating_sub(4));
    stream(buf, area.x + (area.width - sw) / 2, top + 4, sw, t);
    let rest = Rect::new(area.x, top + 7, area.width, area.height.saturating_sub(8));
    panel(buf, rest, lines);
}

/// Where a waiting rival's board will be.
pub fn draw_waiting(buf: &mut Buffer, area: Rect, s: &str) {
    fill(buf, area, BG);
    centered(buf, area, area.y + area.height / 2, s, style(DIM));
}

const LOGO: [&str; 3] = ["╔═╗╦╔═╗╔═╗╦╔═╗╔═╗", "╚═╗║║ ╦╠═╝║╠═╝║╣ ", "╚═╝╩╚═╝╩  ╩╩  ╚═╝"];

/// A straight pipe across the screen with packets of data running through it.
fn stream(buf: &mut Buffer, x0: u16, y0: u16, w: u16, t: f32) {
    for c in 0..w {
        put(buf, x0 + c, y0, "═", style(WALL));
        put(buf, x0 + c, y0 + 2, "═", style(WALL));
        let on = (c as f32 - t * 9.0).rem_euclid(7.0) < 4.5;
        let col = if on { data(c as i32, 0, t, GREEN) } else { HOLLOW };
        put(buf, x0 + c, y0 + 1, "▀", Style::new().fg(col).bg(col));
    }
}

const TITLE_W: u16 = 66;
const TITLE_H: u16 = 21;

fn title_top(area: Rect) -> u16 {
    area.y + area.height.saturating_sub(TITLE_H) / 2
}

/// The title's menu: each button's place on screen, its label and what it does.
pub fn title_buttons(area: Rect) -> Vec<(Rect, &'static str, Act)> {
    const ITEMS: [(&str, Act); 4] = [("Enter run", Act::Ok), ("V host versus", Act::Host), ("C join versus", Act::Join), ("Q quit", Act::Quit)];
    let width = |l: &str| l.chars().count() as u16 + 4;
    let total: u16 = ITEMS.iter().map(|(l, _)| width(l)).sum::<u16>() + 2 * (ITEMS.len() as u16 - 1);
    let mut x = area.x + area.width.saturating_sub(total) / 2;
    let y = title_top(area) + 12;
    ITEMS
        .iter()
        .map(|&(label, act)| {
            let r = Rect::new(x, y, width(label), 1);
            x += r.width + 2;
            (r, label, act)
        })
        .collect()
}

/// `hover`: the button under the mouse.
pub fn draw_title(buf: &mut Buffer, area: Rect, hi: i64, last: Option<i64>, hover: Option<usize>, t: f32) {
    fill(buf, area, BG);
    if area.width < TITLE_W || area.height < TITLE_H {
        too_small(buf, area);
        return;
    }
    let top = title_top(area);
    for (i, row) in LOGO.iter().enumerate() {
        centered(buf, area, top + i as u16, row, bold(AMBER));
    }
    centered(buf, area, top + 4, "lay the pipe before the data arrives", style(DIM));
    let sw = 44.min(area.width - 4);
    stream(buf, area.x + (area.width - sw) / 2, top + 6, sw, t);
    let prompt = "$ ./sigpipe";
    let px = area.x + area.width.saturating_sub(prompt.len() as u16 + 1) / 2;
    text(buf, px, top + 10, prompt, bold(GREEN));
    if (t * 2.0) as i64 % 2 == 0 {
        put(buf, px + prompt.len() as u16, top + 10, "▌", style(GREEN));
    }
    for (i, (r, label, _)) in title_buttons(area).into_iter().enumerate() {
        let st = if hover == Some(i) { Style::new().fg(BG).bg(AMBER).add_modifier(Modifier::BOLD) } else { bold(AMBER) };
        text(buf, r.x, r.y, &format!("[ {label} ]"), st);
    }
    let help = [
        "move: arrows, WASD, hjkl or the mouse",
        "lay a piece: Space, Enter or click",
        "fast flow: F    pause: P    mute: M    back: Esc",
        "gamepad: d-pad, A lay, X fast, Start pause",
        "versus is on the LAN: host on one machine, join from another",
    ];
    for (i, h) in help.iter().enumerate() {
        centered(buf, area, top + 14 + i as u16, h, style(DIM));
    }
    let scores = match last {
        Some(s) => format!("last run {s}      hi score {hi}"),
        None => format!("hi score {hi}"),
    };
    centered(buf, area, top + 20, &scores, style(DIM));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe full of data lights exactly the inside of its art; a dry one nothing.
    #[test]
    fn full_pipe_lights_its_inside() {
        use Piece::*;
        for piece in [LeftRight, UpDown, UpRight, UpLeft, DownRight, DownLeft, Cross, TeeNoUp, TeeNoDown, TeeNoLeft, TeeNoRight] {
            let art = piece_art(piece);
            let mut arms = [None; 4];
            for d in Dir::ALL.into_iter().filter(|d| piece.opens(*d)) {
                arms[d as usize] = Some(Arm { start: 0.0, len: 0.5, inward: d == Dir::Up, done: true });
            }
            for y in 0..6 {
                for x in 0..6 {
                    // inside, or under one of the bridge's rails
                    let inside = matches!(art[y as usize / 2].chars().nth(x as usize).unwrap(), '·' | '¦');
                    assert_eq!(lit_pipe(x, y, piece, &arms, true, 10.0), inside, "{piece:?} pixel {x},{y}");
                    assert!(!lit_pipe(x, y, piece, &[None; 4], false, 10.0));
                }
            }
        }
    }

    #[test]
    fn title_buttons_fit_and_do_not_overlap() {
        let area = Rect::new(0, 0, 80, 24);
        let b = title_buttons(area);
        assert!(b.iter().all(|(r, ..)| r.right() <= area.right()));
        assert!(b.windows(2).all(|w| w[0].0.right() < w[1].0.x));
        assert!(b.iter().map(|(r, ..)| r.width).sum::<u16>() + 6 <= TITLE_W);
    }

    #[test]
    fn layout_fits_80x24() {
        let l = layout(Rect::new(0, 0, 80, 24));
        assert!(l.fits);
        assert!(l.board_box.right() <= 80 && l.board_box.bottom() <= 24);
        assert_eq!(tile_at(&l, l.board.0, l.board.1), Some((0, 0)));
        assert_eq!(tile_at(&l, l.board.0 + TW * 10 - 1, l.board.1 + TH * 7 - 1), Some((9, 6)));
    }
}
