# 键道安装器

键道输入方案的一键安装工具，基于 [Tauri](https://tauri.app/) 构建，支持桌面端与 Android。

## 功能

- 自动从 GitHub 获取最新键道版本并下载
- 智能合并 `default.custom.yaml`，保留用户已有的非键道方案
- 安装进度实时展示
- 自动检测 Rime 配置目录（也可手动选择）
- 安装完成后提示重新部署 Rime

## 支持平台

| 平台 | Rime 前端 | 配置目录 |
|------|-----------|----------|
| macOS | 鼠须管（Squirrel） | `~/Library/Rime` |
| Windows | 小狼毫（Weasel） | `%APPDATA%\Rime` |
| Linux | Fcitx5-Rime / iBus-Rime | `~/.local/share/fcitx5/rime` |
| Android | 同文输入法（Trime） | `/sdcard/rime`（SAF） |
| iOS | iRime | 需手动导入，仅提供下载链接 |

## 下载

前往 [Releases](https://github.com/xkinput/keytao-installer/releases) 下载对应平台的安装包。

## 开发

```bash
pnpm install
pnpm tauri dev
```

构建：

```bash
pnpm tauri build
```

Android 构建需要配置 Android SDK 与 NDK，参考 [Tauri 移动端文档](https://tauri.app/start/prerequisites/#android)。

## 许可证

MIT
