//! ferro-programs: FerroOS Heavy's windows for adding and removing things,
//! in place of text menus in a terminal.
//!
//!   ferro-programs                            Add Programs (add.rs)
//!   ferro-programs --run TITLE [--root] -- PROGRAM [ARGS...]
//!                                             PROGRAM in the progress
//!                                             window (run.rs): installing,
//!                                             uninstalling, updating
//!   ferro-programs --view TITLE FILE          a text file to read (Help)

mod add;
mod proto;
mod run;
mod view;
mod win;

use ferro_ui::Res;

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => add::run(),
        Some("--view") if args.len() == 3 => view::run(&args[1], &args[2]),
        Some("--run") if args.len() >= 4 => {
            let title = &args[1];
            let rest = &args[2..];
            let root = rest.first().map(String::as_str) == Some("--root");
            let rest = if root { &rest[1..] } else { rest };
            match rest.split_first() {
                Some((sep, argv)) if sep == "--" && !argv.is_empty() => run::run(title, root, argv),
                _ => usage(),
            }
        }
        _ => usage(),
    }
}

fn usage() -> Res<()> {
    eprintln!("usage: ferro-programs [--run TITLE [--root] -- PROGRAM [ARGS...] | --view TITLE FILE]");
    std::process::exit(2);
}

#[cfg(test)]
mod shots {
    /// Every window, as pictures: `cargo test -p ferro-programs -- --ignored`
    /// with FERRO_SHOTS=folder.
    #[test]
    #[ignore]
    fn shots() {
        let dir = std::env::var("FERRO_SHOTS").expect("FERRO_SHOTS");
        let mut all = vec![("add".to_owned(), crate::add::tests::shot(false)), ("add-typing".to_owned(), crate::add::tests::shot(true))];
        for kind in ["running", "yesno", "choose", "secret", "done", "fail"] {
            all.push((format!("run-{kind}"), crate::run::tests::shot(kind)));
        }
        for (name, s) in all {
            let bytes: Vec<u8> = s.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            std::fs::write(format!("{dir}/{name}-{}x{}.rgba", s.width, s.height), bytes).unwrap();
        }
    }
}
