//! Pure librime engine wrapper — no Tauri, no D-Bus, no platform I/O.
//! Every platform frontend (Tauri app, ibus engine, macOS IMKit, Windows TSF)
//! links against this crate as its rime back-end.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImeState {
    pub preedit: String,
    pub cursor: usize,
    pub candidates: Vec<Candidate>,
    pub highlighted_candidate_index: usize,
    pub page: usize,
    pub is_last_page: bool,
    pub committed: Option<String>,
    pub select_keys: Option<String>,
    pub ascii_mode: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyProcessResult {
    pub state: ImeState,
    pub accepted: bool,
}

impl ImeState {
    pub fn empty() -> Self {
        Self {
            preedit: String::new(),
            cursor: 0,
            candidates: vec![],
            highlighted_candidate_index: 0,
            page: 0,
            is_last_page: true,
            committed: None,
            select_keys: None,
            ascii_mode: false,
        }
    }
}

fn rime_build_dirs(user_data_dir: &Path, shared_data_dir: &Path) -> (PathBuf, PathBuf) {
    let staging_dir = user_data_dir.join("build");
    let prebuilt_dir = if user_data_dir == shared_data_dir {
        shared_data_dir.join("prebuilt")
    } else {
        shared_data_dir.join("build")
    };
    (staging_dir, prebuilt_dir)
}

fn rime_log_dir(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join("log")
}

// ── Desktop-only engine (guarded at the module level) ────────────────────────

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop {
    use super::*;
    use rime_api::{
        create_session, full_deploy_and_wait, initialize, setup, DeployResult, KeyEvent, KeyStatus,
        Traits,
    };
    use std::sync::OnceLock;

    // librime setup+initialize must run exactly once per process.
    static RIME_INITED: OnceLock<()> = OnceLock::new();

    /// Initialize and fully deploy librime.
    /// `setup` + `initialize` run only on the first call; subsequent calls only
    /// re-run `full_deploy_and_wait` so that newly installed schemas are picked up.
    /// Blocking — run inside `tokio::task::spawn_blocking` when called from async code.
    pub fn deploy(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
        let log_dir = rime_log_dir(Path::new(&user_data_dir));

        RIME_INITED.get_or_init(|| {
            let user_dir = Path::new(&user_data_dir);
            let shared_dir = Path::new(&shared_data_dir);
            let (staging_dir, prebuilt_dir) = rime_build_dirs(user_dir, shared_dir);
            let log_dir = rime_log_dir(user_dir);

            let _ = std::fs::create_dir_all(&staging_dir);
            let _ = std::fs::create_dir_all(&prebuilt_dir);
            let _ = std::fs::create_dir_all(&log_dir);

            let mut traits = Traits::new();
            traits.set_user_data_dir(&user_data_dir);
            traits.set_shared_data_dir(&shared_data_dir);
            traits.set_staging_dir(&staging_dir.to_string_lossy());
            traits.set_prebuilt_data_dir(&prebuilt_dir.to_string_lossy());
            traits.set_log_dir(&log_dir.to_string_lossy());
            traits.set_distribution_name("KeyTao");
            traits.set_distribution_code_name("keytao");
            traits.set_distribution_version("1.0.0");
            traits.set_app_name("rime.keytao");
            setup(&mut traits);
            initialize(&mut traits);
        });
        match full_deploy_and_wait() {
            DeployResult::Success => Ok(()),
            DeployResult::Failure => Err(format!(
                "Rime deployment failed. See librime logs in {}",
                log_dir.display()
            )),
        }
    }

    /// An active rime input session.
    pub struct Engine {
        session: rime_api::Session,
    }

    // SAFETY: Session holds only a usize (session_id).
    // librime's C API is documented as thread-safe across different sessions.
    unsafe impl Send for Engine {}
    unsafe impl Sync for Engine {}

    impl Engine {
        /// Create a new session. `deploy()` must have succeeded first.
        pub fn new() -> Result<Self, String> {
            let session = create_session().map_err(|e| format!("{e:?}"))?;
            Ok(Self { session })
        }

        pub fn process_key(&self, keycode: u32, mask: u32) -> ImeState {
            self.process_key_result(keycode, mask).state
        }

        pub fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
            let status = self.session.process_key(KeyEvent::new(keycode, mask));
            KeyProcessResult {
                state: extract_state(&self.session),
                accepted: matches!(status, KeyStatus::Accept),
            }
        }

        pub fn select_candidate(&self, index: usize) -> ImeState {
            if index < 9 {
                let kc = b'1' as u32 + index as u32;
                self.session.process_key(KeyEvent::new(kc, 0));
            }
            extract_state(&self.session)
        }

        pub fn change_page(&self, backward: bool) -> ImeState {
            let kc = if backward { b'-' as u32 } else { b'=' as u32 };
            self.session.process_key(KeyEvent::new(kc, 0));
            extract_state(&self.session)
        }

        pub fn reset(&self) -> ImeState {
            self.session.process_key(KeyEvent::new(0xff1b_u32, 0)); // XK_Escape
            ImeState::empty()
        }

        pub fn current_schema_name(&self) -> String {
            self.session
                .status()
                .map(|s| s.schema_name().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        }

        pub fn is_ascii_mode(&self) -> bool {
            self.session
                .status()
                .map(|s| s.is_ascii_mode)
                .unwrap_or(false)
        }
    }

    fn extract_state(session: &rime_api::Session) -> ImeState {
        let committed = session.commit().map(|c| c.text().to_string());

        let Some(ctx) = session.context() else {
            return ImeState {
                committed,
                ..ImeState::empty()
            };
        };

        let comp = ctx.composition();
        let preedit = comp.preedit.unwrap_or("").to_string();
        let cursor = comp.cursor_pos;

        let menu = ctx.menu();
        let candidates = menu
            .candidates
            .iter()
            .map(|c| Candidate {
                text: c.text.to_string(),
                comment: c.comment.map(|s: &str| s.to_string()),
            })
            .collect();

        let ascii_mode = session.status().map(|s| s.is_ascii_mode).unwrap_or(false);

        ImeState {
            preedit,
            cursor,
            candidates,
            highlighted_candidate_index: menu.highlighted_candidate_index,
            page: menu.page_no,
            is_last_page: menu.is_last_page,
            committed,
            select_keys: menu.select_keys.map(|s: &str| s.to_string()),
            ascii_mode,
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::{deploy, Engine};

fn is_default_custom(filename: &str) -> bool {
    filename == "default.custom.yaml" || filename == "default-custom.yaml"
}

fn read_optional_default_custom(base: &Path) -> Option<String> {
    std::fs::read_to_string(base.join("default.custom.yaml"))
        .ok()
        .or_else(|| std::fs::read_to_string(base.join("default-custom.yaml")).ok())
}

fn has_base_default_yaml(dir: &Path) -> bool {
    dir.join("default.yaml").is_file()
}

#[cfg(target_os = "linux")]
fn nix_store_rime_data_dirs() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir("/nix/store")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .into_string()
                .ok()
                .map(|name| (name, entry.path()))
        })
        .filter(|(name, _)| !name.ends_with(".drv") && name.contains("-rime-data-"))
        .map(|(_, path)| path.join("share/rime-data"))
        .filter(|path| has_base_default_yaml(path))
        .collect();
    paths.sort();
    paths.reverse();
    paths
}

pub fn parse_schema_list(content: &str) -> Vec<String> {
    let mut schemas = Vec::new();
    let mut in_list = false;
    for line in content.lines() {
        let t = line.trim();
        if t.contains("schema_list:") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(rest) = t.strip_prefix("- schema:") {
                let schema = rest.trim().to_string();
                if !schema.is_empty() {
                    schemas.push(schema);
                }
            } else if !t.is_empty() && !t.starts_with('#') && !t.starts_with('-') {
                in_list = false;
            }
        }
    }
    schemas
}

fn schema_list_from_yaml(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Sequence(entries)) = value else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| match entry {
            Value::Mapping(mapping) => mapping
                .get(Value::String("schema".to_string()))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            Value::String(schema) => Some(schema.clone()),
            _ => None,
        })
        .collect()
}

fn make_schema_list_value(schemas: &[String]) -> Value {
    Value::Sequence(
        schemas
            .iter()
            .map(|schema| {
                let mut mapping = Mapping::new();
                mapping.insert(
                    Value::String("schema".to_string()),
                    Value::String(schema.clone()),
                );
                Value::Mapping(mapping)
            })
            .collect(),
    )
}

fn merge_yaml_mapping(existing: &Mapping, package: &Mapping) -> Mapping {
    let mut merged = package.clone();

    for (key, existing_value) in existing {
        match (key.as_str(), package.get(key)) {
            (Some("schema_list"), Some(package_value)) => {
                let package_schemas: Vec<String> = schema_list_from_yaml(Some(package_value))
                    .into_iter()
                    .filter(|schema| schema.starts_with("keytao"))
                    .collect();
                let user_schemas: Vec<String> = schema_list_from_yaml(Some(existing_value))
                    .into_iter()
                    .filter(|schema| !schema.starts_with("keytao"))
                    .collect();
                let merged_schemas: Vec<String> = user_schemas
                    .iter()
                    .chain(package_schemas.iter())
                    .cloned()
                    .collect();
                merged.insert(key.clone(), make_schema_list_value(&merged_schemas));
            }
            (_, Some(Value::Mapping(package_map))) => {
                if let Value::Mapping(existing_map) = existing_value {
                    merged.insert(
                        key.clone(),
                        Value::Mapping(merge_yaml_mapping(existing_map, package_map)),
                    );
                }
            }
            (_, Some(_)) => {}
            (_, None) => {
                merged.insert(key.clone(), existing_value.clone());
            }
        }
    }

    merged
}

fn string_merge_default_custom(
    existing: Option<&str>,
    package_content: &str,
) -> (String, Vec<String>) {
    let keytao_schemas: Vec<String> = parse_schema_list(package_content)
        .into_iter()
        .filter(|schema| schema.starts_with("keytao"))
        .collect();
    let user_schemas: Vec<String> = existing
        .map(|content| {
            parse_schema_list(content)
                .into_iter()
                .filter(|schema| !schema.starts_with("keytao"))
                .collect()
        })
        .unwrap_or_default();
    let merged_schemas: Vec<String> = user_schemas
        .iter()
        .chain(keytao_schemas.iter())
        .cloned()
        .collect();

    let mut out = String::new();
    let mut in_list = false;
    for line in package_content.lines() {
        let t = line.trim();
        if !in_list {
            out.push_str(line);
            out.push('\n');
            if t.contains("schema_list:") {
                in_list = true;
                for schema in &merged_schemas {
                    out.push_str(&format!("    - schema: {schema}\n"));
                }
            }
        } else if t.starts_with("- schema:") {
        } else {
            in_list = false;
            out.push_str(line);
            out.push('\n');
        }
    }

    (out, user_schemas)
}

pub fn merge_default_custom_content(
    existing: Option<&str>,
    package_content: &str,
) -> Result<(String, Vec<String>), String> {
    let package_yaml = match serde_yaml::from_str::<Value>(package_content) {
        Ok(Value::Mapping(mapping)) => mapping,
        _ => return Ok(string_merge_default_custom(existing, package_content)),
    };

    let user_schemas: Vec<String> = existing
        .map(parse_schema_list)
        .unwrap_or_default()
        .into_iter()
        .filter(|schema| !schema.starts_with("keytao"))
        .collect();

    let merged_yaml = if let Some(existing) = existing {
        match serde_yaml::from_str::<Value>(existing) {
            Ok(Value::Mapping(existing_mapping)) => {
                Value::Mapping(merge_yaml_mapping(&existing_mapping, &package_yaml))
            }
            _ => Value::Mapping(package_yaml.clone()),
        }
    } else {
        Value::Mapping(package_yaml.clone())
    };

    let mut merged = serde_yaml::to_string(&merged_yaml).map_err(|e| e.to_string())?;
    if let Some(stripped) = merged.strip_prefix("---\n") {
        merged = stripped.to_string();
    }

    Ok((merged, user_schemas))
}

fn extract_lua_require(line: &str) -> Option<String> {
    let pos = line.find("require")?;
    let after = line[pos + 7..].trim_start();
    if !after.starts_with('(') {
        return None;
    }
    let after = after[1..].trim_start();
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let content = &after[1..];
    let end = content.find(quote)?;
    Some(content[..end].to_string())
}

pub fn parse_rime_lua_requires(content: &str) -> Vec<String> {
    let mut requires = Vec::new();
    let mut in_block_comment = false;
    for line in content.lines() {
        let t = line.trim();
        if in_block_comment {
            if t.contains("--]]") {
                in_block_comment = false;
            }
            continue;
        }
        if t.starts_with("--[[") {
            in_block_comment = true;
            continue;
        }
        if t.is_empty() || t.starts_with("--") {
            continue;
        }
        if let Some(module) = extract_lua_require(t) {
            if !requires.contains(&module) {
                requires.push(module);
            }
        }
    }
    requires
}

pub fn merge_rime_lua_content(
    local_content: Option<&str>,
    package_content: &str,
    package_lua_filenames: &HashSet<String>,
) -> (String, Vec<(String, String)>) {
    let Some(local_content) = local_content else {
        return (package_content.to_string(), Vec::new());
    };

    let package_requires: HashSet<String> = parse_rime_lua_requires(package_content)
        .into_iter()
        .collect();
    let mut renames = Vec::new();
    let mut extra_lines = Vec::new();
    let mut in_block_comment = false;

    for line in local_content.lines() {
        let t = line.trim();
        if in_block_comment {
            if t.contains("--]]") {
                in_block_comment = false;
            }
            continue;
        }
        if t.starts_with("--[[") {
            in_block_comment = true;
            continue;
        }
        if t.is_empty() || t.starts_with("--") {
            continue;
        }
        if let Some(module) = extract_lua_require(t) {
            if package_requires.contains(&module) {
                continue;
            }
            let filename = format!("{module}.lua");
            if package_lua_filenames.contains(&filename) {
                let new_name = format!("{module}_user");
                let new_line = line
                    .replace(&format!("\"{}\"", module), &format!("\"{}\"", new_name))
                    .replace(&format!("'{}'", module), &format!("'{}'", new_name));
                renames.push((module, new_name));
                extra_lines.push(new_line);
            } else {
                extra_lines.push(line.to_string());
            }
        } else {
            extra_lines.push(line.to_string());
        }
    }

    let mut merged = package_content.to_string();
    if !extra_lines.is_empty() {
        if !merged.ends_with('\n') {
            merged.push('\n');
        }
        for line in &extra_lines {
            merged.push_str(line);
            merged.push('\n');
        }
    }

    (merged, renames)
}

pub fn sync_user_rime_assets(user_data_dir: &Path, shared_data_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(user_data_dir).map_err(|e| format!("create user dir: {e}"))?;

    let package_default_custom = std::fs::read_dir(shared_data_dir).ok().and_then(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .find(|entry| is_default_custom(&entry.file_name().to_string_lossy()))
            .and_then(|entry| std::fs::read_to_string(entry.path()).ok())
    });

    if let Some(package_content) = package_default_custom {
        let existing = read_optional_default_custom(user_data_dir);
        let (merged, _) = merge_default_custom_content(existing.as_deref(), &package_content)?;
        std::fs::write(user_data_dir.join("default.custom.yaml"), merged)
            .map_err(|e| format!("write default.custom.yaml: {e}"))?;
    }

    let package_rime_lua = std::fs::read_to_string(shared_data_dir.join("rime.lua")).ok();
    if let Some(package_content) = package_rime_lua {
        let package_lua_filenames: HashSet<String> = std::fs::read_dir(shared_data_dir.join("lua"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_file() {
                    Some(entry.file_name().to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect();

        let local_content = std::fs::read_to_string(user_data_dir.join("rime.lua")).ok();
        let (merged, renames) = merge_rime_lua_content(
            local_content.as_deref(),
            &package_content,
            &package_lua_filenames,
        );

        if !renames.is_empty() {
            let user_lua_dir = user_data_dir.join("lua");
            std::fs::create_dir_all(&user_lua_dir).map_err(|e| format!("create lua dir: {e}"))?;
            for (old_name, new_name) in renames {
                let old_path = user_lua_dir.join(format!("{old_name}.lua"));
                let new_path = user_lua_dir.join(format!("{new_name}.lua"));
                if !new_path.exists() && old_path.exists() {
                    let bytes = std::fs::read(&old_path)
                        .map_err(|e| format!("read lua/{old_name}.lua: {e}"))?;
                    std::fs::write(&new_path, bytes)
                        .map_err(|e| format!("write lua/{new_name}.lua: {e}"))?;
                }
            }
        }

        std::fs::write(user_data_dir.join("rime.lua"), merged)
            .map_err(|e| format!("write rime.lua: {e}"))?;
    }

    Ok(())
}

// ── Platform path helpers (all platforms) ────────────────────────────────────

/// Dedicated keytao user data directory for this platform.
pub fn default_user_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|h| h.join("Library/keytao"));
    }
    #[cfg(target_os = "windows")]
    {
        return dirs::config_dir().map(|c| c.join("keytao"));
    }
    #[cfg(target_os = "linux")]
    {
        return dirs::data_local_dir().map(|d| d.join("keytao"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// Best-guess shared rime data directory (system-installed schemas/essay.txt).
pub fn default_shared_data_dir() -> String {
    #[cfg(target_os = "macos")]
    {
        let squirrel = "/Library/Input Methods/Squirrel.app/Contents/SharedSupport";
        if Path::new(squirrel).exists() {
            return squirrel.to_string();
        }
        for p in [
            "/opt/homebrew/share/rime-data",
            "/usr/local/share/rime-data",
        ] {
            if Path::new(p).exists() {
                return p.to_string();
            }
        }
        return String::new();
    }
    #[cfg(target_os = "linux")]
    {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();

        for key in [
            "KEYTAO_RIME_SHARED_DATA_DIR",
            "RIME_SHARED_DATA_DIR",
            "RIME_DATA_DIR",
        ] {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                if !value.is_empty() {
                    candidates.push(PathBuf::from(value));
                }
            }
        }

        if let Ok(lib_dir) = std::env::var("RIME_LIB_DIR") {
            let lib_dir = PathBuf::from(lib_dir);
            if let Some(prefix) = lib_dir.parent() {
                candidates.push(prefix.join("share/rime-data"));
            }
        }

        if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
            for base in xdg_data_dirs.split(':').filter(|part| !part.is_empty()) {
                candidates.push(PathBuf::from(base).join("rime-data"));
            }
        }

        candidates.extend(nix_store_rime_data_dirs());

        candidates.extend([
            PathBuf::from("/run/current-system/sw/share/rime-data"),
            PathBuf::from("/usr/local/share/rime-data"),
            PathBuf::from("/usr/share/rime-data"),
        ]);

        for path in candidates {
            if !seen.insert(path.clone()) {
                continue;
            }
            if has_base_default_yaml(&path) {
                return path.to_string_lossy().into_owned();
            }
        }
        return "/usr/share/rime-data".to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var("WEASEL_ROOT")
            .map(|r| format!("{r}\\data"))
            .unwrap_or_else(|_| r"C:\Program Files\Rime\weasel-data".to_string());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        String::new()
    }
}

/// Returns true if `dir` exists and contains at least one `.schema.yaml` file.
pub fn has_schemas(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".schema.yaml"))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        merge_default_custom_content, merge_rime_lua_content, parse_rime_lua_requires,
        parse_schema_list, rime_build_dirs, rime_log_dir,
    };
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn parse_schema_list_reads_schema_entries() {
        let content = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: foo\n";
        assert_eq!(parse_schema_list(content), vec!["keytao", "foo"]);
    }

    #[test]
    fn merge_default_custom_keeps_user_schemas() {
        let existing =
            "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: keytao_old\n";
        let package = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: keytao-dz\n";
        let (merged, user) = merge_default_custom_content(Some(existing), package).unwrap();
        assert_eq!(user, vec!["user_schema"]);
        assert!(merged.contains("- schema: user_schema"));
        assert!(merged.contains("- schema: keytao"));
        assert!(merged.contains("- schema: keytao-dz"));
        assert!(!merged.contains("keytao_old"));
    }

    #[test]
    fn merge_default_custom_preserves_other_patch_keys() {
        let existing = "patch:\n  menu:\n    page_size: 9\n  ascii_composer:\n    switch_key:\n      Caps_Lock: noop\n  schema_list:\n    - schema: user_schema\n";
        let package = "patch:\n  menu:\n    page_size: 6\n  schema_list:\n    - schema: keytao\n";
        let (merged, _) = merge_default_custom_content(Some(existing), package).unwrap();
        assert!(merged.contains("switch_key"));
        assert!(merged.contains("Caps_Lock"));
        assert!(merged.contains("page_size: 6"));
    }

    #[test]
    fn parse_rime_lua_requires_skips_block_comments() {
        let content = "--[[\nfoo = require(\"bar\")\n--]]\nreal = require(\"real\")\n";
        assert_eq!(parse_rime_lua_requires(content), vec!["real"]);
    }

    #[test]
    fn merge_rime_lua_appends_user_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let package = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua_content(Some(local), package, &HashSet::new());
        assert!(merged.contains("require(\"keytao_filter\")"));
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    #[test]
    fn merge_rime_lua_renames_conflicting_user_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let package = "keytao = require(\"keytao\")\n";
        let package_files: HashSet<String> = ["my_mod.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua_content(Some(local), package, &package_files);
        assert_eq!(
            renames,
            vec![("my_mod".to_string(), "my_mod_user".to_string())]
        );
        assert!(merged.contains("require(\"my_mod_user\")"));
    }

    #[test]
    fn same_root_user_and_shared_use_separate_build_dirs() {
        let root = Path::new("/tmp/keytao");
        let (staging, prebuilt) = rime_build_dirs(root, root);
        assert_eq!(staging, Path::new("/tmp/keytao/build"));
        assert_eq!(prebuilt, Path::new("/tmp/keytao/prebuilt"));
    }

    #[test]
    fn rime_logs_are_written_under_dedicated_keytao_dir() {
        let root = Path::new("/tmp/keytao");
        assert_eq!(rime_log_dir(root), Path::new("/tmp/keytao/log"));
    }
}
