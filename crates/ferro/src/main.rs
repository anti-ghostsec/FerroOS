//! `ferro`: every FerroOS program in one static binary.
//!
//! `/init` and the programs in `C:\bin` are symlinks to it, and it runs the
//! one it was started as. Separate static binaries would each carry their
//! own copy of the Rust standard library and shared crates; the root file
//! system lives in RAM, so storing that code once saves about 2 MB.
//!
//! Process names stay distinct: the kernel names a process after the path it
//! was started from, so `PS` still lists init, ferro-net, ferro-shell, ...

fn main() {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let path = std::path::Path::new(&argv0);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        "init" => ferro_init::main(),
        "ferro-shell" => ferro_shell::main(),
        "ferro-cmd" => ferro_cmd::main(),
        "ferro-run" => ferro_sandbox::cli::main(),
        "ferro-system" => ferro_system::cli::main(),
        "ferro-net" | "nslookup" | "fetch" => ferro_net::cli::main(),
        _ => {
            eprintln!("ferro: start this as init, ferro-shell, ferro-cmd, ferro-run, ferro-system, ferro-net, nslookup or fetch");
            std::process::exit(2);
        }
    }
}
