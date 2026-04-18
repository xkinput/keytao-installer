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

const CACHE_TTL_SECS: u64 = 3600;

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
pub struct InstallResult {
    pub merged_schemas: Vec<String>,
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

    if let Some(ref c) = cached {
        if now.saturating_sub(c.cached_at) < CACHE_TTL_SECS {
            return Ok(c.release.clone());
        }
    }

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

    let version = release["tag_name"].as_str().unwrap_or("unknown").to_string();
    let name = release["name"].as_str().unwrap_or("").to_string();
    let published_at = release["published_at"].as_str().unwrap_or("").to_string();

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

    let info = ReleaseInfo { version, name, published_at, download_urls: urls };

    if let (Some(path), false) = (cache_path, etag.is_empty()) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let cache = ReleaseCache { etag, cached_at: now, release: info.clone() };
        serde_json::to_string(&cache).ok().and_then(|s| std::fs::write(&path, s).ok());
    }

    Ok(info)
}

fn rime_default_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    { dirs::home_dir().map(|h| h.join("Library/Rime")) }
    #[cfg(target_os = "windows")]
    { dirs::config_dir().map(|c| c.join("Rime")) }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir()?;
        let fcitx5 = home.join(".local/share/fcitx5/rime");
        let ibus = home.join(".config/ibus/rime");
        if fcitx5.exists() { Some(fcitx5) } else if ibus.exists() { Some(ibus) } else { Some(fcitx5) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { None }
}

#[tauri::command]
async fn select_directory(#[allow(unused_variables)] app: AppHandle, im_type: Option<String>) -> Result<Option<String>, String> {
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
                    "ibus"   => Some(home.join(".config/ibus/rime")),
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
    for line in content.lines() {
        let t = line.trim();
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

    for line in local_content.lines() {
        let t = line.trim();
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
        .map(|c| parse_schema_list(c).into_iter().filter(|s| !s.starts_with("keytao")).collect())
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

#[tauri::command]
async fn download_to_temp(app: AppHandle, url: String) -> Result<String, String> {
    let emit = |stage: &str, percent: u32, message: &str| {
        let _ = app.emit("install-progress", InstallProgress {
            stage: stage.to_string(),
            percent,
            message: message.to_string(),
        });
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
        let _ = app.emit("install-progress", InstallProgress {
            stage: stage.to_string(),
            percent,
            message: message.to_string(),
        });
    };

    emit("extracting", 61, "正在解压...");

    let zip_bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
    let dest = PathBuf::from(&dest_path);

    // First pass: collect zip metadata and merge candidates
    let (merged_dc_path, merged_dc_content, merged_schemas, merged_rime_lua_path, merged_rime_lua_content, renamed_lua_files) = {
        use std::io::Read;
        use std::collections::HashSet;

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
            let relative = raw.splitn(2, '/').nth(1).unwrap_or("").trim_end_matches('/').to_string();
            if relative.is_empty() || file.is_dir() {
                continue;
            }
            let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();

            if is_default_custom(&filename) && zip_dc_path.is_none() {
                let mut buf = String::new();
                file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                zip_dc_path = Some(relative);
                zip_dc_content = Some(buf);
            } else if filename == "rime.lua" && !relative.contains('/') && zip_rime_lua_path.is_none() {
                let mut buf = String::new();
                file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                zip_rime_lua_path = Some(relative);
                zip_rime_lua_content = Some(buf);
            } else if relative.starts_with("lua/") && !relative[4..].contains('/') {
                zip_lua_filenames.insert(filename);
            }
        }

        // Merge default.custom.yaml
        let (dc_path, dc_content, schemas) = if let (Some(path), Some(content)) = (zip_dc_path, zip_dc_content) {
            let existing = std::fs::read_to_string(dest.join("default.custom.yaml"))
                .ok()
                .or_else(|| std::fs::read_to_string(dest.join("default-custom.yaml")).ok());
            let (merged, user) = merge_default_custom(existing.as_deref(), &content);
            (Some(path), Some(merged), user)
        } else {
            (None, None, Vec::new())
        };

        // Merge rime.lua
        let (rl_path, rl_content, renamed) = if let (Some(path), Some(zip_rl)) = (zip_rime_lua_path, zip_rime_lua_content) {
            if let Ok(local_rl) = std::fs::read_to_string(dest.join("rime.lua")) {
                let (merged, renames) = merge_rime_lua(&local_rl, &zip_rl, &zip_lua_filenames);
                // Read local lua files that need renaming before zip overwrites them
                let renamed_contents: Vec<(String, Vec<u8>)> = renames
                    .iter()
                    .filter_map(|(old, new)| {
                        let local_file = dest.join("lua").join(format!("{}.lua", old));
                        std::fs::read(&local_file).ok().map(|bytes| (new.clone(), bytes))
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

    for i in 0..total {
        let (relative, is_dir, content) = {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let raw = file.name().to_string();
            let relative = raw.splitn(2, '/').nth(1).unwrap_or("").trim_end_matches('/').to_string();
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
            std::fs::create_dir_all(dest.join(&relative)).ok();
        } else if Some(&relative) == merged_dc_path.as_ref() {
            if let Some(ref mc) = merged_dc_content {
                let out = dest.join(&relative);
                if let Some(p) = out.parent() {
                    std::fs::create_dir_all(p).ok();
                }
                std::fs::write(&out, mc.as_bytes())
                    .map_err(|e| format!("写入失败 {relative}: {e}"))?;
            }
        } else if Some(&relative) == merged_rime_lua_path.as_ref() {
            if let Some(ref mc) = merged_rime_lua_content {
                let out = dest.join(&relative);
                if let Some(p) = out.parent() {
                    std::fs::create_dir_all(p).ok();
                }
                std::fs::write(&out, mc.as_bytes())
                    .map_err(|e| format!("写入失败 {relative}: {e}"))?;
            }
        } else {
            let out = dest.join(&relative);
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("创建目录失败: {e}"))?;
            }
            std::fs::write(&out, &content).map_err(|e| format!("写入失败 {relative}: {e}"))?;
        }

        let percent = 61 + ((i + 1) * 39 / total) as u32;
        emit("extracting", percent, &format!("正在安装... {}/{}", i + 1, total));
    }

    // Write renamed user lua files (saved before zip overwrote them)
    for (new_module, bytes) in &renamed_lua_files {
        let out = dest.join("lua").join(format!("{}.lua", new_module));
        std::fs::write(&out, bytes).map_err(|e| format!("写入重命名文件失败 {new_module}: {e}"))?;
    }

    std::fs::remove_file(&zip_path).ok();
    emit("done", 100, "安装完成！");

    Ok(InstallResult { merged_schemas })
}

// ─── Android plugin ──────────────────────────────────────────────────────────

#[cfg(target_os = "android")]
struct ScopedStorageHandle<R: tauri::Runtime>(tauri::plugin::PluginHandle<R>);

fn scoped_storage_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("scopedStorage")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin(
                    "ink.rea.keytao_installer",
                    "ScopedStoragePlugin",
                )?;
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
            .run_mobile_plugin("openApp", serde_json::json!({ "packageName": package_name }))
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
            .run_mobile_plugin("readLocalSchemas", serde_json::json!({ "treeUri": tree_uri }))
            .map_err(|e| e.to_string())?;

        let schemas = result["schemas"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
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
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        Ok(InstallResult { merged_schemas })
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
    Ok(InstallerUpdateInfo { current_version: current, latest_version: latest, has_update, release_url })
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
