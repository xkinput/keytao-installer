fn main() {
    // On Linux: embed the keytao-ime binary via include_bytes! for a fully
    // self-contained release build.
    //
    // Embedding strategy (avoids nested-cargo file-lock deadlock):
    //   1. Release builds (scripts/_docker-build.sh): pre-build keytao-linux-ime
    //      independently and set KEYTAO_IME_PATH to the binary path.
    //      build.rs simply copies it — no nested `cargo build` needed.
    //   2. Local `pnpm tauri dev/build` (TAURI_CLI_VERSION set, no KEYTAO_IME_PATH):
    //      fall back to spawning `cargo build -p keytao-linux-ime`.
    //   3. `cargo check` / `cargo clippy` (neither env var set): write an empty
    //      placeholder so include_bytes! compiles without errors.
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rerun-if-changed=../crates/keytao-linux-ime/src");
        println!("cargo:rerun-if-env-changed=KEYTAO_IME_PATH");
        println!("cargo:rerun-if-env-changed=TAURI_CLI_VERSION");

        let out_dir = std::env::var("OUT_DIR").unwrap();
        let out_ime = std::path::Path::new(&out_dir).join("keytao-ime");

        if let Ok(pre_built) = std::env::var("KEYTAO_IME_PATH") {
            // Path 1: pre-built binary provided by the build script
            std::fs::copy(&pre_built, &out_ime)
                .unwrap_or_else(|e| panic!("copy keytao-ime from {pre_built}: {e}"));
        } else if std::env::var("TAURI_CLI_VERSION").is_ok() {
            // Path 2: local tauri build — spawn nested cargo
            use std::process::Command;

            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            let workspace_root = std::path::Path::new(&manifest_dir)
                .parent()
                .expect("src-tauri has a parent");

            let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".into());
            let cargo_args: Vec<&str> = if profile == "release" {
                vec!["build", "-p", "keytao-linux-ime", "--release"]
            } else {
                vec!["build", "-p", "keytao-linux-ime"]
            };

            let status = Command::new("cargo")
                .args(&cargo_args)
                .current_dir(workspace_root)
                .status()
                .expect("failed to run cargo build for keytao-linux-ime");

            if !status.success() {
                panic!("keytao-linux-ime build failed");
            }

            let built = workspace_root
                .join("target")
                .join(&profile)
                .join("keytao-ime");

            std::fs::copy(&built, &out_ime).expect("copy keytao-ime to OUT_DIR");
        } else {
            // Path 3: cargo check / clippy — empty placeholder
            if !out_ime.exists() {
                std::fs::write(&out_ime, b"").ok();
            }
        }
    }

    tauri_build::build()
}
