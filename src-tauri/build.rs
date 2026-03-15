fn main() {
    println!(
        "cargo:rustc-env=TAURI_BUILD_TARGET={}",
        std::env::var("TARGET").expect("TARGET not set by cargo")
    );
    tauri_build::build();
}
