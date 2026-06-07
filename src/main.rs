// main.rs — maqam-live: real-time maqam sequencer / REPL

mod app;
mod audio;
mod carpet;
mod command;
mod record;
mod renderer;
mod sequencer;
mod session_v3;
mod source_background;
mod synth;
mod tuning;
mod ui;

/// Shared atomic: audio thread writes current phrase index, TUI reads it.
pub static CUR_PHRASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static CUR_SUBDIV: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static CUR_PLAYS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static CUR_JUMP_REM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Progress atomics: written by render thread, read by TUI.
pub static REC_SAMPLES_DONE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static REC_SAMPLES_TOTAL: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static REC_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Jump counters visible to TUI: phrase_id → remaining jumps.
/// Written by audio thread on every jump state change.
pub static JUMP_COUNTERS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, usize>>,
> = std::sync::OnceLock::new();

pub fn jump_counters() -> &'static std::sync::Mutex<std::collections::HashMap<usize, usize>> {
    JUMP_COUNTERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

use crossbeam_channel::bounded;

fn cli_commands(args: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for arg in args {
        if arg == "--" {
            if !cur.is_empty() {
                commands.push(cur.join(" "));
                cur.clear();
            }
        } else {
            cur.push(arg.clone());
        }
    }
    if !cur.is_empty() {
        commands.push(cur.join(" "));
    }
    commands
}

fn normalize_load_target_if_needed(cmd: &str) {
    let trimmed = cmd.trim();
    let mut parts = trimmed.split_whitespace();
    if parts.next() != Some("load") {
        return;
    }
    if let Some(path) = parts.next() {
        let _ = crate::session_v3::downgrade_v3_file_to_v2_for_current_loader(path);
    }
}

fn run_cli(commands: Vec<String>) -> anyhow::Result<()> {
    eprintln!("carpet-guided-background: controlled branch active");
    eprintln!("carpet-guided-background: src/carpet.rs is present; record.rs wiring is next");

    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);
    let _stream = audio::start_audio(rx)?;
    let mut app = app::App::new(tx);

    for cmd in &commands {
        eprintln!("> {cmd}");
        normalize_load_target_if_needed(cmd);
        app.handle_command(cmd);
        crate::session_v3::normalize_saved_message(app.message.as_ref());
        app.tick();
        if let Some(msg) = &app.message {
            eprintln!("{msg}");
        }
    }

    // `m` records on a worker thread. In CLI mode, wait for it and print the path.
    while app.rec_rx.is_some() || REC_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    app.tick();

    if let Some(path) = &app.last_recording {
        println!("{path}");
    } else if let Some(msg) = &app.message {
        println!("{msg}");
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return run_cli(cli_commands(&args));
    }

    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);

    // Keep the stream alive for the lifetime of the app.
    let _stream = audio::start_audio(rx)?;

    let mut app = app::App::new(tx);
    ui::run(&mut app)?;

    Ok(())
}
