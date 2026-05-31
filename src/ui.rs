// ui.rs — one row per phrase

use std::io;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use crate::app::App;

const BG:       Color = Color::Rgb(0, 0, 0);
const BORDER:   Color = Color::Rgb(0, 255, 0);
const ACCENT:   Color = Color::Rgb(0, 255, 0);
const DIM:      Color = Color::Rgb(0, 180, 0);
const CMD:      Color = Color::Rgb(0, 255, 0);
const ERR:      Color = Color::Rgb(255, 80, 80);
const MAQAM:    Color = Color::Rgb(0, 200, 0);
const REPEAT:   Color = Color::Rgb(0, 255, 0);

pub fn run(app: &mut App) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend  = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    loop {
        app.tick();   // poll render thread result; clears rec_rx on completion
        term.draw(|f| draw(f, app))?;

        if event::poll(std::time::Duration::from_millis(40))? {
            if let Event::Key(key) = event::read()? {
                // Any key dismisses the help overlay
                if app.show_help {
                    app.show_help = false;
                    continue;
                }
                if app.show_jins {
                    app.show_jins = false;
                    continue;
                }
                match key.code {
                    KeyCode::Char('c') | KeyCode::Char('q')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    { app.should_quit = true; }

                    KeyCode::Enter => {
                        let cmd = app.input.clone();
                        app.history_push(&cmd);
                        app.input.clear();
                        app.cursor_pos = 0;
                        app.handle_command(&cmd);
                    }
                    KeyCode::Up    => { app.history_up(); }
                    KeyCode::Down  => { app.history_down(); }
                    KeyCode::Left  => { app.cursor_left(); }
                    KeyCode::Right => { app.cursor_right(); }
                    KeyCode::Home  => { app.cursor_home(); }
                    KeyCode::End   => { app.cursor_end(); }
                    KeyCode::Delete => {
                        app.history_pos = None;
                        app.delete_char();
                    }
                    KeyCode::Backspace => {
                        app.history_pos = None;
                        app.backspace();
                    }
                    KeyCode::Char(c) => {
                        app.history_pos = None;
                        app.insert_char(c);
                    }
                    KeyCode::Esc => {
                        app.input.clear();
                        app.cursor_pos  = 0;
                        app.message     = None;
                        app.history_pos = None;
                    }
                    _ => {}
                }
            }
        }
        if app.should_quit { break; }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(ratatui::widgets::Block::default().style(Style::default().bg(BG)), area);
    if app.show_help {
        draw_help(f, area);
        return;
    }
    if app.show_jins {
        draw_jins_list(f, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3), Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    draw_phrases(f, app, chunks[0]);
    draw_input(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);
    draw_recording(f, app, chunks[3]);
}

fn draw_phrases(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let cur = if app.paused { usize::MAX } else {
        crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed)
    };
    let cur_sub   = crate::CUR_SUBDIV.load(std::sync::atomic::Ordering::Relaxed);
    let cur_plays = crate::CUR_PLAYS.load(std::sync::atomic::Ordering::Relaxed);
    let n = app.phrases.len().max(1);

    let items: Vec<ListItem> = app.phrases.iter().enumerate().map(|(idx, phrase)| {
        let playing = !app.paused && idx == cur % n;
        let id_str  = format!("{:>2}: ", phrase.id);
        let marker  = if playing { "▶ " } else { "  " };

        // Jump entries — show live counter for every jump, not just the playing one
        if let Some(ref js) = phrase.jump {
            // Read counter from the shared map (written by audio thread)
            let remaining = crate::jump_counters().lock()
                .ok()
                .and_then(|jc| jc.get(&phrase.id).copied())
                .unwrap_or(js.times.saturating_sub(1));
            let pass    = js.times.saturating_sub(remaining);  // 1-based current pass
            let total   = js.times;
            let counter = format!("  [{}/{}]", pass, total);

            let col_src = if playing {
                Color::Rgb(255, 210, 80)   // bright amber when active
            } else {
                Color::Rgb(110, 95, 40)    // dim amber otherwise
            };
            let col_ctr = if playing {
                Color::Rgb(255, 255, 150)  // bright counter when active
            } else {
                Color::Rgb(160, 140, 70)   // visible but subdued when inactive
            };
            return ListItem::new(Line::from(vec![
                Span::styled(marker,             Style::default().fg(ACCENT).bg(BG).add_modifier(Modifier::BOLD)),
                Span::styled(id_str,             Style::default().fg(DIM).bg(BG)),
                Span::styled(phrase.src.clone(), Style::default().fg(col_src).bg(BG).add_modifier(Modifier::BOLD)),
                Span::styled(counter,            Style::default().fg(col_ctr).bg(BG).add_modifier(Modifier::BOLD)),
            ]));
        }

        let src_str   = format!("{:<28}", phrase.src);
        let rhythm    = phrase.rhythm_display();
        let maqam_str = phrase.bar.ratio_strs.join(" | ");

        let (fg_id, fg_src) = if playing {
            (ACCENT, Color::Rgb(255, 255, 180))
        } else {
            (DIM, ACCENT)
        };

        let mut spans = vec![
            Span::styled(marker,  Style::default().fg(ACCENT).bg(BG).add_modifier(Modifier::BOLD)),
            Span::styled(id_str,  Style::default().fg(fg_id).bg(BG)),
            Span::styled(src_str, Style::default().fg(fg_src).bg(BG)),
            Span::raw(" "),
        ];

        for (si, ch) in rhythm.chars().enumerate() {
            let is_now = playing && si == cur_sub;
            let sty = if is_now {
                // Active beat: black text on bright white — unmistakable
                Style::default()
                    .fg(Color::Rgb(0, 0, 0))
                    .bg(Color::Rgb(255, 255, 255))
                    .add_modifier(Modifier::BOLD)
            } else if playing {
                // Playing phrase, other beats: dim green so active stands out
                let col = match ch { 'X' => Color::Rgb(0, 160, 0), _ => Color::Rgb(0, 100, 0) };
                Style::default().fg(col).bg(BG)
            } else {
                // Inactive phrase: gray
                Style::default().fg(Color::Rgb(80, 80, 80)).bg(BG)
            };
            spans.push(Span::styled(ch.to_string(), sty));
        }

        if playing && phrase.repeat > 1 {
            spans.push(Span::styled(
                format!(" {}/{}", cur_plays + 1, phrase.repeat),
                Style::default().fg(Color::Rgb(180,180,100)).bg(BG).add_modifier(Modifier::BOLD),
            ));
        } else if !playing && phrase.repeat > 1 {
            spans.push(Span::styled(
                format!("  ×{}", phrase.repeat),
                Style::default().fg(REPEAT).bg(BG),
            ));
        }

        spans.push(Span::styled(
            format!("  {maqam_str}"),
            Style::default().fg(MAQAM).bg(BG).add_modifier(if playing { Modifier::empty() } else { Modifier::DIM }),
        ));

        ListItem::new(Line::from(spans))
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" maqam-live ", Style::default().fg(ACCENT).bg(BG).add_modifier(Modifier::BOLD)))
            .border_style(Style::default().fg(BORDER).bg(BG))
            .style(Style::default().bg(BG)));
    f.render_widget(list, area);
}

fn draw_input(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let chars: Vec<char> = app.input.chars().collect();
    let mut spans = vec![Span::styled("> ", Style::default().fg(DIM).bg(BG))];
    for &ch in chars.iter() {
        spans.push(Span::styled(ch.to_string(), Style::default().fg(CMD).bg(BG)));
    }
    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(BG))
        .block(Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" cmd ", Style::default().fg(DIM).bg(BG)))
            .border_style(Style::default().fg(BORDER).bg(BG)));
    f.render_widget(para, area);

    // Position the real terminal cursor — blinking block, always visible.
    // area.x + 1 (border) + 2 ("> ") + cursor_pos columns
    let cx = area.x + 1 + 2 + app.cursor_pos as u16;
    let cy = area.y + 1; // +1 for top border
    f.set_cursor_position((cx, cy));
}

fn draw_status(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = if let Some(msg) = &app.message {
        let col = if msg.starts_with('✗') { ERR } else { DIM };
        Line::from(vec![Span::styled(format!("  {msg}"), Style::default().fg(col).bg(BG))])
    } else {
        Line::from(vec![Span::styled(
            format!("  {}BPM:{} sus:{:.1}s vol:{:.2} phrases:{}  [?] help  [z] pause",
                if app.paused { "⏸ PAUSED  " } else { "" },
                app.bpm, app.sustain, app.vol, app.phrases.len()),
            Style::default().fg(DIM).bg(BG),
        )])
    };
    f.render_widget(Paragraph::new(text).style(Style::default().bg(BG)), area);
}

fn draw_recording(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    use std::sync::atomic::Ordering::Relaxed;

    let active = crate::REC_ACTIVE.load(Relaxed);
    let text = if active {
        let done  = crate::REC_SAMPLES_DONE.load(Relaxed);
        let total = crate::REC_SAMPLES_TOTAL.load(Relaxed).max(1);
        let pct   = (done * 100 / total).min(100);

        // bar width: total area minus fixed decorations ("  ◉ rendering [" + "] 100%" = 22 chars)
        let bar_w = (area.width as usize).saturating_sub(22).max(4);
        let filled = bar_w * pct / 100;
        let bar: String = std::iter::repeat('=').take(filled)
            .chain(std::iter::once('>').take(if filled < bar_w { 1 } else { 0 }))
            .chain(std::iter::repeat(' ').take(bar_w.saturating_sub(filled + 1)))
            .collect();

        Line::from(vec![
            Span::styled("  ◉ rendering [",
                Style::default().fg(Color::Rgb(200, 80, 80))),
            Span::styled(bar,
                Style::default().fg(Color::Rgb(0, 200, 100))),
            Span::styled(format!("] {pct:>3}%"),
                Style::default().fg(Color::Rgb(160, 160, 180))),
        ])
    } else {
        match &app.last_recording {
            Some(path) => Line::from(vec![
                Span::styled("  ◉ ",
                    Style::default().fg(Color::Rgb(200, 80, 80))),
                Span::styled(path.as_str(),
                    Style::default().fg(Color::Rgb(160, 160, 180))),
            ]),
            None => Line::from(vec![
                Span::styled("  m → record cycle to ~/maqam-<ts>.mp4",
                    Style::default().fg(Color::Rgb(55, 55, 70))),
            ]),
        }
    };
    f.render_widget(Paragraph::new(text).style(Style::default().bg(BG)), area);
}

fn draw_help(f: &mut Frame, area: ratatui::layout::Rect) {
    use ratatui::widgets::{Block, Borders, Paragraph};
    use ratatui::text::{Line, Span};

    let green   = Style::default().fg(ACCENT).bg(BG);
    let bright  = Style::default().fg(Color::Rgb(0,255,0)).bg(BG).add_modifier(Modifier::BOLD);
    let dim     = Style::default().fg(DIM).bg(BG);
    let heading = Style::default().fg(Color::Rgb(0,255,0)).bg(BG).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

    let lines: Vec<Line> = vec![
        Line::from(vec![Span::styled("  maqam-live — command reference", heading)]),
        Line::from(vec![Span::styled("  press any key to close", dim)]),
        Line::from(vec![Span::raw("")]),

        Line::from(vec![Span::styled("  ADD A PHRASE", bright)]),
        Line::from(vec![Span::styled("  <root> <maqam> [groups] [, <root> <maqam>…] [r<N>]", green)]),
        Line::from(vec![Span::styled("    roots:   c  d  e  f  g  a  b   (append + or - for sharp/flat)", dim)]),
        Line::from(vec![Span::styled("    maqams:  nah  bay  hij  ras  kur  sab  aja  nik  suz  jih", dim)]),
        Line::from(vec![Span::styled("             nahawand bayati hijaz rast kurd saba ajam nikriz suznak jiharkah", dim)]),
        Line::from(vec![Span::styled("    groups:  332  44  3322  4431  (additive 8th-note rhythm)", dim)]),
        Line::from(vec![Span::styled("    r<N>:    r4 = repeat 4 times before advancing", dim)]),
        Line::from(vec![Span::styled("    comma:   stack ajnas into one scale  (d bay, a nah)", dim)]),
        Line::from(vec![Span::raw("")]),

        Line::from(vec![Span::styled("  SEQUENCE CONTROL", bright)]),
        Line::from(vec![Span::styled("  j <id> [n]              ", green), Span::styled("jump to phrase id, n times total", dim)]),
        Line::from(vec![Span::styled("  i <id> <phrase>         ", green), Span::styled("insert phrase before id", dim)]),
        Line::from(vec![Span::styled("  i <id> j <target> [n]   ", green), Span::styled("insert jump entry before id", dim)]),
        Line::from(vec![Span::styled("  x <id> [id…]            ", green), Span::styled("delete by id  (blocked if playing)", dim)]),
        Line::from(vec![Span::styled("  edit <id> <phrase>      ", green), Span::styled("replace phrase content  (blocked if playing)", dim)]),
        Line::from(vec![Span::styled("  edit <id> j <tgt> [n]   ", green), Span::styled("replace phrase with jump entry", dim)]),
        Line::from(vec![Span::styled("  rot                     ", green), Span::styled("move last phrase to front", dim)]),
        Line::from(vec![Span::raw("")]),

        Line::from(vec![Span::styled("  SETTINGS", bright)]),
        Line::from(vec![Span::styled("  bpm <n>   ", green), Span::styled("tempo (20–400)    ", dim),
                        Span::styled("  s <n>     ", green), Span::styled("sustain seconds", dim)]),
        Line::from(vec![Span::styled("  vol <n>   ", green), Span::styled("volume (0–2)      ", dim),
                        Span::styled("  z         ", green), Span::styled("toggle pause", dim)]),
        Line::from(vec![Span::styled("  z <id>    ", green), Span::styled("seek to phrase id (no pause toggle)", dim)]),
        Line::from(vec![Span::raw("")]),

        Line::from(vec![Span::styled("  RECORDING", bright)]),
        Line::from(vec![Span::styled("  m [n]    ", green), Span::styled("record n cycles to ~/maqam-<ts>.mp4", dim)]),
        Line::from(vec![Span::raw("")]),

        Line::from(vec![Span::styled("  OTHER", bright)]),
        Line::from(vec![Span::styled("  clear    ", green), Span::styled("remove all phrases    ", dim),
                        Span::styled("  q / Ctrl-C  ", green), Span::styled("quit", dim)]),
        Line::from(vec![Span::styled("  ;        ", green), Span::styled("separate multiple commands on one line", dim)]),
        Line::from(vec![Span::raw("")]),

        Line::from(vec![Span::styled("  Music theory: maqamworld.com", dim)]),
        Line::from(vec![Span::styled("  Source:       https://github.com/rfielding/maqam", dim)]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT).bg(BG))
        .style(Style::default().bg(BG));

    let para = Paragraph::new(lines)
        .block(block)
        .style(Style::default().fg(ACCENT).bg(BG));

    f.render_widget(para, area);
}

fn draw_jins_list(f: &mut Frame, area: ratatui::layout::Rect) {
    use ratatui::widgets::{Block, Borders, Paragraph};
    use ratatui::text::{Line, Span};
    use crate::tuning::Maqam;

    let name_col = Style::default().fg(ACCENT).bg(BG);
    let rat_col  = Style::default().fg(DIM).bg(BG);
    let heading  = Style::default().fg(Color::Rgb(0,255,0)).bg(BG)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let dim      = Style::default().fg(DIM).bg(BG);

    let mut lines = vec![
        Line::from(vec![Span::styled("  maqam-live — jins registry", heading)]),
        Line::from(vec![Span::styled("  press any key to close", dim)]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::styled(
            "  create <Name> <p/q> …   delete <Name>   ls",
            Style::default().fg(Color::Rgb(0,160,0)).bg(BG),
        )]),
        Line::from(vec![Span::raw("")]),
    ];

    for (name, ratios) in Maqam::list_all() {
        let rat_str = ratios.iter()
            .map(|&(p,q)| format!("{p}/{q}"))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14}", name), name_col),
            Span::styled(rat_str, rat_col),
        ]));
    }

    let para = Paragraph::new(lines)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT).bg(BG))
            .style(Style::default().bg(BG)))
        .style(Style::default().fg(ACCENT).bg(BG));

    f.render_widget(para, area);
}
