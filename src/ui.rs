// ui.rs — one row per phrase

use crate::app::App;
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
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;

const BG: Color = Color::Rgb(0, 0, 0);
const BORDER: Color = Color::Rgb(0, 255, 0);
const ACCENT: Color = Color::Rgb(0, 255, 0);
const DIM: Color = Color::Rgb(0, 180, 0);
const CMD: Color = Color::Rgb(0, 255, 0);
const ERR: Color = Color::Rgb(255, 80, 80);
const CURRENT_GREEN: Color = Color::Rgb(80, 255, 120);
const NEXT_DIFFERENT_BLUE: Color = Color::Rgb(95, 125, 230);
const NEXT_BLUE: Color = Color::Rgb(70, 150, 255);
const INACTIVE_GRAY: Color = Color::Rgb(128, 128, 128);
const ROW_GUARD: Color = Color::Rgb(0, 0, 0);

pub fn run(app: &mut App) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    loop {
        app.tick(); // poll render thread result; clears rec_rx on completion
                    // Reassert after terminal initialization and before every flush. Color
                    // is semantic state here, and a dropped foreground command otherwise
                    // makes every cell inherit the terminal profile's default green.
        crossterm::style::force_color_output(true);
        term.draw(|f| draw(f, app))?;

        if event::poll(std::time::Duration::from_millis(40))? {
            if let Event::Key(key) = event::read()? {
                if app.show_help {
                    match key.code {
                        KeyCode::Up => app.overlay_scroll_up(),
                        KeyCode::Down => app.overlay_scroll_down(),
                        KeyCode::Home => app.overlay_scroll_home(),
                        KeyCode::Esc | KeyCode::Char('?') => {
                            app.show_help = false;
                            app.help_scroll = 0;
                        }
                        _ => {
                            app.show_help = false;
                            app.help_scroll = 0;
                        }
                    }
                    continue;
                }
                if app.show_jins {
                    match key.code {
                        KeyCode::Up => app.overlay_scroll_up(),
                        KeyCode::Down => app.overlay_scroll_down(),
                        KeyCode::Home => app.overlay_scroll_home(),
                        KeyCode::Esc => {
                            app.show_jins = false;
                            app.jins_scroll = 0;
                        }
                        _ => {
                            app.show_jins = false;
                            app.jins_scroll = 0;
                        }
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('c') | KeyCode::Char('q')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        app.should_quit = true;
                    }

                    KeyCode::Enter => {
                        let cmd = if app.input.trim().is_empty() {
                            app.last_history().unwrap_or("").to_string()
                        } else {
                            app.input.clone()
                        };
                        app.history_push(&cmd);
                        app.input.clear();
                        app.cursor_pos = 0;
                        app.message_scroll_home();
                        app.handle_command(&cmd);
                    }
                    KeyCode::Up => {
                        app.history_up();
                    }
                    KeyCode::Down => {
                        app.history_down();
                    }
                    KeyCode::Left => {
                        app.cursor_left();
                    }
                    KeyCode::Right => {
                        app.cursor_right();
                    }
                    KeyCode::Home => {
                        app.cursor_home();
                    }
                    KeyCode::End => {
                        app.cursor_end();
                    }
                    KeyCode::PageUp => {
                        app.message_scroll_up();
                    }
                    KeyCode::PageDown => {
                        app.message_scroll_down();
                    }
                    KeyCode::Tab => {
                        app.complete_input();
                    }
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
                        app.cursor_pos = 0;
                        app.message = None;
                        app.history_pos = None;
                    }
                    _ => {}
                }
            }
        }
        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().fg(INACTIVE_GRAY).bg(BG)),
        area,
    );
    if app.show_help {
        draw_help(f, app, area);
        return;
    }
    if app.show_jins {
        draw_jins_list(f, app, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);
    draw_phrases(f, app, chunks[0]);
    draw_input(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);
    draw_recording(f, app, chunks[3]);
}

fn draw_phrases(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let cur = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
    let cur_sub = crate::CUR_SUBDIV.load(std::sync::atomic::Ordering::Relaxed);
    let cur_plays = crate::CUR_PLAYS.load(std::sync::atomic::Ordering::Relaxed);
    let next = crate::NEXT_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
    let exit = crate::EXIT_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
    let n = app.phrases.len().max(1);
    let current_is_last_repeat = app
        .phrases
        .get(cur % n)
        .is_none_or(|phrase| cur_plays + 1 >= phrase.repeat.max(1));
    let show_prediction = true;
    let upcoming_jump_source = if show_prediction && !app.phrases.is_empty() {
        let mut position = (cur + 1) % app.phrases.len();
        let mut found = None;
        for _ in 0..app.phrases.len() {
            let phrase = &app.phrases[position];
            if phrase.jump.is_some() {
                found = Some(position);
                break;
            }
            if phrase.control.is_none() {
                break;
            }
            position = (position + 1) % app.phrases.len();
        }
        found
    } else {
        None
    };
    let id_positions: std::collections::HashMap<usize, usize> = app
        .phrases
        .iter()
        .enumerate()
        .map(|(index, phrase)| (phrase.id, index))
        .collect();
    let live_jump_counters = crate::jump_counters()
        .lock()
        .map(|counters| counters.clone())
        .unwrap_or_default();
    let jumpbacks: Vec<(usize, usize, usize, usize)> = app
        .phrases
        .iter()
        .enumerate()
        .filter_map(|(source, phrase)| {
            let jump = phrase.jump.as_ref()?;
            let target = id_positions.get(&jump.target_id).copied()?;
            (target != source).then_some((target, source, phrase.id, jump.times))
        })
        .collect();
    let status_width = app
        .phrases
        .iter()
        .fold("[settings]".len(), |width, phrase| {
            let total = phrase
                .jump
                .as_ref()
                .map_or(phrase.repeat.max(1), |jump| jump.times);
            width.max(format!("[{total}/{total}]").len())
        });

    let mut items: Vec<ListItem> = Vec::new();

    items.extend(app.phrases.iter().enumerate().map(|(idx, phrase)| {
        let playing = idx == cur % n;
        let is_up_next = show_prediction && !playing && idx == next;
        let is_next_different = show_prediction && !playing && idx == exit;
        let state_color = if playing {
            CURRENT_GREEN
        } else if is_up_next {
            NEXT_BLUE
        } else if is_next_different {
            NEXT_DIFFERENT_BLUE
        } else {
            INACTIVE_GRAY
        };
        let jump_prefix: Vec<Span> = jumpbacks
            .iter()
            .map(|&(target, source, jump_id, times)| {
                let on_path = target.min(source) <= idx && idx <= target.max(source);
                if !on_path {
                    return Span::styled("    ", Style::default().fg(INACTIVE_GRAY).bg(BG));
                }
                let value = live_jump_counters.get(&jump_id).copied().unwrap_or(0);
                let will_jump =
                    Some(source) == upcoming_jump_source && value.saturating_add(1) < times.max(1);
                // The source endpoint describes the immediate transition,
                // so do not fill it while the current phrase still has
                // another local repeat to play.  Paused score prediction
                // may still color the destination, but `●` is reserved for
                // the iteration which will actually leave the phrase.
                let leaves_current_phrase = current_is_last_repeat;
                let (glyph, color) = if idx == target {
                    (
                        if target < source {
                            "┌──>"
                        } else {
                            "└──>"
                        },
                        if will_jump {
                            NEXT_DIFFERENT_BLUE
                        } else {
                            INACTIVE_GRAY
                        },
                    )
                } else if idx == source && will_jump && leaves_current_phrase {
                    ("●   ", NEXT_BLUE)
                } else {
                    ("│   ", INACTIVE_GRAY)
                };
                Span::styled(glyph, Style::default().fg(color).bg(BG))
            })
            .collect();
        let id_str = format!("{:>3}: ", phrase.id);
        let marker = if playing {
            "▶ "
        } else if is_up_next {
            "▸ "
        } else if is_next_different {
            if current_is_last_repeat {
                "▷ "
            } else {
                "◇ "
            }
        } else {
            "· "
        };
        // Jump entries — show live counter for every jump, not just the playing one
        if let Some(ref js) = phrase.jump {
            let valid_target = app.phrases.iter().any(|p| p.id == js.target_id);
            // Read counter from the shared map (written by audio thread)
            let pass = live_jump_counters.get(&phrase.id).copied().unwrap_or(0);
            let total = js.times;
            let displayed_pass = pass.saturating_add(1).min(total.max(1));
            let counter = format!("{:<status_width$} ", format!("[{displayed_pass}/{total}]"));

            let col_src = if !valid_target {
                Color::Rgb(255, 80, 80)
            } else {
                state_color
            };
            let col_ctr = if !valid_target {
                Color::Rgb(255, 120, 120)
            } else {
                state_color
            };
            let err = if valid_target {
                ""
            } else {
                "  [missing target]"
            };
            let mut spans = vec![
                Span::styled("•", Style::default().fg(ROW_GUARD).bg(BG)),
                Span::styled(id_str, Style::default().fg(state_color).bg(BG)),
                Span::styled(
                    marker,
                    Style::default()
                        .fg(state_color)
                        .bg(BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            spans.extend(jump_prefix.clone());
            spans.extend(vec![
                Span::styled(
                    counter,
                    Style::default()
                        .fg(col_ctr)
                        .bg(BG)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    phrase.display_src(),
                    Style::default()
                        .fg(col_src)
                        .bg(BG)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    err,
                    Style::default()
                        .fg(Color::Rgb(255, 100, 100))
                        .bg(BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            return ListItem::new(Line::from(spans));
        }

        if phrase.control.is_some() {
            let status = if matches!(phrase.control, Some(crate::sequencer::ControlSpec::Stop)) {
                "[stop]"
            } else {
                "[settings]"
            };
            let mut spans = vec![
                Span::styled("•", Style::default().fg(ROW_GUARD).bg(BG)),
                Span::styled(id_str, Style::default().fg(state_color).bg(BG)),
                Span::styled(
                    marker,
                    Style::default()
                        .fg(state_color)
                        .bg(BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            spans.extend(jump_prefix.clone());
            spans.extend(vec![
                Span::styled(
                    format!("{status:<status_width$} "),
                    Style::default().fg(state_color).bg(BG),
                ),
                Span::styled(
                    phrase.display_src(),
                    Style::default()
                        .fg(state_color)
                        .bg(BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            return ListItem::new(Line::from(spans));
        }

        let src_str = format!("{:<28}", phrase.display_src());
        let ratios = phrase.pitch_ratios_display();
        let rhythm = phrase.rhythm_display();
        let total_plays = phrase.repeat.max(1);
        let displayed_play = if playing {
            cur_plays.saturating_add(1).min(total_plays)
        } else {
            1
        };

        let mut spans = vec![
            Span::styled("•", Style::default().fg(ROW_GUARD).bg(BG)),
            Span::styled(id_str, Style::default().fg(state_color).bg(BG)),
            Span::styled(
                marker,
                Style::default()
                    .fg(state_color)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        spans.extend(jump_prefix);
        spans.extend(vec![
            Span::styled(
                format!(
                    "{:<status_width$} ",
                    format!("[{displayed_play}/{total_plays}]")
                ),
                Style::default()
                    .fg(state_color)
                    .bg(BG)
                    .add_modifier(if playing {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(src_str, Style::default().fg(state_color).bg(BG)),
            Span::raw(" "),
        ]);

        for (si, ch) in rhythm.chars().enumerate() {
            let is_now = !app.paused && playing && si == cur_sub;
            let mut sty = Style::default().fg(state_color).bg(BG);
            if is_now {
                sty = sty.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            spans.push(Span::styled(ch.to_string(), sty));
        }
        if !ratios.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(ratios, Style::default().fg(DIM).bg(BG)));
        }

        ListItem::new(Line::from(spans))
    }));

    let title = match app.session_filename() {
        Some(filename) => format!(" maqam-live {filename} {} ", app.globals_filename()),
        None => format!(" maqam-live {} ", app.globals_filename()),
    };
    let meter = format!(" {} ", latency_status());
    let title_width = area.width.saturating_sub(2) as usize;
    let padding = title_width.saturating_sub(title.chars().count() + meter.chars().count());
    let banner = Line::from(vec![
        Span::styled(
            title,
            Style::default()
                .fg(ACCENT)
                .bg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default().bg(BG)),
        Span::styled(meter, Style::default().fg(DIM).bg(BG)),
    ]);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(banner)
            .border_style(Style::default().fg(BORDER).bg(BG))
            .style(Style::default().fg(INACTIVE_GRAY).bg(BG)),
    );
    f.render_widget(list, area);
}

fn draw_input(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let chars: Vec<char> = app.input.chars().collect();
    let mut spans = vec![Span::styled("> ", Style::default().fg(DIM).bg(BG))];
    let cursor = app.cursor_pos.min(chars.len());
    for (idx, &ch) in chars.iter().enumerate() {
        let style = if idx == cursor {
            Style::default().fg(BG).bg(CMD).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(CMD).bg(BG)
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    if cursor >= chars.len() {
        spans.push(Span::styled(
            " ",
            Style::default().fg(BG).bg(CMD).add_modifier(Modifier::BOLD),
        ));
    }
    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(BG))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" cmd ", Style::default().fg(DIM).bg(BG)))
                .border_style(Style::default().fg(BORDER).bg(BG)),
        );
    f.render_widget(para, area);

    // Keep the real terminal cursor aligned with the painted inverse cursor cell.
    // area.x + 1 (border) + 2 ("> ") + cursor_pos columns
    let cx = area.x + 1 + 2 + cursor as u16;
    let cy = area.y + 1; // +1 for top border
    f.set_cursor_position((cx, cy));
}

fn draw_status(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let nam_error = crate::nam_error()
        .lock()
        .ok()
        .and_then(|error| error.clone());
    let status_message = app
        .message
        .as_deref()
        .map(str::to_string)
        .or_else(|| nam_error.map(|error| format!("✗ NAM: {error}")));
    if let Some(msg) = status_message {
        let col = if msg.starts_with('✗') { ERR } else { DIM };
        let paragraph = Paragraph::new(msg)
            .style(Style::default().fg(col).bg(BG))
            .scroll((app.message_scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(
                        " response PgUp/PgDn ",
                        Style::default().fg(DIM).bg(BG),
                    ))
                    .border_style(Style::default().fg(BORDER).bg(BG)),
            );
        f.render_widget(paragraph, area);
        return;
    }

    let vcf_status = if app.vcf.all.enabled {
        format_vcf_status(app.vcf.all)
    } else {
        let mut parts = Vec::new();
        for setting in [
            app.vcf.mic,
            app.vcf.bass,
            app.vcf.kanun,
            app.vcf.kick,
            app.vcf.tanbura,
        ] {
            if setting.enabled {
                parts.push(format_vcf_status(setting));
            }
        }
        if parts.is_empty() {
            "off".to_string()
        } else {
            parts.join(" ")
        }
    };
    let fx_status = format_fx_status(app.fx);
    let text = Line::from(vec![Span::styled(
        format!(
            "  {}BPM:{} sus:{:.1}s vcf:{} fx:{} vol:{:.2} phrases:{}  [?] help  [z] sound  [z id] jump",
            if app.paused { "⏸ PAUSED  " } else { "" },
            app.bpm,
            app.sustain,
            vcf_status,
            fx_status,
            app.vol,
            app.phrases.len()
        ),
        Style::default().fg(DIM).bg(BG),
    )]);
    f.render_widget(Paragraph::new(text).style(Style::default().bg(BG)), area);
}

fn latency_status() -> String {
    use std::sync::atomic::Ordering::Relaxed;
    let left = crate::AUDIO_LATENCY_LEFT_US.load(Relaxed);
    let right = crate::AUDIO_LATENCY_RIGHT_US.load(Relaxed);
    let value = |us: u64| {
        if us == 0 {
            "--".to_string()
        } else {
            format!("{:.1}ms", us as f64 / 1000.0)
        }
    };
    let level = |raw: u32| {
        if raw == 0 {
            "-∞".to_string()
        } else {
            format!("{:.0}", 20.0 * (raw as f64 / 1_000_000.0).log10())
        }
    };
    let input_left = crate::INPUT_LEFT_LEVEL.load(Relaxed);
    let input_right = crate::INPUT_RIGHT_LEVEL.load(Relaxed);
    let nam_output = crate::NAM_OUTPUT_LEVEL.load(Relaxed);
    let nam = match crate::NAM_STATUS.load(Relaxed) {
        1 => "on".to_string(),
        2 => "login".to_string(),
        3 => "downloading".to_string(),
        4 => crate::nam_error()
            .lock()
            .ok()
            .and_then(|error| error.clone())
            .map(|error| format!("error: {}", nam_error_kind(&error)))
            .unwrap_or_else(|| "error".to_string()),
        5 => "bypass".to_string(),
        _ => "none".to_string(),
    };
    format!(
        "lat L:{} R:{}  in L:{} R:{}dB post:{}dB NAM:{}",
        value(left),
        value(right),
        level(input_left),
        level(input_right),
        level(nam_output),
        nam
    )
}

fn nam_error_kind(text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();
    if lower.contains("sample-rate") || lower.contains("sample rate") {
        "sample-rate"
    } else if lower.contains("download") {
        "download"
    } else if lower.contains("login") || lower.contains("tone3000") {
        "login"
    } else if lower.contains("invalid") || lower.contains("gain") {
        "signal"
    } else {
        "see response"
    }
}

fn format_vcf_status(vcf: crate::vcf::VcfSettings) -> String {
    format!(
        "{}:{:.0}/{:.2}/{:.1}/{}",
        vcf.target.as_str(),
        vcf.cutoff_hz,
        vcf.resonance,
        vcf.drive,
        vcf.wave.as_str()
    )
}

fn format_fx_status(fx: crate::fx::FxSettings) -> String {
    match (fx.reverb_enabled, fx.delay_enabled) {
        (false, false) => "off".to_string(),
        (true, false) => format!("rev:{:.2}/{:.2}", fx.reverb_mix, fx.reverb_decay),
        (false, true) => format!(
            "delay:{:.2}/{:.2}/{:.2}",
            fx.delay_time_secs, fx.delay_feedback, fx.delay_mix
        ),
        (true, true) => format!(
            "rev:{:.2}/{:.2} delay:{:.2}/{:.2}/{:.2}",
            fx.reverb_mix, fx.reverb_decay, fx.delay_time_secs, fx.delay_feedback, fx.delay_mix
        ),
    }
}

fn draw_recording(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    use std::sync::atomic::Ordering::Relaxed;

    let active = crate::REC_ACTIVE.load(Relaxed);
    let text = if active {
        let done = crate::REC_SAMPLES_DONE.load(Relaxed);
        let total = crate::REC_SAMPLES_TOTAL.load(Relaxed).max(1);
        let pct = (done * 100 / total).min(100);

        // bar width: total area minus fixed decorations ("  ◉ rendering [" + "] 100%" = 22 chars)
        let bar_w = (area.width as usize).saturating_sub(22).max(4);
        let filled = bar_w * pct / 100;
        let bar: String = std::iter::repeat('=')
            .take(filled)
            .chain(std::iter::once('>').take(if filled < bar_w { 1 } else { 0 }))
            .chain(std::iter::repeat(' ').take(bar_w.saturating_sub(filled + 1)))
            .collect();

        Line::from(vec![
            Span::styled(
                "  ◉ rendering [",
                Style::default().fg(Color::Rgb(200, 80, 80)),
            ),
            Span::styled(bar, Style::default().fg(Color::Rgb(0, 200, 100))),
            Span::styled(
                format!("] {pct:>3}%"),
                Style::default().fg(Color::Rgb(160, 160, 180)),
            ),
        ])
    } else if let Some(progress) = &app.nam_download_progress {
        let pct = progress
            .total
            .filter(|total| *total > 0)
            .map(|total| (progress.downloaded * 100 / total).min(100));
        let bar_w = (area.width as usize).saturating_sub(36).max(4);
        let filled = pct.map(|pct| bar_w * pct as usize / 100).unwrap_or(0);
        let bar: String = std::iter::repeat('=')
            .take(filled)
            .chain(std::iter::once('>').take(if filled < bar_w { 1 } else { 0 }))
            .chain(std::iter::repeat(' ').take(bar_w.saturating_sub(filled + 1)))
            .collect();
        let action = if progress.load_after { "load" } else { "cache" };
        let suffix = match pct {
            Some(pct) => format!("] {pct:>3}% {action} {}", progress.name),
            None => format!(
                "] {} {action} {}",
                format_download_size(progress.downloaded),
                progress.name
            ),
        };
        Line::from(vec![
            Span::styled("  ↓ NAM [", Style::default().fg(Color::Rgb(120, 170, 255))),
            Span::styled(bar, Style::default().fg(Color::Rgb(0, 200, 100))),
            Span::styled(suffix, Style::default().fg(Color::Rgb(160, 160, 180))),
        ])
    } else {
        match &app.last_recording {
            Some(path) => Line::from(vec![
                Span::styled("  ◉ ", Style::default().fg(Color::Rgb(200, 80, 80))),
                Span::styled(
                    path.as_str(),
                    Style::default().fg(Color::Rgb(160, 160, 180)),
                ),
            ]),
            None => Line::from(vec![Span::styled(
                "  m → record cycle to ./maqam-<ts>.mp4",
                Style::default().fg(Color::Rgb(55, 55, 70)),
            )]),
        }
    };
    f.render_widget(Paragraph::new(text).style(Style::default().bg(BG)), area);
}

fn format_download_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn draw_help(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let bright = Style::default()
        .fg(Color::Rgb(0, 255, 0))
        .bg(BG)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(DIM).bg(BG);
    let heading = Style::default()
        .fg(Color::Rgb(0, 255, 0))
        .bg(BG)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled("  maqam-live — README", heading)]),
        Line::from(vec![Span::styled("  Esc closes, Up/Down scroll", dim)]),
        Line::from(vec![Span::raw("")]),
    ];

    for raw in include_str!("../README.md").lines() {
        let line = raw.trim_end();
        if line == "```" {
            continue;
        }
        if line.starts_with("![") {
            lines.push(Line::from(vec![Span::styled(format!("  {line}"), dim)]));
        } else if line.starts_with("### ") {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", line.trim_start_matches("### ")),
                bright,
            )]));
        } else if line.starts_with("## ") || line.starts_with("# ") {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", line.trim_start_matches('#').trim()),
                heading,
            )]));
        } else if line.starts_with("- ") {
            lines.push(Line::from(vec![Span::styled(format!("  {line}"), dim)]));
        } else {
            lines.push(Line::from(vec![Span::raw(format!("  {line}"))]));
        }
    }

    lines.push(Line::from(vec![Span::raw("")]));
    for raw in crate::command::language_reference().lines() {
        if raw.ends_with(':') {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), heading)]));
        } else if raw.starts_with("- ") || raw.starts_with("  - ") || raw.starts_with("  note:") {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), dim)]));
        } else {
            lines.push(Line::from(vec![Span::raw(format!("  {raw}"))]));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT).bg(BG))
        .style(Style::default().bg(BG));

    let para = Paragraph::new(lines)
        .block(block)
        .style(Style::default().fg(ACCENT).bg(BG))
        .scroll((app.help_scroll, 0));

    f.render_widget(para, area);
}

fn draw_jins_list(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    use crate::tuning::Maqam;
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let heading = Style::default()
        .fg(Color::Rgb(0, 255, 0))
        .bg(BG)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let dim = Style::default().fg(DIM).bg(BG);

    let mut lines = vec![
        Line::from(vec![Span::styled("  maqam-live — jins registry", heading)]),
        Line::from(vec![Span::styled("  Esc closes, Up/Down scroll", dim)]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::styled(
            "  audition <Name>|<root> <Name>[, ...]   create <Name> <p/q> …   delete <Name>   ls",
            Style::default().fg(Color::Rgb(0, 160, 0)).bg(BG),
        )]),
        Line::from(vec![Span::raw("")]),
    ];

    for (name, ratios) in Maqam::list_all() {
        let color = Maqam::color_for_ratios(&ratios);
        let line_style = Style::default()
            .fg(Color::Rgb(color[0], color[1], color[2]))
            .bg(BG);
        let rat_str = ratios
            .iter()
            .map(|&(p, q)| format!("{p}/{q}"))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<14}", name),
                line_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(rat_str, line_style),
        ]));
    }

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT).bg(BG))
                .style(Style::default().bg(BG)),
        )
        .style(Style::default().fg(ACCENT).bg(BG))
        .scroll((app.jins_scroll, 0));

    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nam_error_status_is_compact_kind() {
        let kind = nam_error_kind(
            "NAM sample-rate mismatch for .nam/nama2.nam: model expects 48000 Hz but audio output is 44100 Hz; restart maqam-live with `MAQAM_SAMPLE_RATE=48000 maqam-live`",
        );

        assert_eq!(kind, "sample-rate");
    }
}
