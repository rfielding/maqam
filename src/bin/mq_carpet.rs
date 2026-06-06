// mq_carpet.rs — disabled placeholder
//
// The first deterministic Rust carpet renderer prototype failed visually: it
// produced sparse cells and disconnected seams instead of the embroidered
// carpet reference.  Keep this binary as an explicit stop sign so nobody
// mistakes it for the real renderer.
//
// The canonical reference is now the Python guided-redraw pipeline documented
// in docs/carpet-renderer.md.  Port Rust from that reference only after matching
// the preserved still image and MP4 behavior.

fn main() {
    eprintln!("mq_carpet Rust prototype is intentionally disabled.");
    eprintln!("The visual target is the preserved embroidered carpet reference.");
    eprintln!("Use scripts/make_guided_redraw_mp4.py with the reference PNG instead.");
    std::process::exit(2);
}
