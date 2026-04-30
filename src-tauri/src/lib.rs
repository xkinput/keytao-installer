use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Deserialize, Clone)]
struct ReleaseCache {
    etag: String,
    cached_at: u64,
    release: ReleaseInfo,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DownloadUrls {
    pub macos: Option<String>,
    pub windows: Option<String>,
    pub linux: Option<String>,
    pub android: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub name: String,
    pub published_at: String,
    pub body: String,
    pub download_urls: DownloadUrls,
}

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String,
    pub percent: u32,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct FileItem {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Serialize, Clone)]
pub struct VerifyEntry {
    pub path: String,
    pub ok: bool,
    pub note: String,
}

#[derive(Serialize, Clone)]
pub struct InstallResult {
    pub merged_schemas: Vec<String>,
    pub logs: Vec<String>,
    pub verify: Vec<VerifyEntry>,
}

fn build_client(app: &AppHandle) -> Result<reqwest::Client, String> {
    let version = app.package_info().version.to_string();
    reqwest::Client::builder()
        .user_agent(format!("keytao-installer/{version}"))
        .build()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn fetch_latest_release(app: AppHandle) -> Result<ReleaseInfo, String> {
    let cache_path = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())
        .map(|d| d.join("release_cache.json"))
        .ok();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let cached: Option<ReleaseCache> = cache_path.as_ref().and_then(|p| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    });

    let client = build_client(&app)?;
    let mut req = client.get("https://api.github.com/repos/xkinput/KeyTao/releases/latest");
    if let Some(ref c) = cached {
        req = req.header("If-None-Match", &c.etag);
    }

    let response = req.send().await.map_err(|e| format!("网络请求失败: {e}"))?;

    if response.status() == 304 {
        return Ok(cached.unwrap().release);
    }

    if !response.status().is_success() {
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        let msg = body["message"].as_str().unwrap_or("");
        if msg.contains("rate limit") {
            if let Some(c) = cached {
                return Ok(c.release);
            }
            return Err("GitHub API 请求频率超限，请稍后再试".to_string());
        }
        return Err(format!("获取版本信息失败，HTTP {status}: {msg}"));
    }

    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let release: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))?;

    let version = release["tag_name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let name = release["name"].as_str().unwrap_or("").to_string();
    let published_at = release["published_at"].as_str().unwrap_or("").to_string();
    let body = release["body"].as_str().unwrap_or("").to_string();

    let mut urls = DownloadUrls {
        macos: None,
        windows: None,
        linux: None,
        android: None,
    };

    if let Some(assets) = release["assets"].as_array() {
        for asset in assets {
            let asset_name = asset["name"].as_str().unwrap_or("").to_lowercase();
            let url = match asset["browser_download_url"].as_str() {
                Some(u) => u.to_string(),
                None => continue,
            };
            if !asset_name.ends_with(".zip") {
                continue;
            }
            if asset_name.contains("mac") || asset_name.contains("macos") {
                urls.macos = Some(url);
            } else if asset_name.contains("win") || asset_name.contains("windows") {
                urls.windows = Some(url);
            } else if asset_name.contains("android") || asset_name.contains("trime") {
                urls.android = Some(url);
            } else if asset_name.contains("linux") {
                urls.linux = Some(url);
            }
        }
    }

    let info = ReleaseInfo {
        version,
        name,
        published_at,
        body,
        download_urls: urls,
    };

    if let (Some(path), false) = (cache_path, etag.is_empty()) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let cache = ReleaseCache {
            etag,
            cached_at: now,
            release: info.clone(),
        };
        serde_json::to_string(&cache)
            .ok()
            .and_then(|s| std::fs::write(&path, s).ok());
    }

    Ok(info)
}

fn rime_default_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Rime"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir().map(|c| c.join("Rime"))
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir()?;
        let fcitx5 = home.join(".local/share/fcitx5/rime");
        let ibus = home.join(".config/ibus/rime");
        if fcitx5.exists() {
            Some(fcitx5)
        } else if ibus.exists() {
            Some(ibus)
        } else {
            Some(fcitx5)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

#[tauri::command]
async fn select_directory(
    #[allow(unused_variables)] app: AppHandle,
    im_type: Option<String>,
) -> Result<Option<String>, String> {
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::{DialogExt, FilePath};
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut builder = app.dialog().file();
        let resolved_default = im_type
            .as_deref()
            .and_then(|im| {
                let home = dirs::home_dir()?;
                match im {
                    "fcitx5" => Some(home.join(".local/share/fcitx5/rime")),
                    "ibus" => Some(home.join(".config/ibus/rime")),
                    _ => None,
                }
            })
            .or_else(rime_default_path);
        if let Some(default) = resolved_default {
            builder = builder.set_directory(default);
        }
        builder.pick_folder(move |folder: Option<FilePath>| {
            let _ = tx.send(folder);
        });
        let result = rx.await.map_err(|e| e.to_string())?;
        Ok(result.map(|p| p.to_string()))
    }
    #[cfg(target_os = "android")]
    {
        Err("Not supported on Android".into())
    }
}

#[tauri::command]
fn list_dir(path: String) -> Result<Vec<FileItem>, String> {
    let entries = std::fs::read_dir(&path).map_err(|e| format!("读取目录失败: {e}"))?;
    let mut items: Vec<FileItem> = entries
        .filter_map(|e| e.ok())
        .map(|e| FileItem {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
        })
        .collect();
    items.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(items)
}

#[tauri::command]
fn read_local_schemas(path: String) -> Vec<String> {
    let base = std::path::Path::new(&path);
    let content = std::fs::read_to_string(base.join("default.custom.yaml"))
        .or_else(|_| std::fs::read_to_string(base.join("default-custom.yaml")))
        .unwrap_or_default();
    parse_schema_list(&content)
}

fn is_default_custom(filename: &str) -> bool {
    filename == "default.custom.yaml" || filename == "default-custom.yaml"
}

fn parse_schema_list(content: &str) -> Vec<String> {
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
                let s = rest.trim().to_string();
                if !s.is_empty() {
                    schemas.push(s);
                }
            } else if !t.is_empty() && !t.starts_with('#') && !t.starts_with('-') {
                in_list = false;
            }
        }
    }
    schemas
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

fn parse_rime_lua_requires(content: &str) -> Vec<String> {
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
        if t.starts_with("--") || t.is_empty() {
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

// Returns (merged_rime_lua, renames) where renames is [(old_module, new_module)].
// Conflicting user modules are renamed to "<name>_user" to avoid overwrite by zip's lua files.
fn merge_rime_lua(
    local_content: &str,
    zip_content: &str,
    zip_lua_filenames: &std::collections::HashSet<String>,
) -> (String, Vec<(String, String)>) {
    use std::collections::HashSet;
    let zip_requires: HashSet<String> = parse_rime_lua_requires(zip_content).into_iter().collect();
    let mut renames: Vec<(String, String)> = Vec::new();
    let mut extra_lines: Vec<String> = Vec::new();
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
            if zip_requires.contains(&module) {
                continue;
            }
            let lua_filename = format!("{}.lua", module);
            if zip_lua_filenames.contains(&lua_filename) {
                let new_name = format!("{}_user", module);
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

    let mut merged = zip_content.to_string();
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

fn merge_default_custom(existing: Option<&str>, zip_content: &str) -> (String, Vec<String>) {
    let keytao: Vec<String> = parse_schema_list(zip_content)
        .into_iter()
        .filter(|s| s.starts_with("keytao"))
        .collect();
    let user: Vec<String> = existing
        .map(|c| {
            parse_schema_list(c)
                .into_iter()
                .filter(|s| !s.starts_with("keytao"))
                .collect()
        })
        .unwrap_or_default();
    let all: Vec<String> = user.iter().chain(keytao.iter()).cloned().collect();

    let mut out = String::new();
    let mut in_list = false;
    for line in zip_content.lines() {
        let t = line.trim();
        if !in_list {
            out.push_str(line);
            out.push('\n');
            if t.contains("schema_list:") {
                in_list = true;
                for s in &all {
                    out.push_str(&format!("    - schema: {s}\n"));
                }
            }
        } else if t.starts_with("- schema:") {
            // skip original entries
        } else {
            in_list = false;
            out.push_str(line);
            out.push('\n');
        }
    }

    (out, user)
}

/// After extraction, verify key files were written correctly.
/// - default.custom.yaml / rime.lua: read back and compare byte-for-byte with expected content
/// - dict / schema / lua files: just check existence
fn verify_install(
    dest: &std::path::Path,
    expected_dc: Option<&str>,
    expected_rl: Option<&str>,
    zip_bytes: &[u8],
) -> Vec<VerifyEntry> {
    use std::io::Read;
    let mut entries: Vec<VerifyEntry> = Vec::new();

    // Verify default.custom.yaml content matches what we wrote
    if let Some(expected) = expected_dc {
        let path = dest.join("default.custom.yaml");
        let label = "default.custom.yaml".to_string();
        match std::fs::read_to_string(&path) {
            Ok(actual) if actual == expected => entries.push(VerifyEntry {
                path: label,
                ok: true,
                note: "内容一致".into(),
            }),
            Ok(_) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: "内容与写入时不符，可能被其他程序修改或写入不完整".into(),
            }),
            Err(e) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: format!("读取失败: {e}"),
            }),
        }
    }

    // Verify rime.lua content matches what we wrote
    if let Some(expected) = expected_rl {
        let path = dest.join("rime.lua");
        let label = "rime.lua".to_string();
        match std::fs::read_to_string(&path) {
            Ok(actual) if actual == expected => entries.push(VerifyEntry {
                path: label,
                ok: true,
                note: "内容一致".into(),
            }),
            Ok(_) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: "内容与写入时不符，可能被其他程序修改或写入不完整".into(),
            }),
            Err(e) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: format!("读取失败: {e}"),
            }),
        }
    }

    // Check that every non-empty zip entry (excluding the two merge-handled files) was written to disk
    if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)) {
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                let raw = file.name().to_string();
                let relative = raw.trim_end_matches('/').to_string();
                if relative.is_empty() || file.is_dir() {
                    continue;
                }
                let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();
                // Only spot-check key file types (schemas, dicts, lua, opencc)
                let is_key = filename.ends_with(".schema.yaml")
                    || filename.ends_with(".dict.yaml")
                    || (filename.ends_with(".lua") && !relative.contains('/'))
                    || relative.starts_with("lua/")
                    || relative.starts_with("opencc/");
                if !is_key {
                    continue;
                }
                // Skip merge-handled files (already verified above)
                if is_default_custom(&filename) || filename == "rime.lua" {
                    continue;
                }
                let on_disk = dest.join(&relative);
                if on_disk.exists() {
                    entries.push(VerifyEntry {
                        path: relative,
                        ok: true,
                        note: "文件存在".into(),
                    });
                } else {
                    entries.push(VerifyEntry {
                        path: relative,
                        ok: false,
                        note: "文件不存在".into(),
                    });
                }
            }
        }
    }
    entries
}

/// Writes `content` to `path`, forcibly overwriting even read-only files.
/// On Linux, triggers a polkit (pkexec) root-auth dialog if the file is root-owned.
/// Returns a tag for logging: "" (normal), " [forced]", or " [root]".
fn write_file_force(path: &std::path::Path, content: &[u8]) -> Result<&'static str, String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    if std::fs::write(path, content).is_ok() {
        return Ok("");
    }
    // Try chmod before falling back to root — works when we own the file but it's read-only
    #[cfg(unix)]
    if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
        if std::fs::write(path, content).is_ok() {
            return Ok(" [forced]");
        }
    }
    write_file_privileged_fallback(path, content)
}

#[cfg(target_os = "linux")]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    content: &[u8],
) -> Result<&'static str, String> {
    let tmp = std::env::temp_dir().join("keytao_privileged_write");
    std::fs::write(&tmp, content).map_err(|e| format!("临时文件写入失败: {e}"))?;
    let result = std::process::Command::new("pkexec")
        .arg("cp")
        .arg("--")
        .arg(&tmp)
        .arg(path)
        .output();
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok(o) if o.status.success() => Ok(" [root]"),
        Ok(o) => Err(format!(
            "需要 root 权限写入 {}，认证失败或被取消: {}",
            path.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("无法启动 pkexec（请确认系统已安装 polkit）: {e}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    _content: &[u8],
) -> Result<&'static str, String> {
    Err(format!("写入失败（权限不足）：{}", path.display()))
}

#[tauri::command]
async fn download_to_temp(app: AppHandle, url: String) -> Result<String, String> {
    let emit = |stage: &str, percent: u32, message: &str| {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            },
        );
    };

    emit("downloading", 0, "正在下载...");

    let client = build_client(&app)?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("下载失败，HTTP {}", response.status()));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded = 0u64;
    let mut bytes: Vec<u8> = if total_size > 0 {
        Vec::with_capacity(total_size as usize)
    } else {
        Vec::new()
    };

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        downloaded += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);
        if total_size > 0 {
            let percent = (downloaded * 60 / total_size) as u32;
            emit(
                "downloading",
                percent,
                &format!(
                    "正在下载... {:.1}MB / {:.1}MB",
                    downloaded as f64 / 1_048_576.0,
                    total_size as f64 / 1_048_576.0
                ),
            );
        }
    }

    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    let temp_path = cache_dir.join("keytao_download.zip");
    std::fs::write(&temp_path, &bytes).map_err(|e| format!("保存临时文件失败: {e}"))?;

    emit("downloading", 60, "下载完成，准备解压...");
    Ok(temp_path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn smart_install<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    zip_path: String,
    dest_path: String,
) -> Result<InstallResult, String> {
    let emit = |stage: &str, percent: u32, message: &str| {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            },
        );
    };

    emit("extracting", 61, "正在解压...");

    let zip_bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
    let dest = PathBuf::from(&dest_path);

    // First pass: collect zip metadata and merge candidates
    let (
        merged_dc_path,
        merged_dc_content,
        merged_schemas,
        merged_rime_lua_path,
        merged_rime_lua_content,
        renamed_lua_files,
    ) = {
        use std::collections::HashSet;
        use std::io::Read;

        let cursor = std::io::Cursor::new(&zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;

        let mut zip_dc_path: Option<String> = None;
        let mut zip_dc_content: Option<String> = None;
        let mut zip_rime_lua_path: Option<String> = None;
        let mut zip_rime_lua_content: Option<String> = None;
        let mut zip_lua_filenames: HashSet<String> = HashSet::new();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let raw = file.name().to_string();
            let relative = raw.trim_end_matches('/').to_string();
            if relative.is_empty() || file.is_dir() {
                continue;
            }
            let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();

            if is_default_custom(&filename) && zip_dc_path.is_none() {
                let mut buf = String::new();
                file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                zip_dc_path = Some(relative);
                zip_dc_content = Some(buf);
            } else if filename == "rime.lua"
                && !relative.contains('/')
                && zip_rime_lua_path.is_none()
            {
                let mut buf = String::new();
                file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                zip_rime_lua_path = Some(relative);
                zip_rime_lua_content = Some(buf);
            } else if relative.starts_with("lua/") && !relative[4..].contains('/') {
                zip_lua_filenames.insert(filename);
            }
        }

        // Merge default.custom.yaml
        let (dc_path, dc_content, schemas) =
            if let (Some(path), Some(content)) = (zip_dc_path, zip_dc_content) {
                let existing = std::fs::read_to_string(dest.join("default.custom.yaml"))
                    .ok()
                    .or_else(|| std::fs::read_to_string(dest.join("default-custom.yaml")).ok());
                let (merged, user) = merge_default_custom(existing.as_deref(), &content);
                (Some(path), Some(merged), user)
            } else {
                (None, None, Vec::new())
            };

        // Merge rime.lua
        let (rl_path, rl_content, renamed) =
            if let (Some(path), Some(zip_rl)) = (zip_rime_lua_path, zip_rime_lua_content) {
                if let Ok(local_rl) = std::fs::read_to_string(dest.join("rime.lua")) {
                    let (merged, renames) = merge_rime_lua(&local_rl, &zip_rl, &zip_lua_filenames);
                    // Read local lua files that need renaming before zip overwrites them
                    let renamed_contents: Vec<(String, Vec<u8>)> = renames
                        .iter()
                        .filter_map(|(old, new)| {
                            let local_file = dest.join("lua").join(format!("{}.lua", old));
                            std::fs::read(&local_file)
                                .ok()
                                .map(|bytes| (new.clone(), bytes))
                        })
                        .collect();
                    (Some(path), Some(merged), renamed_contents)
                } else {
                    (Some(path), Some(zip_rl), Vec::new())
                }
            } else {
                (None, None, Vec::new())
            };

        (dc_path, dc_content, schemas, rl_path, rl_content, renamed)
    };

    // Second pass: smart extraction
    let cursor = std::io::Cursor::new(&zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;
    let total = archive.len();
    let mut logs: Vec<String> = Vec::new();

    for i in 0..total {
        let (relative, is_dir, content) = {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let raw = file.name().to_string();
            let relative = raw.trim_end_matches('/').to_string();
            if relative.is_empty() {
                continue;
            }
            let is_dir = file.is_dir();
            let mut buf = Vec::new();
            if !is_dir {
                use std::io::Read;
                file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            }
            (relative, is_dir, buf)
        };

        if is_dir {
            if let Err(e) = std::fs::create_dir_all(dest.join(&relative)) {
                logs.push(format!("[WARN] mkdir {relative}: {e}"));
            }
        } else if Some(&relative) == merged_dc_path.as_ref() {
            if let Some(ref mc) = merged_dc_content {
                let out = dest.join(&relative);
                match write_file_force(&out, mc.as_bytes()) {
                    Ok(tag) => logs.push(format!("[MERGED]{tag} {relative}")),
                    Err(e) => {
                        logs.push(format!("[ERROR] {relative}: {e}"));
                        return Err(e);
                    }
                }
            }
        } else if Some(&relative) == merged_rime_lua_path.as_ref() {
            if let Some(ref mc) = merged_rime_lua_content {
                let out = dest.join(&relative);
                match write_file_force(&out, mc.as_bytes()) {
                    Ok(tag) => logs.push(format!("[MERGED]{tag} {relative}")),
                    Err(e) => {
                        logs.push(format!("[ERROR] {relative}: {e}"));
                        return Err(e);
                    }
                }
            }
        } else {
            let out = dest.join(&relative);
            match write_file_force(&out, &content) {
                Ok(tag) => logs.push(format!("[OK]{tag} {relative}")),
                Err(e) => {
                    logs.push(format!("[ERROR] {relative}: {e}"));
                    return Err(e);
                }
            }
        }

        let percent = 61 + ((i + 1) * 39 / total) as u32;
        emit(
            "extracting",
            percent,
            &format!("正在安装... {}/{}", i + 1, total),
        );
    }

    // Write renamed user lua files (saved before zip overwrote them)
    for (new_module, bytes) in &renamed_lua_files {
        let out = dest.join("lua").join(format!("{}.lua", new_module));
        match write_file_force(&out, bytes) {
            Ok(tag) => logs.push(format!("[RENAMED]{tag} lua/{new_module}.lua")),
            Err(e) => {
                logs.push(format!("[ERROR] rename lua/{new_module}.lua: {e}"));
                return Err(e);
            }
        }
    }

    std::fs::remove_file(&zip_path).ok();
    emit("done", 100, "安装完成！");

    let verify = verify_install(
        &dest,
        merged_dc_content.as_deref(),
        merged_rime_lua_content.as_deref(),
        &zip_bytes,
    );

    Ok(InstallResult {
        merged_schemas,
        logs,
        verify,
    })
}

// ─── Android plugin ──────────────────────────────────────────────────────────

#[cfg(target_os = "android")]
struct ScopedStorageHandle<R: tauri::Runtime>(tauri::plugin::PluginHandle<R>);

fn scoped_storage_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("scopedStorage")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    api.register_android_plugin("ink.rea.keytao_installer", "ScopedStoragePlugin")?;
                app.manage(ScopedStorageHandle(handle));
            }
            Ok(())
        })
        .build()
}

#[tauri::command]
async fn android_open_app<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    package_name: String,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "openApp",
                serde_json::json!({ "packageName": package_name }),
            )
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_pick_directory<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("pickDirectory", ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_list_files<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tree_uri: String,
) -> Result<Vec<FileItem>, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("listFiles", serde_json::json!({ "treeUri": tree_uri }))
            .map_err(|e| e.to_string())?;

        let files = result["files"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| FileItem {
                        name: v["name"].as_str().unwrap_or("").to_string(),
                        is_dir: v["isDir"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(files)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_read_local_schemas<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tree_uri: String,
) -> Result<Vec<String>, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "readLocalSchemas",
                serde_json::json!({ "treeUri": tree_uri }),
            )
            .map_err(|e| e.to_string())?;

        let schemas = result["schemas"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(schemas)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_smart_extract<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    zip_path: String,
    tree_uri: String,
) -> Result<InstallResult, String> {
    #[cfg(target_os = "android")]
    {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: "extracting".into(),
                percent: 61,
                message: "正在解压...".into(),
            },
        );

        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "smartExtractZip",
                serde_json::json!({ "zipPath": zip_path, "treeUri": tree_uri }),
            )
            .map_err(|e| e.to_string())?;

        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: "done".into(),
                percent: 100,
                message: "安装完成！".into(),
            },
        );

        let merged_schemas = result["mergedSchemas"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let logs = result["logs"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let verify = result["verify"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        Some(VerifyEntry {
                            path: v["path"].as_str()?.to_string(),
                            ok: v["ok"].as_bool().unwrap_or(false),
                            note: v["note"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(InstallResult {
            merged_schemas,
            logs,
            verify,
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[derive(Serialize)]
pub struct InstallerUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
}

#[tauri::command]
async fn check_installer_update(app: AppHandle) -> Result<InstallerUpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let client = build_client(&app)?;
    let resp = client
        .get("https://api.github.com/repos/xkinput/keytao-installer/releases/latest")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let latest_tag = json["tag_name"].as_str().unwrap_or("").to_string();
    let latest = latest_tag.trim_start_matches('v').to_string();
    let release_url = json["html_url"].as_str().unwrap_or("").to_string();
    let has_update = !latest.is_empty() && latest != current;
    Ok(InstallerUpdateInfo {
        current_version: current,
        latest_version: latest,
        has_update,
        release_url,
    })
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(scoped_storage_plugin())
        .invoke_handler(tauri::generate_handler![
            check_installer_update,
            fetch_latest_release,
            select_directory,
            download_to_temp,
            list_dir,
            read_local_schemas,
            smart_install,
            android_open_app,
            android_pick_directory,
            android_list_files,
            android_read_local_schemas,
            android_smart_extract,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ── parse_rime_lua_requires ───────────────────────────────────────────────

    #[test]
    fn test_parse_requires_basic() {
        let content = "keytao_filter = require(\"keytao_filter\")\nfoo = require('bar')\n";
        let r = parse_rime_lua_requires(content);
        assert_eq!(r, vec!["keytao_filter", "bar"]);
    }

    #[test]
    fn test_parse_requires_skips_single_line_comments() {
        let content = "-- foo = require(\"foo\")\nreal = require(\"real\")\n";
        let r = parse_rime_lua_requires(content);
        assert_eq!(r, vec!["real"]);
    }

    #[test]
    fn test_parse_requires_skips_block_comment_content() {
        let content = "--[[\n  foo = require(\"bar\")\n--]]\nreal = require(\"real\")\n";
        let r = parse_rime_lua_requires(content);
        assert_eq!(r, vec!["real"]);
    }

    // ── merge_rime_lua ────────────────────────────────────────────────────────

    #[test]
    fn test_merge_appends_unique_local_require() {
        let local = "my_mod = require(\"my_mod\")\n";
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(merged.contains("require(\"keytao_filter\")"));
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_skips_require_already_in_zip() {
        let local = "keytao_filter = require(\"keytao_filter\")\n";
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, _) = merge_rime_lua(local, zip, &HashSet::new());
        assert_eq!(merged.matches("require(\"keytao_filter\")").count(), 1);
    }

    #[test]
    fn test_merge_renames_conflicting_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let zip = "keytao = require(\"keytao\")\n";
        let filenames: HashSet<String> = ["my_mod.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua(local, zip, &filenames);
        assert_eq!(
            renames,
            vec![("my_mod".to_string(), "my_mod_user".to_string())]
        );
        assert!(merged.contains("require(\"my_mod_user\")"));
        assert!(!merged.contains("require(\"my_mod\")"));
    }

    #[test]
    fn test_merge_ignores_block_comment_content() {
        // Reproduces the Android bug: block comment lines such as ``` were
        // appended verbatim to the merged output because the loop did not
        // track --[[ ... --]] state.
        let local = concat!(
            "--[[\n",
            "librime-lua 样例\n",
            "```\n",
            "  engine:\n",
            "    translators:\n",
            "```\n",
            "--]]\n",
            "--[[\n",
            "各例可使用 `require` 引入。\n",
            "```\n",
            "  foo = require(\"bar\")\n",
            "```\n",
            "--]]\n",
            "my_mod = require(\"my_mod\")\n",
        );
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(!merged.contains("librime-lua"), "block comment line leaked");
        assert!(!merged.contains("engine:"), "block comment line leaked");
        assert!(!merged.contains("```"), "block comment backticks leaked");
        assert!(
            !merged.contains("require(\"bar\")"),
            "in-comment require leaked"
        );
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    // ── parse_schema_list ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_schema_list_basic() {
        let content = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        assert_eq!(parse_schema_list(content), vec!["keytao_b", "keytao_bg"]);
    }

    #[test]
    fn test_parse_schema_list_stops_at_non_schema() {
        let content = "patch:\n  schema_list:\n    - schema: foo\n  other_key: val\n";
        assert_eq!(parse_schema_list(content), vec!["foo"]);
    }

    // ── merge_default_custom ──────────────────────────────────────────────────

    #[test]
    fn test_merge_dc_preserves_user_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: another\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert!(merged.contains("- schema: my_schema"));
        assert!(merged.contains("- schema: another"));
        assert!(merged.contains("- schema: keytao_b"));
        assert_eq!(user, vec!["my_schema", "another"]);
    }

    #[test]
    fn test_merge_dc_excludes_user_keytao_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: keytao_b\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert_eq!(user, vec!["my_schema"]);
        assert!(merged.contains("- schema: keytao_b"));
        assert!(merged.contains("- schema: keytao_bg"));
    }

    #[test]
    fn test_merge_dc_no_existing_file() {
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n";
        let (merged, user) = merge_default_custom(None, zip);
        assert!(user.is_empty());
        assert!(merged.contains("- schema: keytao_b"));
    }

    // ── real keytao rime.lua ──────────────────────────────────────────────────

    const KEYTAO_RIME_LUA: &str = concat!(
        "--[[\n",
        "librime-lua 样例\n",
        "```\n",
        "  engine:\n",
        "    translators:\n",
        "      - lua_translator@lua_function3\n",
        "      - lua_translator@lua_function4\n",
        "    filters:\n",
        "      - lua_filter@lua_function1\n",
        "      - lua_filter@lua_function2\n",
        "```\n",
        "其中各 `lua_function` 为在本文件所定义变量名。\n",
        "--]]\n",
        "\n",
        "--[[\n",
        "本文件的后面是若干个例子，按照由简单到复杂的顺序示例了 librime-lua 的用法。\n",
        "每个例子都被组织在 `lua` 目录下的单独文件中，打开对应文件可看到实现和注解。\n",
        "\n",
        "各例可使用 `require` 引入。\n",
        "```\n",
        "  foo = require(\"bar\")\n",
        "```\n",
        "可认为是载入 `lua/bar.lua` 中的例子，并起名为 `foo`。\n",
        "配方文件中的引用方法为：`...@foo`。\n",
        "--]]\n",
        "\n",
        "date_time_translator = require(\"date_time\")\n",
        "\n",
        "\n",
        "-- single_char_filter: 候选项重排序，使单字优先\n",
        "-- 详见 `lua/single_char.lua`\n",
        "-- single_char_filter = require(\"single_char\")\n",
        "\n",
        "\n",
        "-- keytao_filter: 单字模式 & 630 即 ss 词组提示\n",
        "-- 详见 `lua/keytao_filter.lua`\n",
        "keytao_filter = require(\"keytao_filter\")\n",
        "\n",
        "-- 顶功处理器\n",
        "topup_processor = require(\"for_topup\")\n",
        "\n",
        "-- 声笔笔简码提示 | 顶功提示 | 补全处理\n",
        "hint_filter = require(\"for_hint\")\n",
        "\n",
        "-- number_translator: 将 `=` + 阿拉伯数字 翻译为大小写汉字\n",
        "number_translator = require(\"xnumber\")\n",
        "\n",
        "-- 用 ' 作为次选键\n",
        "smart_2 = require(\"smart_2\")\n",
    );

    #[test]
    fn test_parse_requires_keytao_rime_lua() {
        // Block comment contains `foo = require("bar")` which must NOT be included.
        let requires = parse_rime_lua_requires(KEYTAO_RIME_LUA);
        assert_eq!(
            requires,
            vec![
                "date_time",
                "keytao_filter",
                "for_topup",
                "for_hint",
                "xnumber",
                "smart_2"
            ]
        );
        assert!(
            !requires.contains(&"bar".to_string()),
            "in-comment require must not be parsed"
        );
    }

    #[test]
    fn test_merge_reinstall_no_duplicates() {
        // Installing over an existing identical rime.lua should produce the same file.
        let (merged, renames) = merge_rime_lua(KEYTAO_RIME_LUA, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(renames.is_empty());
        // Every require should appear exactly once.
        for module in &[
            "date_time",
            "keytao_filter",
            "for_topup",
            "for_hint",
            "xnumber",
            "smart_2",
        ] {
            let needle = format!("require(\"{module}\")");
            assert_eq!(
                merged.matches(needle.as_str()).count(),
                1,
                "require(\"{module}\") duplicated after reinstall"
            );
        }
    }

    #[test]
    fn test_merge_user_extra_module_appended() {
        // User has the keytao rime.lua as local, plus one extra module.
        let local = format!("{KEYTAO_RIME_LUA}my_custom = require(\"my_custom\")\n");
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(renames.is_empty());
        assert!(merged.contains("require(\"my_custom\")"));
        // Keytao requires still appear exactly once.
        assert_eq!(merged.matches("require(\"keytao_filter\")").count(), 1);
    }

    #[test]
    fn test_merge_user_extra_module_conflict_renamed() {
        // User has a custom `date_time.lua` that would be overwritten by zip.
        let local = format!("{KEYTAO_RIME_LUA}my_dt = require(\"my_dt\")\n");
        let filenames: HashSet<String> = ["my_dt.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &filenames);
        assert_eq!(
            renames,
            vec![("my_dt".to_string(), "my_dt_user".to_string())]
        );
        assert!(merged.contains("require(\"my_dt_user\")"));
        assert!(!merged.contains("require(\"my_dt\")"));
    }

    // ── zip overwrites local keytao content ──────────────────────────────────

    #[test]
    fn test_merge_zip_is_base_local_keytao_no_duplicates() {
        // Local already has the same keytao rime.lua; merged must equal zip exactly.
        let (merged, renames) = merge_rime_lua(KEYTAO_RIME_LUA, KEYTAO_RIME_LUA, &HashSet::new());
        assert_eq!(merged, KEYTAO_RIME_LUA);
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_old_keytao_missing_module_zip_provides_it() {
        // Local = older keytao rime.lua without smart_2.
        // Zip = new keytao rime.lua with smart_2.
        // smart_2 must appear exactly once in merged output.
        let old_local: String = KEYTAO_RIME_LUA
            .lines()
            .filter(|l| !l.trim_start().starts_with("smart_2"))
            .collect::<Vec<_>>()
            .join("\n");
        let (merged, renames) = merge_rime_lua(&old_local, KEYTAO_RIME_LUA, &HashSet::new());
        assert_eq!(merged.matches("require(\"smart_2\")").count(), 1);
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_user_extra_preserved_zip_overwrites_keytao_no_dups() {
        // Local = keytao rime.lua + user-defined module.
        // Zip = same keytao rime.lua (re-install / upgrade).
        // merged must start with zip content; user module appended once;
        // every keytao module appears exactly once.
        let local = format!("{KEYTAO_RIME_LUA}user_plugin = require(\"user_plugin\")\n");
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(merged.starts_with(KEYTAO_RIME_LUA));
        assert!(merged.contains("require(\"user_plugin\")"));
        for module in &[
            "date_time",
            "keytao_filter",
            "for_topup",
            "for_hint",
            "xnumber",
            "smart_2",
        ] {
            assert_eq!(
                merged.matches(&format!("require(\"{module}\")")).count(),
                1,
                "require(\"{module}\") must appear exactly once"
            );
        }
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_keytao_rime_lua_no_block_comment_leak() {
        // Using actual keytao rime.lua as local; merged result must not contain
        // any content from the --[[ ]] header blocks.
        let local = KEYTAO_RIME_LUA;
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, _) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(
            !merged.contains("librime-lua"),
            "block comment header leaked"
        );
        assert!(!merged.contains("engine:"), "block comment content leaked");
        assert!(
            !merged.contains("```"),
            "backticks from block comment leaked"
        );
        assert!(
            !merged.contains("require(\"bar\")"),
            "in-comment require leaked"
        );
    }
}
