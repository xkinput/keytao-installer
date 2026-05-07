fn main() {
    // On Linux: build keytao-ime and embed it via include_bytes! so the final
    // Tauri binary is fully self-contained (no externalBin sidecar needed).
    //
    // We write the binary (or an empty placeholder) to $OUT_DIR/keytao-ime so
    // that `include_bytes!(concat!(env!("OUT_DIR"), "/keytao-ime"))` always
    // compiles, even during plain `cargo check`.
    //
    // Actual compilation only runs when TAURI_CLI_VERSION is set (i.e. the
    // Tauri CLI invoked us) to avoid the cargo file-lock deadlock during
    // `cargo check` / `cargo clippy`.
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rerun-if-changed=../crates/keytao-linux-ime/src");
        println!("cargo:rerun-if-env-changed=TAURI_CLI_VERSION");

        let out_dir = std::env::var("OUT_DIR").unwrap();
        let out_ime = std::path::Path::new(&out_dir).join("keytao-ime");

        let invoked_by_tauri = std::env::var("TAURI_CLI_VERSION").is_ok();

        if invoked_by_tauri {
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
            // Placeholder so include_bytes! compiles during `cargo check`
            if !out_ime.exists() {
                std::fs::write(&out_ime, b"").ok();
            }
        }
    }

    tauri_build::build()
}
