//! **A list element a call changes is a place**, and so is a field reached
//! through one (#526): `items[1].tags.push(x)` writes through the index, where
//! a read (`*index::get(…)`) cannot be changed.

mod common;

#[test]
fn a_field_of_an_element_is_changed_in_place() {
    let source = "struct Item {\n\
         \x20   tags: Vec[String],\n\
         \x20   n: i64,\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut items: Vec[Item] = [Item { tags: [], n: 0 }, Item { tags: [], n: 0 }]\n\
         \x20   items[1].n = 5\n\
         \x20   items[1].tags.push(\"x\")\n\
         \x20   items[1].tags.push(\"y\")\n\
         \x20   println(f\"{items[0].tags.len()} {items[1].tags.len()} {items[1].n}\")\n\
         }\n";
    let dir = common::scratch_dir("element-places");
    let file = dir.join("main.nika");
    std::fs::write(&file, source).expect("the source");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("run")
        .arg(&file)
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "0 2 5\n",
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
