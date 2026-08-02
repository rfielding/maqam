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
pub static AUDIO_OUTPUT_SAMPLE_RATE_HZ: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
pub static AUDIO_INPUT_SAMPLE_RATE_HZ: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
pub static NAM_MODEL_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// 0 none, 1 active, 2 login required/in progress, 3 downloading, 4 error, 5 bypassed.
pub static NAM_STATUS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static NAM_ERROR: std::sync::OnceLock<std::sync::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

pub fn nam_error() -> &'static std::sync::Mutex<Option<String>> {
    NAM_ERROR.get_or_init(|| std::sync::Mutex::new(None))
}

pub fn set_nam_error(message: impl Into<String>) {
    if let Ok(mut error) = nam_error().lock() {
        *error = Some(message.into());
    }
    NAM_STATUS.store(4, std::sync::atomic::Ordering::Relaxed);
}

pub fn clear_nam_error() {
    if let Ok(mut error) = nam_error().lock() {
        *error = None;
    }
}

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

fn run_cli(commands: Vec<String>) -> anyhow::Result<()> {
    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);
    let rx_guard = rx.clone();
    let preferred_sample_rate = app::preferred_nam_sample_rate_for_startup_commands(&commands);
    let _stream = match audio::start_audio_with_preferred_sample_rate(rx, preferred_sample_rate) {
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

    let mut last_nam_progress: Option<(String, u64, Option<u64>)> = None;
    while app.nam_download_progress.is_some() {
        app.tick();
        if let Some(progress) = &app.nam_download_progress {
            let snapshot = (progress.name.clone(), progress.downloaded, progress.total);
            if last_nam_progress.as_ref() != Some(&snapshot) {
                match progress.total.filter(|total| *total > 0) {
                    Some(total) => eprintln!(
                        "NAM download {}: {}% ({}/{})",
                        progress.name,
                        (progress.downloaded * 100 / total).min(100),
                        cli_size(progress.downloaded),
                        cli_size(total)
                    ),
                    None => eprintln!(
                        "NAM download {}: {}",
                        progress.name,
                        cli_size(progress.downloaded)
                    ),
                }
                last_nam_progress = Some(snapshot);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    app.tick();

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

fn score_file_arg(args: &[String]) -> Option<&str> {
    let path = args.first()?;
    if args.len() == 1 && path.ends_with(".mq") && std::path::Path::new(path).is_file() {
        Some(path)
    } else {
        None
    }
}

fn cli_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn main() -> anyhow::Result<()> {
    // Color is semantic UI state in maqam-live. Remove NO_COLOR before any
    // Crossterm code or worker thread can memoize it, then explicitly enable
    // ANSI colors. Even NO_COLOR=0 counts as enabled under the convention.
    std::env::remove_var("NO_COLOR");
    crossterm::style::force_color_output(true);

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(path) = score_file_arg(&args) {
        let load_command = format!("load {path}");
        let preferred_sample_rate =
            app::preferred_nam_sample_rate_for_startup_commands(&[load_command.clone()]);
        let (tx, rx) = bounded::<sequencer::AudioCmd>(512);

        // Keep the stream alive for the lifetime of the app.
        let _stream = audio::start_audio_with_preferred_sample_rate(rx, preferred_sample_rate)?;

        let mut app = app::App::new(tx);
        app.handle_command(&load_command);
        app.tick();
        ui::run(&mut app)?;
        return Ok(());
    }

    if !args.is_empty() {
        return run_cli(cli_commands(&args));
    }

    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);
    let preferred_sample_rate = app::preferred_nam_sample_rate_for_cached_models();

    // Keep the stream alive for the lifetime of the app.
    let _stream = audio::start_audio_with_preferred_sample_rate(rx, preferred_sample_rate)?;

    let mut app = app::App::new(tx);
    ui::run(&mut app)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_mq_arg_is_score_file() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maqam-main-arg-{suffix}.mq"));
        std::fs::write(&path, "MAQAM_SESSION_V3\n").unwrap();
        let arg = path.to_string_lossy().into_owned();

        assert_eq!(score_file_arg(&[arg.clone()]), Some(arg.as_str()));
        assert_eq!(score_file_arg(&[arg.clone(), "extra".into()]), None);
        assert_eq!(score_file_arg(&["missing.mq".into()]), None);

        let _ = std::fs::remove_file(path);
    }
}
