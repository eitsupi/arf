fn main() {
    println!("cargo:rerun-if-changed=src/sys/repl_driver.c");
    cc::Build::new()
        .file("src/sys/repl_driver.c")
        .warnings(true)
        .compile("arf_repl_driver");
}
