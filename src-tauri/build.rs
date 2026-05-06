fn main() {
    // On Linux: build the standalone keytao-ime binary and place it where Tauri
    // expects the sidecar (src-tauri/binaries/keytao-ime-<target-triple>).
    //
    // IMPORTANT: we only do this when invoked by the Tauri CLI (tauri build /
    // tauri dev), detected via the TAURI_CLI_VERSION env var.  Plain
    // `cargo check` / `cargo clippy` must NOT spawn a child `cargo` process
    // because the parent already holds Cargo's global file lock — doing so
    // causes an immediate deadlock.
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rerun-if-changed=../crates/keytao-linux-ime/src");
        println!("cargo:rerun-if-env-changed=TAURI_CLI_VERSION");

        let invoked_by_tauri = std::env::var("TAURI_CLI_VERSION").is_ok();

        // tauri_build::build() checks that every externalBin file exists.
        // When not invoked by the Tauri CLI (e.g. plain `cargo check`) we skip
        // the real compilation and just ensure a placeholder exists so the
        // check passes without a deadlock.
        let target_triple_for_placeholder =
            std::env::var("TARGET").unwrap_or_else(|_| "x86_64-unknown-linux-gnu".into());
        let binaries_dir =
            std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("binaries");
        std::fs::create_dir_all(&binaries_dir).ok();
        let placeholder = binaries_dir.join(format!("keytao-ime-{target_triple_for_placeholder}"));
        if !placeholder.exists() {
            std::fs::write(&placeholder, b"").ok();
        }

        if invoked_by_tauri {
            use std::process::Command;

            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            let workspace_root = std::path::Path::new(&manifest_dir)
                .parent()
                .expect("src-tauri has a parent");

            let target_triple = std::env::var("TARGET").unwrap_or_else(|_| {
                let out = Command::new("rustc")
                    .arg("-Vv")
                    .output()
                    .expect("rustc -Vv");
                String::from_utf8(out.stdout)
                    .unwrap()
                    .lines()
                    .find(|l| l.starts_with("host:"))
                    .unwrap_or("host: x86_64-unknown-linux-gnu")
                    .trim_start_matches("host:")
                    .trim()
                    .to_owned()
            });

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

            let binaries_dir = std::path::Path::new(&manifest_dir).join("binaries");
            std::fs::create_dir_all(&binaries_dir).expect("create binaries/");

            let dest = binaries_dir.join(format!("keytao-ime-{target_triple}"));
            std::fs::copy(&built, &dest).expect("copy keytao-ime sidecar");
        }
    }

    tauri_build::build()
}
