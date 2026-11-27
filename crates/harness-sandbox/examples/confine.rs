//! Run a command under the write confinement a step gets, to see what a tool
//! needs: `cargo run -p harness-sandbox --example confine -- <cwd> <cmd> [args…]`.
//! Prints the allowlist, then runs the command and passes its exit status on.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let cwd = std::path::PathBuf::from(args.next().expect("cwd"));
    let program = args.next().expect("command");
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).current_dir(&cwd);
    for p in harness_sandbox::step_write_allowlist(&cwd, &|_| None) {
        eprintln!("allow {}", p.display());
    }
    match harness_sandbox::confine_step(&mut cmd, &cwd, &|_| None) {
        Ok(on) => eprintln!("confined: {on}"),
        Err(why) => eprintln!("not confined: {why:?}"),
    }
    let status = cmd.status().expect("spawn");
    std::process::exit(status.code().unwrap_or(1));
}
