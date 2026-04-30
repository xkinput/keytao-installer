import { useState, useEffect, useRef } from "react"
import { invoke } from "@tauri-apps/api/core"
import { getVersion } from "@tauri-apps/api/app"
import { listen } from "@tauri-apps/api/event"
import { platform } from "@tauri-apps/plugin-os"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog"
import {
  FolderOpen,
  Download,
  CheckCircle2,
  AlertTriangle,
  Terminal,
  Smartphone,
  Monitor,
  Apple,
  ExternalLink,
  RefreshCw,
  FileText,
  Folder,
  Info,
} from "lucide-react"

type OSType = "windows" | "macos" | "linux" | "android" | "ios" | "unknown"

interface InstallerUpdateInfo {
  current_version: string
  latest_version: string
  has_update: boolean
  release_url: string
}

interface ReleaseInfo {
  version: string
  name: string
  published_at: string
  body: string
  download_urls: {
    macos?: string
    windows?: string
    linux?: string
    android?: string
  }
}

interface InstallProgress {
  stage: string
  percent: number
  message: string
}

interface FileItem {
  name: string
  is_dir: boolean
}

interface VerifyEntry {
  path: string
  ok: boolean
  note: string
}

interface InstallResult {
  merged_schemas: string[]
  logs: string[]
  verify: VerifyEntry[]
}

interface RimeInfo {
  name: string
  url: string
  configPath: string
  commands?: string[]
  note?: string
  nixosNote?: string
  nixosUrl?: string
  appPackage?: string
}

const OS_META: Record<string, { label: string; icon: React.ReactNode; rime: RimeInfo }> = {
  macos: {
    label: "macOS",
    icon: <Apple className="h-4 w-4" />,
    rime: {
      name: "鼠须管（Squirrel）",
      url: "https://rime.im/download/#macOS",
      configPath: "~/Library/Rime",
      commands: ["brew install --cask squirrel"],
    },
  },
  windows: {
    label: "Windows",
    icon: <Monitor className="h-4 w-4" />,
    rime: {
      name: "小狼毫（Weasel）",
      url: "https://rime.im/download/#Windows",
      configPath: "%APPDATA%\\Rime",
      note: "从官网下载 exe 安装包",
    },
  },
  linux: {
    label: "Linux",
    icon: <Terminal className="h-4 w-4" />,
    rime: {
      name: "Fcitx5-Rime / iBus-Rime",
      url: "https://rime.im/download/#Linux",
      configPath: "~/.local/share/fcitx5/rime  或  ~/.config/ibus/rime",
      commands: [
        "sudo apt install fcitx5-rime        # Ubuntu/Debian",
        "sudo pacman -S fcitx5-rime          # Arch",
        "sudo dnf install fcitx5-rime        # Fedora",
      ],
      nixosNote: "NixOS 用户请参考专用安装文档",
      nixosUrl: "https://github.com/xkinput/KeyTao/blob/master/INSTALL_NIXOS.md",
    },
  },
  android: {
    label: "Android",
    icon: <Smartphone className="h-4 w-4" />,
    rime: {
      name: "同文输入法（Trime）",
      url: "https://github.com/osfans/trime/releases",
      configPath: "/sdcard/rime",
      note: "从 GitHub Releases 下载 APK，或通过 F-Droid 安装",
      appPackage: "com.osfans.trime",
    },
  },
  ios: {
    label: "iOS",
    icon: <Apple className="h-4 w-4" />,
    rime: {
      name: "iRime",
      url: "https://apps.apple.com/app/irime/id1142623977",
      configPath: "通过 iCloud 或文件共享导入",
      note: "App Store 搜索 iRime",
    },
  },
}

function InfoRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-3 py-2">
      <span className="text-muted-foreground text-xs min-w-24 pt-0.5 shrink-0">{label}</span>
      <div className="text-sm min-w-0 flex-1">{children}</div>
    </div>
  )
}

function CodeBlock({ children }: { children: string }) {
  return (
    <code className="block text-xs font-mono bg-muted/60 text-muted-foreground px-3 py-1.5 rounded-md">
      {children}
    </code>
  )
}

function safUriToDisplayPath(uri: string): string {
  try {
    const treeId = decodeURIComponent(uri.split("/tree/")[1] || "")
    return "/" + treeId.replace("primary:", "sdcard/")
  } catch {
    return uri
  }
}

function FileList({
  files,
  loading,
  onRefresh,
  disabled,
}: {
  files: FileItem[]
  loading: boolean
  onRefresh: () => void
  disabled: boolean
}) {
  return (
    <div className="rounded-lg border border-border overflow-hidden">
      <div className="flex items-center justify-between px-3 py-1.5 bg-muted/30 border-b border-border">
        <span className="text-xs text-muted-foreground">{files.length} 个项目</span>
        <button
          onClick={onRefresh}
          disabled={loading || disabled}
          className="text-muted-foreground hover:text-foreground disabled:opacity-40 transition-colors"
          title="刷新"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>
      <div className="max-h-48 overflow-y-auto">
        {loading ? (
          <div className="flex items-center justify-center py-6 text-xs text-muted-foreground gap-2">
            <RefreshCw className="h-3.5 w-3.5 animate-spin" />
            读取中...
          </div>
        ) : files.length === 0 ? (
          <div className="py-6 text-center text-xs text-muted-foreground">目录为空</div>
        ) : (
          files.map((item, i) => (
            <div
              key={i}
              className="flex items-center gap-2 px-3 py-1.5 border-b border-border/40 last:border-0 hover:bg-muted/20"
            >
              {item.is_dir ? (
                <Folder className="h-3.5 w-3.5 text-amber-500 shrink-0" />
              ) : (
                <FileText className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
              )}
              <span className="text-xs font-mono truncate">{item.name}</span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

export default function App() {
  const [osType, setOsType] = useState<OSType>("unknown")
  const [appVersion, setAppVersion] = useState<string>("")
  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo | null>(null)
  const [releaseError, setReleaseError] = useState<string | null>(null)
  const [isFetchingRelease, setIsFetchingRelease] = useState(true)
  const [installerUpdate, setInstallerUpdate] = useState<InstallerUpdateInfo | null>(null)

  // Linux IM type selection
  const [linuxImType, setLinuxImType] = useState<"fcitx5" | "ibus" | null>(null)

  // Directory selection
  const [selectedDir, setSelectedDir] = useState<string | null>(null)   // display path
  const [safUri, setSafUri] = useState<string | null>(null)             // Android SAF URI

  // File preview
  const [files, setFiles] = useState<FileItem[]>([])
  const [isLoadingFiles, setIsLoadingFiles] = useState(false)
  const [localSchemas, setLocalSchemas] = useState<string[] | null>(null)

  // Install
  const [progress, setProgress] = useState<InstallProgress | null>(null)
  const [isInstalling, setIsInstalling] = useState(false)
  const [installResult, setInstallResult] = useState<InstallResult | null>(null)
  const [installError, setInstallError] = useState<string | null>(null)
  const [showChangelog, setShowChangelog] = useState(false)

  const unlistenRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    const p = platform()
    const map: Record<string, OSType> = {
      macos: "macos", windows: "windows", linux: "linux",
      android: "android", ios: "ios",
    }
    setOsType(map[p] ?? "unknown")
    getVersion().then(setAppVersion).catch(() => { })

    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))

    invoke<InstallerUpdateInfo>("check_installer_update")
      .then((info) => { if (info.has_update) setInstallerUpdate(info) })
      .catch(() => { })

    listen<InstallProgress>("install-progress", (e) => {
      setProgress(e.payload)
    }).then((fn) => { unlistenRef.current = fn })

    return () => unlistenRef.current?.()
  }, [])

  const osMeta = OS_META[osType]
  const downloadUrl = releaseInfo?.download_urls[osType as keyof typeof releaseInfo.download_urls]

  async function loadFiles(path?: string, uri?: string) {
    setIsLoadingFiles(true)
    try {
      if (osType === "android" && uri) {
        const [items, schemas] = await Promise.all([
          invoke<FileItem[]>("android_list_files", { treeUri: uri }),
          invoke<string[]>("android_read_local_schemas", { treeUri: uri }).catch(() => null),
        ])
        setFiles(items)
        setLocalSchemas(schemas)
      } else if (path) {
        const [items, schemas] = await Promise.all([
          invoke<FileItem[]>("list_dir", { path }),
          invoke<string[]>("read_local_schemas", { path }).catch(() => null),
        ])
        setFiles(items)
        setLocalSchemas(schemas)
      }
    } catch {
      setFiles([])
      setLocalSchemas(null)
    } finally {
      setIsLoadingFiles(false)
    }
  }

  async function handleSelectDir() {
    setLocalSchemas(null)
    setFiles([])
    if (osType === "android") {
      try {
        const { uri } = await invoke<{ uri: string }>("android_pick_directory")
        const displayPath = safUriToDisplayPath(uri)
        setSafUri(uri)
        setSelectedDir(displayPath)
        setInstallResult(null)
        setInstallError(null)
        await loadFiles(undefined, uri)
      } catch (e) {
        setInstallError(String(e))
      }
    } else {
      try {
        const imType = osType === "linux" ? linuxImType : null
        const dir = await invoke<string | null>("select_directory", { imType })
        if (dir) {
          setSelectedDir(dir)
          setSafUri(null)
          setInstallResult(null)
          setInstallError(null)
          await loadFiles(dir)
        }
      } catch (e) {
        setInstallError(String(e))
      }
    }
  }

  async function handleRefreshFiles() {
    await loadFiles(selectedDir ?? undefined, safUri ?? undefined)
  }

  async function handleInstall() {
    if (!selectedDir || !downloadUrl) return
    setIsInstalling(true)
    setInstallResult(null)
    setInstallError(null)
    setProgress(null)

    try {
      const tempPath = await invoke<string>("download_to_temp", { url: downloadUrl })

      let result: InstallResult
      if (osType === "android" && safUri) {
        result = await invoke<InstallResult>("android_smart_extract", {
          zipPath: tempPath,
          treeUri: safUri,
        })
      } else {
        result = await invoke<InstallResult>("smart_install", {
          zipPath: tempPath,
          destPath: selectedDir,
        })
      }

      setInstallResult(result)
      await loadFiles(selectedDir ?? undefined, safUri ?? undefined)
    } catch (e) {
      setInstallError(String(e))
    } finally {
      setIsInstalling(false)
      setProgress(null)
    }
  }

  async function handleRefetchRelease() {
    setIsFetchingRelease(true)
    setReleaseError(null)
    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))
  }

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="max-w-2xl mx-auto px-4 py-8 space-y-4">

        {/* Header */}
        <div className="text-center space-y-2 pb-2">
          <img src="/logo.png" alt="键道安装器" className="h-16 w-16 mx-auto" />
          <h1 className="text-2xl font-bold tracking-tight">
            键道安装器{appVersion && <span className="ml-2 text-base font-normal text-muted-foreground">v{appVersion}</span>}
          </h1>
          <p className="text-sm text-muted-foreground">
            自动下载最新版键道输入方案并安装到 Rime 配置目录
          </p>
        </div>

        {/* Installer update banner */}
        {installerUpdate && (
          <a
            href={installerUpdate.release_url}
            target="_blank"
            rel="noreferrer"
            className="flex items-center justify-between gap-3 px-4 py-2.5 rounded-lg border border-primary/30 bg-primary/5 text-sm hover:bg-primary/10 transition-colors"
          >
            <div className="flex items-center gap-2">
              <Download className="h-4 w-4 text-primary shrink-0" />
              <span>安装器有新版本可用</span>
              <Badge variant="secondary" className="font-mono text-xs">
                v{installerUpdate.latest_version}
              </Badge>
            </div>
            <div className="flex items-center gap-2 text-muted-foreground text-xs shrink-0">
              <span>当前 v{installerUpdate.current_version}</span>
              <ExternalLink className="h-3 w-3" />
            </div>
          </a>
        )}

        {/* Step 1: Rime 安装指南 */}
        {osMeta && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <span className="flex items-center justify-center w-5 h-5 rounded-full bg-primary text-primary-foreground text-xs font-bold">1</span>
                安装 Rime 输入法
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center gap-2 text-sm">
                {osMeta.icon}
                <span className="font-medium">{osMeta.label}</span>
                <Separator orientation="vertical" className="h-4" />
                <span className="text-muted-foreground">{osMeta.rime.name}</span>
              </div>

              <Separator />

              <div className="space-y-1 divide-y divide-border">
                {osMeta.rime.note && (
                  <InfoRow label="安装方式">
                    <span className="text-muted-foreground">{osMeta.rime.note}</span>
                  </InfoRow>
                )}
                {osMeta.rime.commands && (
                  <InfoRow label="命令安装">
                    <div className="space-y-1 w-full">
                      {osMeta.rime.commands.map((cmd, i) => (
                        <CodeBlock key={i}>{cmd}</CodeBlock>
                      ))}
                    </div>
                  </InfoRow>
                )}
                <InfoRow label="下载地址">
                  <a
                    href={osMeta.rime.url}
                    target="_blank"
                    rel="noreferrer"
                    className="text-primary hover:underline inline-flex items-start gap-1 break-all"
                  >
                    <span className="break-all">{osMeta.rime.url}</span>
                    <ExternalLink className="h-3 w-3 shrink-0 mt-0.5" />
                  </a>
                </InfoRow>
                <InfoRow label="配置目录">
                  <code className="text-xs bg-muted px-2 py-0.5 rounded font-mono break-all">
                    {osType === "linux"
                      ? linuxImType === "ibus"
                        ? "~/.config/ibus/rime"
                        : "~/.local/share/fcitx5/rime"
                      : osMeta.rime.configPath}
                  </code>
                </InfoRow>
                {osMeta.rime.nixosNote && osMeta.rime.nixosUrl && (
                  <InfoRow label="NixOS">
                    <a
                      href={osMeta.rime.nixosUrl}
                      target="_blank"
                      rel="noreferrer"
                      className="text-primary hover:underline inline-flex items-start gap-1"
                    >
                      <span className="break-all">{osMeta.rime.nixosNote}</span>
                      <ExternalLink className="h-3 w-3 shrink-0 mt-0.5" />
                    </a>
                  </InfoRow>
                )}
                {osMeta.rime.appPackage && (
                  <InfoRow label="快捷入口">
                    <Button
                      variant="outline"
                      size="sm"
                      className="gap-1.5 h-7 text-xs"
                      onClick={() =>
                        invoke("android_open_app", { packageName: osMeta.rime.appPackage })
                          .catch((e) => setInstallError(String(e)))
                      }
                    >
                      <ExternalLink className="h-3.5 w-3.5" />
                      打开{osMeta.rime.name}
                    </Button>
                  </InfoRow>
                )}
              </div>
            </CardContent>
          </Card>
        )}

        {/* Step 2: 安装键道方案 */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-sm font-semibold flex items-center gap-2">
              <span className="flex items-center justify-center w-5 h-5 rounded-full bg-primary text-primary-foreground text-xs font-bold">2</span>
              安装键道方案
              <div className="ml-auto flex items-center gap-1.5">
                {releaseInfo && (
                  <Badge variant="secondary" className="font-mono text-xs">
                    {releaseInfo.version}
                  </Badge>
                )}
                {releaseInfo?.body && (
                  <button
                    onClick={() => setShowChangelog(true)}
                    className="text-xs text-muted-foreground hover:text-foreground transition-colors underline underline-offset-2"
                    title="查看更新内容"
                  >
                    更新内容
                  </button>
                )}
                {isFetchingRelease ? (
                  <RefreshCw className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
                ) : (
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-6 w-6"
                    onClick={handleRefetchRelease}
                    title="检查新版本"
                  >
                    <RefreshCw className="h-3.5 w-3.5" />
                  </Button>
                )}
              </div>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">

            {/* Warnings */}
            {releaseError && (
              <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                <span>获取版本信息失败：{releaseError}</span>
              </div>
            )}

            {!downloadUrl && releaseInfo && !isFetchingRelease && (
              <div className="flex items-start gap-2 text-sm text-yellow-400 bg-yellow-400/10 border border-yellow-400/20 rounded-lg px-3 py-2.5">
                <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                <span>
                  当前系统（{osMeta?.label}）暂无对应安装包，请前往{" "}
                  <a
                    href="https://github.com/xkinput/KeyTao/releases"
                    target="_blank"
                    rel="noreferrer"
                    className="underline"
                  >
                    GitHub Releases
                  </a>{" "}
                  手动下载
                </span>
              </div>
            )}

            <div className="flex items-start gap-2 text-sm text-yellow-400/80 bg-yellow-400/8 border border-yellow-400/15 rounded-lg px-3 py-2.5">
              <Info className="h-4 w-4 shrink-0 mt-0.5" />
              <span>
                智能安装：仅覆盖 <code className="text-xs bg-yellow-400/10 px-1 rounded">opencc/</code>、
                <code className="text-xs bg-yellow-400/10 px-1 rounded">lua/</code>、键道词库文件，
                自动合并 <code className="text-xs bg-yellow-400/10 px-1 rounded">default.custom.yaml</code> 和{" "}
                <code className="text-xs bg-yellow-400/10 px-1 rounded">rime.lua</code>（同名 lua 文件自动重命名保留），其余文件不受影响
              </span>
            </div>

            {/* Directory Selection */}
            <div className="space-y-3">
              {osType === "linux" && (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-muted-foreground shrink-0">输入法框架</span>
                  <div className="flex gap-1">
                    {(["fcitx5", "ibus"] as const).map((im) => (
                      <button
                        key={im}
                        onClick={() => setLinuxImType(im)}
                        disabled={isInstalling}
                        className={`px-3 py-1 text-xs rounded-md border transition-colors ${linuxImType === im
                          ? "bg-primary text-primary-foreground border-primary"
                          : "bg-transparent text-muted-foreground border-border hover:border-foreground/40 hover:text-foreground"
                          }`}
                      >
                        {im === "fcitx5" ? "Fcitx5" : "iBus"}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              <div className="flex gap-2 flex-wrap">
                <Button
                  variant="default"
                  size="sm"
                  onClick={handleSelectDir}
                  disabled={isInstalling || (osType === "linux" && linuxImType === null)}
                  className="gap-1.5"
                >
                  <FolderOpen className="h-4 w-4" />
                  {selectedDir ? "重新选择目录" : "选择 Rime 配置目录"}
                </Button>

                {selectedDir && downloadUrl && (
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={handleInstall}
                    disabled={isInstalling}
                    className="gap-1.5"
                  >
                    <Download className="h-4 w-4" />
                    {isInstalling ? "安装中..." : "立即安装"}
                  </Button>
                )}
              </div>


              {/* Selected directory + file list */}
              {selectedDir && (
                <div className="space-y-2">
                  <div className="flex items-center gap-2 bg-muted/40 border border-border rounded-lg px-3 py-2">
                    <CheckCircle2 className="h-4 w-4 text-green-500 shrink-0" />
                    <code className="text-xs font-mono text-muted-foreground break-all flex-1 min-w-0">
                      {selectedDir}
                    </code>
                  </div>

                  {localSchemas !== null && (
                    <div className="flex items-start gap-2 text-xs bg-muted/40 border border-border rounded-lg px-3 py-2">
                      <Info className="h-3.5 w-3.5 shrink-0 mt-0.5 text-muted-foreground" />
                      {localSchemas.length === 0 ? (
                        <span className="text-muted-foreground">未检测到 default.custom.yaml，将自动创建</span>
                      ) : (
                        <span className="text-muted-foreground">
                          检测到本地方案：
                          {localSchemas.map((s, i) => (
                            <span key={s}>
                              <code className="font-mono bg-muted px-1 rounded">{s}</code>
                              {i < localSchemas.length - 1 && "、"}
                            </span>
                          ))}
                          {localSchemas.some(s => !s.startsWith("keytao")) && (
                            <span className="text-foreground/70">（非键道方案将被保留）</span>
                          )}
                        </span>
                      )}
                    </div>
                  )}

                  <FileList
                    files={files}
                    loading={isLoadingFiles}
                    onRefresh={handleRefreshFiles}
                    disabled={isInstalling}
                  />
                </div>
              )}
            </div>

            {/* Progress */}
            {isInstalling && progress && (
              <div className="space-y-1.5">
                <Progress value={progress.percent} className="h-1.5" />
                <p className="text-xs text-muted-foreground">{progress.message}</p>
              </div>
            )}

            {installError && (
              <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                <span>{installError}</span>
              </div>
            )}
          </CardContent>
        </Card>

        <Dialog open={showChangelog} onOpenChange={setShowChangelog}>
          <DialogContent className="max-w-sm">
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <FileText className="h-4 w-4" />
                {releaseInfo?.name || releaseInfo?.version} 更新内容
              </DialogTitle>
              <DialogDescription asChild>
                <div className="mt-2 max-h-96 overflow-y-auto">
                  <pre className="text-xs text-foreground/80 whitespace-pre-wrap font-sans leading-relaxed">
                    {releaseInfo?.body}
                  </pre>
                </div>
              </DialogDescription>
            </DialogHeader>
            <Button onClick={() => setShowChangelog(false)} className="w-full mt-2">
              关闭
            </Button>
          </DialogContent>
        </Dialog>

        <Dialog open={!!installResult} onOpenChange={(open) => { if (!open) setInstallResult(null) }}>
          <DialogContent className="max-w-sm">
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2 text-green-400">
                <CheckCircle2 className="h-5 w-5 shrink-0" />
                安装完成
              </DialogTitle>
              <DialogDescription asChild>
                <div className="space-y-2 pt-1">
                  <p className="text-sm text-foreground">
                    请在输入法中点击<strong>重新部署</strong>以生效
                  </p>
                  {installResult && installResult.merged_schemas.length > 0 && (
                    <p className="text-xs text-muted-foreground">
                      已智能合并本地方案：
                      {installResult.merged_schemas.map((s, i) => (
                        <span key={s}>
                          <code className="font-mono bg-muted px-1 rounded">{s}</code>
                          {i < installResult.merged_schemas.length - 1 && "、"}
                        </span>
                      ))}
                    </p>
                  )}
                  {installResult && installResult.verify.length > 0 && (() => {
                    const failCount = installResult.verify.filter(v => !v.ok).length
                    return (
                      <details className="text-xs" open={failCount > 0}>
                        <summary className="cursor-pointer select-none py-1 flex items-center gap-1.5">
                          {failCount > 0
                            ? <span className="text-destructive">校验失败 {failCount} 项，可能未正确安装</span>
                            : <span className="text-green-400">校验通过（{installResult.verify.length} 项）</span>
                          }
                        </summary>
                        <div className="mt-1 max-h-48 overflow-y-auto rounded-md bg-muted/60 border border-border p-2 space-y-0.5">
                          {installResult.verify.map((entry, i) => (
                            <div key={i} className="flex items-start gap-1.5 font-mono text-[11px] leading-5">
                              <span className={entry.ok ? "text-green-400 shrink-0" : "text-destructive shrink-0"}>
                                {entry.ok ? "✓" : "✗"}
                              </span>
                              <span className={`break-all ${entry.ok ? "text-muted-foreground" : "text-destructive"}`}>
                                {entry.path}
                                {!entry.ok && <span className="text-destructive/70"> — {entry.note}</span>}
                              </span>
                            </div>
                          ))}
                        </div>
                      </details>
                    )
                  })()}
                  {installResult && installResult.logs.length > 0 && (
                    <details className="text-xs">
                      <summary className="cursor-pointer text-muted-foreground hover:text-foreground select-none py-1">
                        安装日志（{installResult.logs.length} 条）
                      </summary>
                      <div className="mt-1 max-h-48 overflow-y-auto rounded-md bg-muted/60 border border-border p-2 space-y-0.5">
                        {installResult.logs.map((line, i) => (
                          <div
                            key={i}
                            className={`font-mono text-[11px] leading-5 ${line.startsWith("[ERROR]")
                              ? "text-destructive"
                              : line.startsWith("[WARN]")
                                ? "text-yellow-400"
                                : line.includes("[root]")
                                  ? "text-orange-400"
                                  : line.includes("[forced]")
                                    ? "text-yellow-300"
                                    : line.startsWith("[MERGED]") || line.startsWith("[RENAMED]")
                                      ? "text-primary"
                                      : "text-muted-foreground"
                              }`}
                          >
                            {line}
                          </div>
                        ))}
                      </div>
                    </details>
                  )}
                </div>
              </DialogDescription>
            </DialogHeader>
            <Button onClick={() => setInstallResult(null)} className="w-full mt-2">
              好的
            </Button>
          </DialogContent>
        </Dialog>

      </div>
    </div>
  )
}
