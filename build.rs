fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=assets");
    #[cfg(windows)]
    embed_resource::compile("assets/windows/spanner.rc", embed_resource::NONE).manifest_required().expect("compile Windows resources");
}
