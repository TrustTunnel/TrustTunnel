fn main() {
    println!("cargo:rerun-if-changed=src/net_utils.c");
    cc::Build::new()
        .file("src/net_utils.c")
        .compile("net_utils");
}
