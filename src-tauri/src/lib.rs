use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

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

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("keytao-installer/0.1.0")
        .build()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn fetch_latest_release() -> Result<ReleaseInfo, String> {
    let client = build_client()?;

    let release: serde_json::Value = client
        .get("https://api.github.com/repos/xkinput/KeyTao/releases/latest")
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
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
            } else if asset_name.contains("android") {
                urls.android = Some(url);
            } else if asset_name.contains("linux") {
                urls.linux = Some(url);
            }
        }
    }

    Ok(ReleaseInfo {
        version,
        name,
        published_at,
        download_urls: urls,
    })
}

fn rime_default_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // Squirrel: ~/Library/Rime
        dirs::home_dir().map(|h| h.join("Library/Rime"))
    }
    #[cfg(target_os = "windows")]
    {
        // Weasel: %APPDATA%\Rime
        dirs::config_dir().map(|c| c.join("Rime"))
    }
    #[cfg(target_os = "linux")]
    {
        // Prefer whichever exists: fcitx5 > ibus, fall back to fcitx5 path
        let home = dirs::home_dir()?;
        let fcitx5 = home.join(".local/share/fcitx5/rime");
        let ibus = home.join(".config/ibus/rime");
        if fcitx5.exists() {
            Some(fcitx5)
        } else if ibus.exists() {
            Some(ibus)
        } else {
            // Neither exists yet; default to fcitx5 (more common on modern distros)
            Some(fcitx5)
        }
    }
    #[cfg(target_os = "android")]
    {
        Some(std::path::PathBuf::from("/sdcard/rime"))
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "android"
    )))]
    {
        None
    }
}

#[tauri::command]
async fn select_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();

    let mut builder = app.dialog().file();
    if let Some(default) = rime_default_path() {
        builder = builder.set_directory(default);
    }
    builder.pick_folder(move |folder| {
        let _ = tx.send(folder);
    });

    let result = rx.await.map_err(|e| e.to_string())?;
    Ok(result.map(|p| p.to_string()))
}

#[tauri::command]
async fn download_and_install(
    app: AppHandle,
    url: String,
    dest_path: String,
) -> Result<(), String> {
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

    let client = build_client()?;
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
            let percent = (downloaded * 50 / total_size) as u32;
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

    emit("extracting", 50, "正在解压...");

    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;
    let total_files = archive.len();
    let dest = PathBuf::from(&dest_path);

    for i in 0..total_files {
        let mut file = archive.by_index(i).map_err(|e| format!("读取压缩包失败: {e}"))?;
        let raw = file.name().to_string();

        // Strip the top-level folder prefix (e.g. "keytao-mac/foo" → "foo")
        let relative = raw.splitn(2, '/').nth(1).unwrap_or("").trim_end_matches('/');
        if relative.is_empty() {
            continue;
        }

        let out_path = dest.join(relative);

        if file.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("创建目录失败 {relative}: {e}"))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建父目录失败: {e}"))?;
            }
            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("创建文件失败 {relative}: {e}"))?;
            std::io::copy(&mut file, &mut out_file)
                .map_err(|e| format!("写入文件失败 {relative}: {e}"))?;
        }

        let percent = 50 + ((i + 1) * 50 / total_files) as u32;
        emit("extracting", percent, &format!("正在解压... {}/{}", i + 1, total_files));
    }

    emit("done", 100, "安装完成！");
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .invoke_handler(tauri::generate_handler![
            fetch_latest_release,
            select_directory,
            download_and_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
