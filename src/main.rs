// main.rs — maqam-live: real-time maqam sequencer / REPL

mod analog;
mod app;
mod audio;
mod carpet;
mod command;
mod fx;
mod midi_clock;
mod midi_clockout;
mod record;
mod renderer;
mod sequencer;
mod session_v3;
mod source_background;
mod sympathetics;
mod synth;
mod tuning;
mod ui;
mod vcf;

/// Shared atomic: audio thread writes current phrase index, TUI reads it.
pub static CUR_PHRASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static NEXT_PHRASE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);
/// Phrase reached after the current phrase completes all of its local repeats.
pub static EXIT_PHRASE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);
pub static CUR_SUBDIV: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static CUR_PLAYS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static CUR_JUMP_VALUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Smoothed capture-to-predicted-playback latency in microseconds. Zero means unavailable.
pub static AUDIO_LATENCY_LEFT_US: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static AUDIO_LATENCY_RIGHT_US: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static INPUT_LEFT_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static INPUT_RIGHT_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static NAM_OUTPUT_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static NAM_MODEL_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// 0 none, 1 active, 2 login required/in progress, 3 downloading, 4 error, 5 bypassed.
pub static NAM_STATUS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Progress atomics: written by render thread, read by TUI.
pub static REC_SAMPLES_DONE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static REC_SAMPLES_TOTAL: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static REC_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Jump counters visible to TUI: phrase_id → completed jump-back count.
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

fn run_cli(commands: Vec<String>, bufsize: u32) -> anyhow::Result<()> {
    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);
    let rx_guard = rx.clone();
    let _stream = match audio::start_audio(rx, bufsize) {
        Ok(stream) => Some(stream),
        Err(err) => {
            eprintln!(
                "audio output unavailable ({err}); continuing command mode without live playback"
            );
            eprintln!(
                "to hear live playback, run maqam-live in an environment with an audio device"
            );
            None
        }
    };
    let mut app = app::App::new(tx);

    for cmd in &commands {
        eprintln!("> {cmd}");
        app.handle_command(cmd);
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

    drop(rx_guard);

    Ok(())
}

fn main() -> anyhow::Result<()> {
    // Color is semantic UI state in maqam-live. Remove NO_COLOR before any
    // Crossterm code or worker thread can memoize it, then explicitly enable
    // ANSI colors. Even NO_COLOR=0 counts as enabled under the convention.
    std::env::remove_var("NO_COLOR");
    crossterm::style::force_color_output(true);

    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut audio_bufsize = 256;

    for arg in &args {
        // println!("arg is: '{}'", arg);
        if arg.starts_with("--bufsize=") {
            if let Some((_key, value)) = arg.split_once('=') {
                match value.trim().parse::<u32>() {
                   Ok(num) => audio_bufsize = num,
                   Err(e) => println!("Failed to parse: {}", e),
                }
            }
        }
    }

    args.retain(|x| !x.starts_with("--bufsize="));

    if !args.is_empty() {
        return run_cli(cli_commands(&args), audio_bufsize);
    }

    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);

    // Keep the stream alive for the lifetime of the app.
    let _stream = audio::start_audio(rx, audio_bufsize)?;

    let mut app = app::App::new(tx);
    ui::run(&mut app)?;

    Ok(())
}
