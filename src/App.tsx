import { useState, useEffect, useRef } from "react"
import { invoke } from "@tauri-apps/api/core"
import { getVersion } from "@tauri-apps/api/app"
import { listen } from "@tauri-apps/api/event"
import { platform } from "@tauri-apps/plugin-os"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { Badge } from "@/components/ui/badge"
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
  ExternalLink,
  RefreshCw,
  FileText,
  Folder,
  Info,
  Keyboard,
  ChevronDown,
  ChevronUp,
  Settings,
  Cpu,
} from "lucide-react"
import { ImePanel } from "@/components/ImePanel"

type OSType = "windows" | "macos" | "linux" | "android" | "ios" | "unknown"

interface InstallerUpdateInfo {
  current_version: string
  latest_version: string
  has_update: boolean
  release_url: string
}

type DownloadSource = "github" | "gitee"

interface PlatformRelease {
  version: string
  download_urls: {
    macos?: string
    windows?: string
    linux?: string
    android?: string
  }
}

interface ReleaseInfo {
  version: string
  name: string
  published_at: string
  body: string
  github: PlatformRelease | null
  gitee: PlatformRelease | null
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
  // eslint-disable-next-line react-hooks/rules-of-hooks
  const [osType, setOsType] = useState<OSType>("unknown")
  const [appVersion, setAppVersion] = useState<string>("")
  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo | null>(null)
  const [releaseError, setReleaseError] = useState<string | null>(null)
  const [isFetchingRelease, setIsFetchingRelease] = useState(true)
  const [downloadSource, setDownloadSource] = useState<DownloadSource>("gitee")
  const [installerUpdate, setInstallerUpdate] = useState<InstallerUpdateInfo | null>(null)

  // macOS IME
  const [imeInstalled, setImeInstalled] = useState(false)
  const [isInstallingIme, setIsInstallingIme] = useState(false)
  const [imeInstallError, setImeInstallError] = useState<string | null>(null)

  // macOS first-run schema download (to ~/Library/keytao)
  const [isDownloadingDefault, setIsDownloadingDefault] = useState(false)
  const [defaultInstallResult, setDefaultInstallResult] = useState<InstallResult | null>(null)
  const [defaultInstallError, setDefaultInstallError] = useState<string | null>(null)
  const [defaultInstallProgress, setDefaultInstallProgress] = useState<InstallProgress | null>(null)

  // Extension features (collapsed by default)
  const [showExtensions, setShowExtensions] = useState(false)

  // Default keytao data dir for this platform
  const [defaultDir, setDefaultDir] = useState<string | null>(null)

  // Directory selection (extension: install to custom dir)
  const [selectedDir, setSelectedDir] = useState<string | null>(null)
  const [safUri, setSafUri] = useState<string | null>(null)

  // File preview
  const [files, setFiles] = useState<FileItem[]>([])
  const [isLoadingFiles, setIsLoadingFiles] = useState(false)
  const [localSchemas, setLocalSchemas] = useState<string[] | null>(null)

  // Install (extension)
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

    // Fetch default data dir
    invoke<string | null>("rime_get_data_dir")
      .then((d) => setDefaultDir(d ?? null))
      .catch(() => { })

    // Check macOS IME install status
    if (map[p] === "macos") {
      invoke<{ installed: boolean }>("macos_ime_status")
        .then((s) => setImeInstalled(s.installed))
        .catch(() => { })
    }

    listen<InstallProgress>("install-progress", (e) => {
      setProgress(e.payload)
      setDefaultInstallProgress(e.payload)
    }).then((fn) => { unlistenRef.current = fn })

    return () => unlistenRef.current?.()
  }, [])

  const activePlatform = downloadSource === "gitee" ? releaseInfo?.gitee : releaseInfo?.github
  const downloadUrl = activePlatform?.download_urls?.[osType as keyof PlatformRelease["download_urls"]]

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
        const dir = await invoke<string | null>("select_directory", { imType: null })
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

  async function handleInstallIme() {
    setIsInstallingIme(true)
    setImeInstallError(null)
    try {
      await invoke("macos_install_ime")
      setImeInstalled(true)
    } catch (e) {
      setImeInstallError(String(e))
    } finally {
      setIsInstallingIme(false)
    }
  }

  async function handleUninstallIme() {
    try {
      await invoke("macos_uninstall_ime")
      setImeInstalled(false)
    } catch (e) {
      setImeInstallError(String(e))
    }
  }

  async function handleDownloadDefault() {
    if (!downloadUrl) return
    setIsDownloadingDefault(true)
    setDefaultInstallResult(null)
    setDefaultInstallError(null)
    setDefaultInstallProgress(null)
    try {
      const result = await invoke<InstallResult>("rime_install_to_default", { url: downloadUrl })
      setDefaultInstallResult(result)
    } catch (e) {
      setDefaultInstallError(String(e))
    } finally {
      setIsDownloadingDefault(false)
      setDefaultInstallProgress(null)
    }
  }

  // ── Release source picker (shared widget) ────────────────────────────────
  const VersionPicker = (
    <div className="flex items-center gap-1.5">
      {releaseInfo?.github && (
        <div className="flex gap-1">
          {(["github", "gitee"] as const).map((src) => {
            const p = src === "github" ? releaseInfo.github : releaseInfo.gitee
            if (!p) return null
            return (
              <button
                key={src}
                onClick={() => setDownloadSource(src)}
                className={`px-2 py-0.5 text-xs rounded border transition-colors font-mono ${downloadSource === src
                  ? "bg-primary text-primary-foreground border-primary"
                  : "bg-transparent text-muted-foreground border-border hover:border-foreground/40"
                  }`}
              >
                {src === "github" ? "GitHub" : "Gitee"} {p.version}
              </button>
            )
          })}
        </div>
      )}
      {releaseInfo && !releaseInfo.github && (
        <Badge variant="secondary" className="font-mono text-xs">{releaseInfo.version}</Badge>
      )}
      {releaseInfo?.body && (
        <button
          onClick={() => setShowChangelog(true)}
          className="text-xs text-muted-foreground hover:text-foreground transition-colors underline underline-offset-2"
        >
          更新内容
        </button>
      )}
      {isFetchingRelease
        ? <RefreshCw className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
        : <Button variant="ghost" size="icon" className="h-6 w-6" onClick={handleRefetchRelease} title="检查新版本">
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
      }
    </div>
  )

  // ── Install result dialog (shared) ─────────────────────────────────────────
  const resultToShow = installResult ?? defaultInstallResult
  const clearResult = () => { setInstallResult(null); setDefaultInstallResult(null) }

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="max-w-2xl mx-auto px-4 py-8 space-y-4">

        {/* Header */}
        <div className="text-center space-y-2 pb-2">
          <img src="/logo.png" alt="键道输入法" className="h-16 w-16 mx-auto" />
          <h1 className="text-2xl font-bold tracking-tight">
            键道输入法{appVersion && <span className="ml-2 text-base font-normal text-muted-foreground">v{appVersion}</span>}
          </h1>
          <p className="text-sm text-muted-foreground">
            基于 librime 的跨平台原生输入法
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
              <Badge variant="secondary" className="font-mono text-xs">v{installerUpdate.latest_version}</Badge>
            </div>
            <div className="flex items-center gap-2 text-muted-foreground text-xs shrink-0">
              <span>当前 v{installerUpdate.current_version}</span>
              <ExternalLink className="h-3 w-3" />
            </div>
          </a>
        )}

        {/* ══ Step 1: 安装输入法（macOS 专属）══════════════════════════════ */}
        {osType === "macos" && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <span className="flex items-center justify-center w-5 h-5 rounded-full bg-primary text-primary-foreground text-xs font-bold">1</span>
                安装 KeyTao 系统输入法
                <span className="ml-auto">
                  {imeInstalled
                    ? <Badge className="text-xs gap-1 bg-green-500/20 text-green-400 border-green-500/30"><CheckCircle2 className="h-3 w-3" />已安装</Badge>
                    : <Badge variant="outline" className="text-xs">未安装</Badge>
                  }
                </span>
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <p className="text-sm text-muted-foreground">
                将 KeyTao.app 安装到 <code className="text-xs bg-muted px-1.5 py-0.5 rounded font-mono">~/Library/Input Methods/</code>，成为 macOS 原生系统输入法。
              </p>
              {imeInstallError && (
                <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                  <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                  <span>{imeInstallError}</span>
                </div>
              )}
              <div className="flex gap-2 flex-wrap">
                <Button size="sm" onClick={handleInstallIme} disabled={isInstallingIme} className="gap-1.5">
                  <Cpu className="h-4 w-4" />
                  {isInstallingIme ? "安装中..." : imeInstalled ? "重新安装" : "安装输入法"}
                </Button>
                {imeInstalled && (
                  <Button variant="outline" size="sm" onClick={handleUninstallIme}
                    className="gap-1.5 text-destructive hover:text-destructive">
                    卸载
                  </Button>
                )}
              </div>
              {imeInstalled && (
                <p className="text-xs text-muted-foreground">
                  如未出现「键道」，请前往<strong>系统设置 → 键盘 → 输入来源</strong>手动添加。
                </p>
              )}
            </CardContent>
          </Card>
        )}

        {/* ══ Step 2: 安装键道方案到默认目录（所有平台）════════════════════ */}
        {osType !== "android" && osType !== "ios" && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <span className="flex items-center justify-center w-5 h-5 rounded-full bg-primary text-primary-foreground text-xs font-bold">
                  {osType === "macos" ? "2" : "1"}
                </span>
                安装键道方案
                <div className="ml-auto">{VersionPicker}</div>
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              {defaultDir && (
                <div className="flex items-center gap-2 text-xs text-muted-foreground bg-muted/40 border border-border rounded-lg px-3 py-2">
                  <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span>安装目录：<code className="font-mono">{defaultDir}</code></span>
                </div>
              )}
              {releaseError && (
                <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                  <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                  <span>获取版本信息失败：{releaseError}</span>
                </div>
              )}
              {defaultInstallError && (
                <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                  <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                  <span>{defaultInstallError}</span>
                </div>
              )}
              {isDownloadingDefault && defaultInstallProgress && (
                <div className="space-y-1.5">
                  <Progress value={defaultInstallProgress.percent} className="h-1.5" />
                  <p className="text-xs text-muted-foreground">{defaultInstallProgress.message}</p>
                </div>
              )}
              <Button size="sm" onClick={handleDownloadDefault}
                disabled={isDownloadingDefault || !downloadUrl} className="gap-1.5">
                <Download className="h-4 w-4" />
                {isDownloadingDefault ? "下载中..." : "一键安装方案"}
              </Button>
            </CardContent>
          </Card>
        )}

        {/* ══ Step 3: 测试 & 状态（桌面平台）══════════════════════════════ */}
        {osType !== "android" && osType !== "ios" && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <span className="flex items-center justify-center w-5 h-5 rounded-full bg-primary text-primary-foreground text-xs font-bold">
                  {osType === "macos" ? "3" : "2"}
                </span>
                测试 &amp; 状态
                <span className="ml-auto">
                  <Badge variant="secondary" className="text-xs gap-1">
                    <Keyboard className="h-3 w-3" />librime
                  </Badge>
                </span>
              </CardTitle>
            </CardHeader>
            <CardContent>
              <ImePanel userDataDir={defaultDir ?? undefined} />
            </CardContent>
          </Card>
        )}

        {/* ══ 扩展功能（所有平台，可折叠）══════════════════════════════════ */}
        <Card>
          <CardHeader className="pb-0">
            <button
              onClick={() => setShowExtensions(v => !v)}
              className="w-full flex items-center gap-2 text-sm font-semibold py-0.5 text-left"
            >
              <Settings className="h-4 w-4 text-muted-foreground" />
              <span>扩展功能</span>
              <span className="ml-auto text-muted-foreground">
                {showExtensions ? <ChevronUp className="h-4 w-4" /> : <ChevronDown className="h-4 w-4" />}
              </span>
            </button>
          </CardHeader>
          {showExtensions && (
            <CardContent className="space-y-4 pt-4">
              <div className="flex items-center gap-2">
                <FolderOpen className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">安装方案到自定义目录</span>
              </div>
              <div className="flex gap-2 flex-wrap">
                <Button variant="outline" size="sm" onClick={handleSelectDir} disabled={isInstalling} className="gap-1.5">
                  <FolderOpen className="h-4 w-4" />
                  {selectedDir ? "重新选择目录" : "选择目录"}
                </Button>
                {selectedDir && downloadUrl && (
                  <Button variant="secondary" size="sm" onClick={handleInstall} disabled={isInstalling} className="gap-1.5">
                    <Download className="h-4 w-4" />
                    {isInstalling ? "安装中..." : "立即安装"}
                  </Button>
                )}
              </div>
              {selectedDir && (
                <div className="space-y-2">
                  <div className="flex items-center gap-2 bg-muted/40 border border-border rounded-lg px-3 py-2">
                    <CheckCircle2 className="h-4 w-4 text-green-500 shrink-0" />
                    <code className="text-xs font-mono text-muted-foreground break-all flex-1 min-w-0">{selectedDir}</code>
                  </div>
                  {localSchemas !== null && (
                    <div className="flex items-start gap-2 text-xs bg-muted/40 border border-border rounded-lg px-3 py-2">
                      <Info className="h-3.5 w-3.5 shrink-0 mt-0.5 text-muted-foreground" />
                      {localSchemas.length === 0
                        ? <span className="text-muted-foreground">未检测到 default.custom.yaml，将自动创建</span>
                        : <span className="text-muted-foreground">
                          检测到本地方案：
                          {localSchemas.map((s, i) => (
                            <span key={s}>
                              <code className="font-mono bg-muted px-1 rounded">{s}</code>
                              {i < localSchemas.length - 1 && "、"}
                            </span>
                          ))}
                        </span>
                      }
                    </div>
                  )}
                  <FileList files={files} loading={isLoadingFiles} onRefresh={handleRefreshFiles} disabled={isInstalling} />
                </div>
              )}
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
          )}
        </Card>

        {/* ── 更新内容弹窗 ──────────────────────────────────────────────── */}
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
            <Button onClick={() => setShowChangelog(false)} className="w-full mt-2">关闭</Button>
          </DialogContent>
        </Dialog>

        {/* ── 安装结果弹窗 ──────────────────────────────────────────────── */}
        <Dialog open={!!resultToShow} onOpenChange={(open) => { if (!open) clearResult() }}>
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
                  {resultToShow && resultToShow.merged_schemas.length > 0 && (
                    <p className="text-xs text-muted-foreground">
                      已智能合并本地方案：
                      {resultToShow.merged_schemas.map((s, i) => (
                        <span key={s}>
                          <code className="font-mono bg-muted px-1 rounded">{s}</code>
                          {i < resultToShow.merged_schemas.length - 1 && "、"}
                        </span>
                      ))}
                    </p>
                  )}
                  {resultToShow && resultToShow.verify.length > 0 && (() => {
                    const failCount = resultToShow.verify.filter(v => !v.ok).length
                    return (
                      <details className="text-xs" open={failCount > 0}>
                        <summary className="cursor-pointer select-none py-1 flex items-center gap-1.5">
                          {failCount > 0
                            ? <span className="text-destructive">校验失败 {failCount} 项，可能未正确安装</span>
                            : <span className="text-green-400">校验通过（{resultToShow.verify.length} 项）</span>
                          }
                        </summary>
                        <div className="mt-1 max-h-48 overflow-y-auto rounded-md bg-muted/60 border border-border p-2 space-y-0.5">
                          {resultToShow.verify.map((entry, i) => (
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
                  {resultToShow && resultToShow.logs.length > 0 && (
                    <details className="text-xs">
                      <summary className="cursor-pointer text-muted-foreground hover:text-foreground select-none py-1">
                        安装日志（{resultToShow.logs.length} 条）
                      </summary>
                      <div className="mt-1 max-h-48 overflow-y-auto rounded-md bg-muted/60 border border-border p-2 space-y-0.5">
                        {resultToShow.logs.map((line, i) => (
                          <div key={i} className={`font-mono text-[11px] leading-5 ${line.startsWith("[ERROR]") ? "text-destructive"
                            : line.startsWith("[WARN]") ? "text-yellow-400"
                              : line.includes("[root]") ? "text-orange-400"
                                : line.includes("[forced]") ? "text-yellow-300"
                                  : line.startsWith("[MERGED]") || line.startsWith("[RENAMED]") ? "text-primary"
                                    : "text-muted-foreground"
                            }`}>{line}</div>
                        ))}
                      </div>
                    </details>
                  )}
                </div>
              </DialogDescription>
            </DialogHeader>
            <Button onClick={clearResult} className="w-full mt-2">好的</Button>
          </DialogContent>
        </Dialog>

      </div>
    </div>
  )
}
