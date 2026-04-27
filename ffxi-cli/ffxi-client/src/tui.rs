//! ratatui front-end — renders `SessionState` for human inspection.
//!
//! The TUI is a *view* on the session, not a controller. Keybinds emit
//! `AgentCommand`s onto the same channel an agent uses, so the TUI and the
//! JSON sidechannel are functionally identical front-ends sharing one
//! Session actor (failure mode #5 mitigation: dedicated tokio task for the
//! UDP socket; TUI subscribes via watch/broadcast).

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction as LayoutDirection, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use tokio::sync::{mpsc, watch};

use crate::state::{AgentCommand, ChatChannel, EntityKind, SessionState, Stage};

pub async fn run(
    state_rx: watch::Receiver<SessionState>,
    cmd_tx: mpsc::Sender<AgentCommand>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Try to enable the kitty keyboard protocol so we can read true key
    // release events. Most modern terminals (kitty, foot, WezTerm, recent
    // iTerm2) support this; macOS Terminal.app does not. If it fails we fall
    // back to press-only semantics — the keybinds still work, just one step
    // per keypress instead of continuous movement while held.
    let kitty_ok = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    )
    .is_ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = render_loop(&mut terminal, state_rx, cmd_tx, kitty_ok).await;

    if kitty_ok {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;
    terminal.show_cursor()?;
    result
}

/// Step distance per W/S press, in game units. FFXI's `pos_x`/`pos_y` are
/// in raw world units; running speed is ~5 u/s, so 1 unit/keypress feels
/// like a deliberate step rather than a sprint.
const MOVE_STEP: f32 = 1.0;

/// Per-tick movement when a key is held. Smaller than MOVE_STEP because in
/// continuous mode we apply movement every tick (~50ms), not once per press.
/// 0.25 u/tick × 20 ticks/s = 5 u/s — matches FFXI's normal run speed.
const MOVE_STEP_HELD: f32 = 0.25;

/// Heading delta per A/D press (one-shot mode).
const ROTATE_STEP: u8 = 8;

/// Heading delta per tick when A/D is held. ~3 °/tick at 50ms ticks → ~60°/s.
const ROTATE_STEP_HELD: u8 = 2;

/// Tick interval for held-key dispatch. 50ms = 20 Hz; the keepalive (1 Hz)
/// will only see the latest position, so this rate is purely for the local
/// movement integration — server bandwidth is unchanged.
const TICK_MS: u64 = 50;

/// In fallback mode (no kitty release events), we treat any direction we
/// haven't seen for `FALLBACK_HOLD_MS` as released. Terminal auto-repeat
/// fires faster than this once it kicks in, but the initial ~500ms delay
/// before auto-repeat starts means held-from-cold won't feel continuous —
/// that's a terminal limitation we surface to the user via the diagnostics
/// hint.
const FALLBACK_HOLD_MS: u128 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HeldDir {
    Forward,
    Back,
    Left,
    Right,
}

/// One-shot inputs (Tab, Enter, Quit). Held movement is dispatched separately
/// from the held-set, not as InputAction events.
#[derive(Debug, Clone, Copy)]
enum InputAction {
    Quit,
    TabTarget,
    EnterAction,
    /// Movement nudge — used only in fallback (non-kitty) mode where each
    /// press is a discrete step. In kitty mode we drive movement entirely
    /// from `held` on tick.
    Nudge(HeldDir),
}

async fn render_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut state_rx: watch::Receiver<SessionState>,
    cmd_tx: mpsc::Sender<AgentCommand>,
    kitty_ok: bool,
) -> Result<()> {
    let tick = Duration::from_millis(TICK_MS);
    let mut target_index: Option<usize> = None;
    let mut last_action_hint: Option<String> = None;
    let mut held: HashMap<HeldDir, Instant> = HashMap::new();
    loop {
        let state = state_rx.borrow_and_update().clone();
        terminal.draw(|f| {
            draw(
                f,
                &state,
                target_index,
                last_action_hint.as_deref(),
                kitty_ok,
                &held,
            )
        })?;

        tokio::select! {
            res = state_rx.changed() => {
                if res.is_err() {
                    return Ok(());
                }
            }
            _ = tokio::time::sleep(tick) => {
                // Drain *all* pending input events before doing the tick step
                // — terminal auto-repeat under heavy keymashing can otherwise
                // flood the queue and outpace our render rate.
                let mut quit = false;
                while let Some(action) = poll_input(kitty_ok, &mut held)? {
                    match action {
                        InputAction::Quit => { quit = true; break; }
                        InputAction::TabTarget => {
                            target_index = next_target(&state, target_index);
                            last_action_hint = target_index
                                .and_then(|i| state.entities.get(i))
                                .map(|e| format!(
                                    "target: {} ({})",
                                    e.name.as_deref().unwrap_or("?"),
                                    entity_kind_label(e.kind),
                                ));
                        }
                        InputAction::EnterAction => {
                            last_action_hint = send_enter_action(&state, target_index, &cmd_tx).await;
                        }
                        InputAction::Nudge(dir) => {
                            // Fallback (non-kitty) one-shot movement.
                            send_held_movement(&state, &[dir], MOVE_STEP, ROTATE_STEP, &cmd_tx).await;
                        }
                    }
                }
                if quit {
                    let _ = cmd_tx.send(AgentCommand::Disconnect).await;
                    return Ok(());
                }
                if kitty_ok && !held.is_empty() {
                    let dirs: Vec<HeldDir> = held.keys().copied().collect();
                    send_held_movement(&state, &dirs, MOVE_STEP_HELD, ROTATE_STEP_HELD, &cmd_tx).await;
                }
                // GC stale held entries. In kitty mode this is a safety net
                // for the case where the terminal claimed REPORT_EVENT_TYPES
                // but never actually delivers Release — without GC the
                // character would slide forever after the user lets go. The
                // longer threshold gives terminals plenty of time to send a
                // real release before we time it out.
                let threshold = if kitty_ok { 500 } else { FALLBACK_HOLD_MS };
                held.retain(|_, t| t.elapsed().as_millis() < threshold);
            }
        }
    }
}

/// Compute the action hint for an Enter press: send an Action(Talk) on the
/// currently-targeted entity if any, otherwise surface a help string.
async fn send_enter_action(
    state: &SessionState,
    target_index: Option<usize>,
    cmd_tx: &mpsc::Sender<AgentCommand>,
) -> Option<String> {
    let Some(e) = target_index.and_then(|i| state.entities.get(i)) else {
        return Some("[enter] no target — press Tab to cycle nearby entities".to_string());
    };
    let cmd = AgentCommand::Action {
        target_id: e.id,
        target_index: e.act_index,
        action_id: ffxi_proto::map::action_id::TALK,
    };
    let _ = cmd_tx.send(cmd).await;
    Some(format!(
        "talked to {} (id={:#x} idx={})",
        e.name.as_deref().unwrap_or("?"),
        e.id,
        e.act_index,
    ))
}

/// Translate a set of held directions into one cumulative `Move` command.
/// Forward+Back cancel; Left+Right cancel. This is what makes diagonal
/// movement feel natural: holding W+D walks at the heading rotated +45°
/// per tick, smoothly arcing the character through the world.
async fn send_held_movement(
    state: &SessionState,
    dirs: &[HeldDir],
    move_step: f32,
    rotate_step: u8,
    cmd_tx: &mpsc::Sender<AgentCommand>,
) {
    let mut forward: i32 = 0;
    let mut rotate: i32 = 0;
    for d in dirs {
        match d {
            HeldDir::Forward => forward += 1,
            HeldDir::Back => forward -= 1,
            HeldDir::Left => rotate -= 1,
            HeldDir::Right => rotate += 1,
        }
    }
    let mut heading = state.self_pos.heading;
    if rotate != 0 {
        let delta = (rotate_step as i32 * rotate).rem_euclid(256) as u8;
        heading = state.self_pos.heading.wrapping_add(delta);
    }
    let (mut x, mut y) = (state.self_pos.pos.x, state.self_pos.pos.y);
    if forward != 0 {
        let (fx, fy) = heading_to_forward(heading);
        let dist = move_step * forward as f32;
        x += fx * dist;
        y += fy * dist;
    }
    if forward == 0 && rotate == 0 {
        return;
    }
    let _ = cmd_tx
        .send(AgentCommand::Move {
            x,
            y,
            z: state.self_pos.pos.z,
            heading,
        })
        .await;
}

/// Compute (dx, dy) for "1 unit forward at heading h". FFXI heading is u8
/// where 0 = +y axis, increasing clockwise (viewed from above), wrapping at
/// 256 = 360°. The minimap puts +y up, so heading 0 is "up on screen".
fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.sin(), angle.cos())
}

/// Cycle to the next entity by current-distance-from-self order.
fn next_target(state: &SessionState, current: Option<usize>) -> Option<usize> {
    if state.entities.is_empty() {
        return None;
    }
    let mut indexed: Vec<(usize, f32)> = state
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let dx = e.pos.x - state.self_pos.pos.x;
            let dy = e.pos.y - state.self_pos.pos.y;
            (i, dx * dx + dy * dy)
        })
        .collect();
    indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let order: Vec<usize> = indexed.iter().map(|(i, _)| *i).collect();
    match current {
        None => Some(order[0]),
        Some(cur) => {
            let pos = order.iter().position(|&i| i == cur).unwrap_or(order.len());
            Some(order[(pos + 1) % order.len()])
        }
    }
}

fn entity_kind_label(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "pc",
        EntityKind::Npc => "npc",
        EntityKind::Mob => "mob",
        EntityKind::Pet => "pet",
        EntityKind::Other => "?",
    }
}

fn entity_color(kind: EntityKind) -> Color {
    match kind {
        EntityKind::Pc => Color::Cyan,
        EntityKind::Npc => Color::White,
        EntityKind::Mob => Color::Red,
        EntityKind::Pet => Color::Yellow,
        EntityKind::Other => Color::DarkGray,
    }
}

fn entity_glyph(kind: EntityKind) -> char {
    match kind {
        EntityKind::Pc => 'P',
        EntityKind::Npc => '◇',
        EntityKind::Mob => 'M',
        EntityKind::Pet => 'p',
        EntityKind::Other => '·',
    }
}

/// Read up to one input event without blocking. Updates `held` based on
/// Press/Release events when `kitty` is true; in fallback mode every Press
/// returns an `InputAction::Nudge` immediately (one step per press) AND
/// refreshes the held timestamp so that auto-repeat looks like "still held."
fn poll_input(
    kitty: bool,
    held: &mut HashMap<HeldDir, Instant>,
) -> Result<Option<InputAction>> {
    if !event::poll(Duration::from_millis(0))? {
        return Ok(None);
    }
    let Event::Key(k) = event::read()? else {
        return Ok(None);
    };
    let dir = match k.code {
        KeyCode::Char('w') | KeyCode::Up => Some(HeldDir::Forward),
        KeyCode::Char('s') | KeyCode::Down => Some(HeldDir::Back),
        KeyCode::Char('a') | KeyCode::Left => Some(HeldDir::Left),
        KeyCode::Char('d') | KeyCode::Right => Some(HeldDir::Right),
        _ => None,
    };
    if let Some(d) = dir {
        match k.kind {
            KeyEventKind::Press | KeyEventKind::Repeat => {
                held.insert(d, Instant::now());
                if !kitty {
                    return Ok(Some(InputAction::Nudge(d)));
                }
                return Ok(None);
            }
            KeyEventKind::Release => {
                held.remove(&d);
                return Ok(None);
            }
        }
    }
    if k.kind != KeyEventKind::Press && k.kind != KeyEventKind::Repeat {
        return Ok(None);
    }
    Ok(match k.code {
        KeyCode::Char('q') | KeyCode::Esc => Some(InputAction::Quit),
        KeyCode::Tab => Some(InputAction::TabTarget),
        KeyCode::Enter => Some(InputAction::EnterAction),
        _ => None,
    })
}

fn draw(
    f: &mut ratatui::Frame,
    state: &SessionState,
    target_index: Option<usize>,
    action_hint: Option<&str>,
    kitty_ok: bool,
    held: &HashMap<HeldDir, Instant>,
) {
    let chunks = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Length(3), // stage bar
            Constraint::Min(8),    // body
            Constraint::Length(3), // diagnostics footer
        ])
        .split(f.area());

    draw_stage_bar(f, chunks[0], state);
    let body = Layout::default()
        .direction(LayoutDirection::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);
    let left = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(body[0]);
    draw_minimap(f, left[0], state, target_index, held);
    draw_world(f, left[1], state, target_index);
    draw_chat(f, body[1], state);
    draw_diagnostics(f, chunks[2], state, action_hint, kitty_ok);
}

/// Render a top-down ASCII minimap centered on `self_pos`. Each cell is a
/// `Span<'static>` so we can color individual glyphs. Entity names float one
/// row above the entity glyph (or below, if at the top edge), truncated to
/// avoid spilling off the side or colliding with adjacent labels.
fn draw_minimap(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &SessionState,
    target_index: Option<usize>,
    held: &HashMap<HeldDir, Instant>,
) {
    let title = format!(
        "  map  hdg {:>3}  pos ({:>6.1}, {:>6.1})  ",
        state.self_pos.heading, state.self_pos.pos.x, state.self_pos.pos.y,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(title, Style::default().fg(Color::Gray)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let w = inner.width as usize;
    let h = inner.height as usize;
    if w < 6 || h < 6 {
        return;
    }
    let cx = w / 2;
    let cy = h / 2;
    // Terminal cells are ~2x as tall as wide. Empirically scale_y ≈ 2 × scale_x
    // produces a square aspect ratio for FFXI's world units.
    let scale_x = 1.5_f32;
    let scale_y = 3.0_f32;

    // Default cell: a faint dot for the grid background, with cross-hairs at
    // center to ground the eye.
    let mut grid: Vec<Vec<Span<'static>>> = (0..h)
        .map(|_| {
            (0..w)
                .map(|_| Span::styled(" ".to_string(), Style::default().fg(Color::DarkGray)))
                .collect()
        })
        .collect();
    // Cross-hairs through center for orientation.
    for col in 0..w {
        if grid[cy][col].content == " " {
            grid[cy][col] = Span::styled("·".to_string(), Style::default().fg(Color::DarkGray));
        }
    }
    for row in 0..h {
        if grid[row][cx].content == " " {
            grid[row][cx] = Span::styled("·".to_string(), Style::default().fg(Color::DarkGray));
        }
    }

    // First pass: place entity glyphs. `placed` records (entity_index, col,
    // row) for the second-pass label placement; `claimed` is the set of cells
    // that contain a glyph and must not be overwritten by a label.
    let mut placed: Vec<(usize, i32, i32)> = Vec::new();
    let mut claimed: HashSet<(i32, i32)> = HashSet::new();
    for (i, e) in state.entities.iter().enumerate() {
        let dx = e.pos.x - state.self_pos.pos.x;
        let dy = e.pos.y - state.self_pos.pos.y;
        let col = (cx as f32 + dx / scale_x).round() as i32;
        let row = (cy as f32 - dy / scale_y).round() as i32;
        if col < 0 || col >= w as i32 || row < 0 || row >= h as i32 {
            continue;
        }
        let is_target = Some(i) == target_index;
        let (glyph, color, modifier) = if is_target {
            ('★', Color::Yellow, Modifier::BOLD)
        } else {
            (entity_glyph(e.kind), entity_color(e.kind), Modifier::empty())
        };
        grid[row as usize][col as usize] = Span::styled(
            glyph.to_string(),
            Style::default().fg(color).add_modifier(modifier),
        );
        placed.push((i, col, row));
        claimed.insert((col, row));
    }
    // Center cell is reserved for self glyph in the final pass.
    claimed.insert((cx as i32, cy as i32));

    // Second pass: float entity name labels one row above the glyph. Labels
    // get truncated to whatever width is available without colliding with
    // a neighbor's glyph cell or another already-drawn label.
    for (i, col, row) in &placed {
        let e = &state.entities[*i];
        let Some(name) = e.name.as_deref() else {
            continue;
        };
        let label_row = if *row > 0 { row - 1 } else { row + 1 };
        if label_row < 0 || label_row >= h as i32 {
            continue;
        }
        let max_label_len = name.chars().count().min(12);
        // Center the label on the entity column but pull it inside the grid
        // bounds at the edges.
        let mut start_col = col - (max_label_len as i32) / 2;
        if start_col < 0 {
            start_col = 0;
        }
        if start_col + max_label_len as i32 > w as i32 {
            start_col = w as i32 - max_label_len as i32;
        }
        let label_color = if Some(*i) == target_index {
            Color::Yellow
        } else {
            entity_color(e.kind)
        };
        for (offset, ch) in name.chars().take(max_label_len).enumerate() {
            let c = start_col + offset as i32;
            if c < 0 || c >= w as i32 {
                continue;
            }
            // Don't overwrite an entity glyph (or the reserved self cell)
            // already placed at this position.
            if claimed.contains(&(c, label_row)) {
                continue;
            }
            grid[label_row as usize][c as usize] = Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(label_color)
                    .add_modifier(Modifier::DIM),
            );
        }
    }

    // Self glyph last so it always wins the center cell.
    grid[cy][cx] = Span::styled(
        "●".to_string(),
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    );
    // Heading indicator one cell ahead, in 8 octants. Each octant is 32/256
    // (= 45°) wide; the buckets center on the cardinals and diagonals so a
    // small heading change visibly rotates the arrow even before crossing a
    // 90° quadrant boundary.
    //
    // (arrow, dx, dy, contributing held-cardinals for the "lit" check)
    let (arrow, dx, dy, lit_a, lit_b) = match state.self_pos.heading {
        h if h < 16 || h >= 240 => ('↑', 0i32, -1i32, HeldDir::Forward, HeldDir::Forward),
        h if (16..48).contains(&h) => ('↗', 1, -1, HeldDir::Forward, HeldDir::Right),
        h if (48..80).contains(&h) => ('→', 1, 0, HeldDir::Right, HeldDir::Right),
        h if (80..112).contains(&h) => ('↘', 1, 1, HeldDir::Right, HeldDir::Back),
        h if (112..144).contains(&h) => ('↓', 0, 1, HeldDir::Back, HeldDir::Back),
        h if (144..176).contains(&h) => ('↙', -1, 1, HeldDir::Back, HeldDir::Left),
        h if (176..208).contains(&h) => ('←', -1, 0, HeldDir::Left, HeldDir::Left),
        _ => ('↖', -1, -1, HeldDir::Left, HeldDir::Forward),
    };
    let ax = cx as i32 + dx;
    let ay = cy as i32 + dy;
    if ax >= 0 && ax < w as i32 && ay >= 0 && ay < h as i32 {
        // Light the arrow when *either* contributing cardinal is held — for
        // diagonals (e.g., ↗ = Forward + Right) holding either key counts as
        // "input matches movement direction."
        let lit = held.contains_key(&lit_a) || held.contains_key(&lit_b);
        let color = if lit { Color::LightYellow } else { Color::Cyan };
        grid[ay as usize][ax as usize] = Span::styled(
            arrow.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
    }

    let lines: Vec<Line> = grid.into_iter().map(Line::from).collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_stage_bar(f: &mut ratatui::Frame, area: Rect, state: &SessionState) {
    let (label, color) = match state.stage {
        Stage::Idle => ("idle", Color::DarkGray),
        Stage::Authenticating => ("auth", Color::Yellow),
        Stage::LobbyHandshake => ("lobby", Color::Yellow),
        Stage::MapBootstrap => ("map-bootstrap", Color::Yellow),
        Stage::Zoning => ("zoning", Color::Yellow),
        Stage::InZone => ("in-zone", Color::Green),
        Stage::Disconnected => ("disconnected", Color::Red),
    };
    let charname = state.character.as_deref().unwrap_or("(no char)");
    let zone = state
        .zone_id
        .map(|z| format!("zone {z}"))
        .unwrap_or_else(|| "—".into());
    let line = Line::from(vec![
        Span::styled(
            "▌ ffxi-client ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled("● ", Style::default().fg(color)),
        Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::raw(format!("  ▪  {charname}  ▪  {zone}")),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_world(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &SessionState,
    target_index: Option<usize>,
) {
    let mut by_dist: Vec<(usize, f32)> = state
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let dx = e.pos.x - state.self_pos.pos.x;
            let dy = e.pos.y - state.self_pos.pos.y;
            (i, (dx * dx + dy * dy).sqrt())
        })
        .collect();
    by_dist.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let max_rows = (area.height as usize).saturating_sub(2);
    let mut items: Vec<ListItem> = Vec::with_capacity(max_rows);
    for (i, dist) in by_dist.into_iter().take(max_rows) {
        let e = &state.entities[i];
        let name = e.name.as_deref().unwrap_or("?");
        let hp = e.hp_pct.map(|p| format!(" hp{p:>3}%")).unwrap_or_default();
        let is_target = Some(i) == target_index;
        let marker = if is_target { '★' } else { ' ' };
        let kind = entity_kind_label(e.kind);
        let line = format!("{marker} {kind:<3} {name:<14} d={dist:5.1}{hp}");
        let style = if is_target {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(entity_color(e.kind))
        };
        items.push(ListItem::new(Line::from(Span::styled(line, style))));
    }
    let title = format!("  nearby ({})  ", state.entities.len());
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(title, Style::default().fg(Color::Gray))),
    );
    f.render_widget(list, area);
}

fn draw_chat(f: &mut ratatui::Frame, area: Rect, state: &SessionState) {
    let max_lines = (area.height as usize).saturating_sub(2);
    let lines: Vec<ListItem> = state
        .chat
        .iter()
        .rev()
        .take(max_lines)
        .rev()
        .map(|line| {
            let (tag, color) = match line.channel {
                ChatChannel::Say => ("[say]", Color::White),
                ChatChannel::Shout => ("[sho]", Color::Cyan),
                ChatChannel::Tell => ("[tll]", Color::Magenta),
                ChatChannel::Party => ("[pty]", Color::Blue),
                ChatChannel::Linkshell => ("[lin]", Color::Green),
                ChatChannel::Yell => ("[yel]", Color::Yellow),
                ChatChannel::System => ("[sys]", Color::Gray),
                ChatChannel::Other => ("[---]", Color::DarkGray),
            };
            let body = format!(" {}: {}", line.sender, line.text);
            ListItem::new(Line::from(vec![
                Span::styled(tag, Style::default().fg(color)),
                Span::raw(body),
            ]))
        })
        .collect();
    let list = List::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled("  chat  ", Style::default().fg(Color::Gray))),
    );
    f.render_widget(list, area);
}

fn draw_diagnostics(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &SessionState,
    action_hint: Option<&str>,
    kitty_ok: bool,
) {
    let d = &state.diagnostics;
    let bf = match d.blowfish_status {
        Some(crate::state::BlowfishStatus::Accepted) => "ok".green(),
        Some(s) => format!("{s:?}").yellow(),
        None => "-".dark_gray(),
    };
    let sync = match (d.sync_in, d.sync_out) {
        (Some(i), Some(o)) => format!("{i}/{o}"),
        _ => "-".into(),
    };
    let age = match d.last_server_packet_age_ms {
        Some(ms) if ms < 5_000 => format!("{ms}ms").into(),
        Some(ms) => format!("{ms}ms").red(),
        None => "-".into(),
    };
    let map = d.map_server_addr.clone().unwrap_or_else(|| "-".into());
    let default_hint = if kitty_ok {
        "[wasd] hold-to-move  [tab] target  [enter] talk  [q]uit"
    } else {
        "[wasd] step  [tab] target  [enter] talk  [q]uit  (kitty off — single-step)"
    };
    let hint = action_hint.unwrap_or(default_hint);
    let line = Line::from(vec![
        Span::styled("bf=", Style::default().fg(Color::DarkGray)),
        bf,
        Span::styled("  sync=", Style::default().fg(Color::DarkGray)),
        Span::raw(sync),
        Span::styled("  last=", Style::default().fg(Color::DarkGray)),
        age,
        Span::styled("  map=", Style::default().fg(Color::DarkGray)),
        Span::raw(map),
        Span::raw("    "),
        Span::styled(hint.to_string(), Style::default().fg(Color::Cyan)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        Paragraph::new(line).block(block).wrap(Wrap { trim: true }),
        area,
    );
}
