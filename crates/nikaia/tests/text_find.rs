//! **`s.find(needle; from:)`** (ADR-320 D6, Part I 2.6, #453 step 4): the byte
//! position of the first match at or after `from`, or `null`, through `std`'s
//! search rather than the language below's `str::find`.

mod common;

fn ran(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.nika");
    std::fs::write(&file, source).expect("the source");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("run")
        .arg(&file)
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).to_string()
}

/// From the start, from a position, nothing past the last match - and a
/// search begun inside a scalar finds the next match. The position cuts the
/// text it came from.
#[test]
fn find_searches_from_a_position() {
    let source = "fn station(line: ref String) -> String {\n\
         \x20   let sep = line.find(\";\") ?? 0\n\
         \x20   return line[0..<sep].clone()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let line = \"Hamburg;12.0;x\"\n\
         \x20   let sep = line.find(\";\") ?? -1\n\
         \x20   let next = line.find(\";\"; from: sep + 1) ?? -1\n\
         \x20   let none = line.find(\";\"; from: next + 1) ?? -1\n\
         \x20   let owned: String = \"\u{e4}a;\u{e4};\"\n\
         \x20   let inside = owned.find(\";\"; from: 1) ?? -1\n\
         \x20   println(f\"{sep} {next} {none} {inside} {station(line)}\")\n\
         }\n";
    assert_eq!(ran("text-find", source), "7 12 -1 3 Hamburg\n");
}
