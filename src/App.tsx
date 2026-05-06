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
  Settings,
  Cpu,
  ScrollText,
  Play,
  Loader2,
  XCircle,
} from "lucide-react"

type OSType = "windows" | "macos" | "linux" | "android" | "ios" | "unknown"
type Tab = "install" | "extension"

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

interface LocalSchemaInfo {
  installed: boolean
  version: string | null
  schemas: string[]
}

interface DeployResult {
  success: boolean
  message: string
}

interface DeployStep {
  msg: string
  done?: boolean
  error?: boolean
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
  const [activeTab, setActiveTab] = useState<Tab>("install")

  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo | null>(null)
  const [releaseError, setReleaseError] = useState<string | null>(null)
  const [isFetchingRelease, setIsFetchingRelease] = useState(true)
  const [downloadSource, setDownloadSource] = useState<DownloadSource>("gitee")
  const [installerUpdate, setInstallerUpdate] = useState<InstallerUpdateInfo | null>(null)

  // macOS IME
  const [imeInstalled, setImeInstalled] = useState(false)
  const [isInstallingIme, setIsInstallingIme] = useState(false)
  const [imeInstallError, setImeInstallError] = useState<string | null>(null)

  // Default data dir
  const [defaultDir, setDefaultDir] = useState<string | null>(null)

  // Local schema info
  const [localSchemaInfo, setLocalSchemaInfo] = useState<LocalSchemaInfo | null>(null)
  const [isCheckingLocal, setIsCheckingLocal] = useState(false)

  // Install (default dir)
  const [isInstalling, setIsInstalling] = useState(false)
  const [installProgress, setInstallProgress] = useState<InstallProgress | null>(null)
  const [installError, setInstallError] = useState<string | null>(null)

  // Deploy
  const [isDeploying, setIsDeploying] = useState(false)
  const [deploySteps, setDeploySteps] = useState<DeployStep[]>([])

  // Log buffer
  const [logBuffer, setLogBuffer] = useState<string[]>([])
  const [showLogs, setShowLogs] = useState(false)

  // Changelog
  const [showChangelog, setShowChangelog] = useState(false)

  // Extension tab
  const [selectedDir, setSelectedDir] = useState<string | null>(null)
  const [safUri, setSafUri] = useState<string | null>(null)
  const [files, setFiles] = useState<FileItem[]>([])
  const [isLoadingFiles, setIsLoadingFiles] = useState(false)
  const [localSchemas, setLocalSchemas] = useState<string[] | null>(null)
  const [isInstallingExt, setIsInstallingExt] = useState(false)
  const [extProgress, setExtProgress] = useState<InstallProgress | null>(null)
  const [extError, setExtError] = useState<string | null>(null)
  const [extResult, setExtResult] = useState<InstallResult | null>(null)

  const unlistenInstallRef = useRef<(() => void) | null>(null)
  const unlistenDeployRef = useRef<(() => void) | null>(null)

  function addLogs(lines: string[]) {
    const ts = new Date().toLocaleTimeString()
    setLogBuffer((prev) => [...prev, ...lines.map((l) => `[${ts}] ${l}`)])
  }

  useEffect(() => {
    const p = platform()
    const map: Record<string, OSType> = {
      macos: "macos", windows: "windows", linux: "linux",
      android: "android", ios: "ios",
    }
    const os = map[p] ?? "unknown"
    setOsType(os)
    getVersion().then(setAppVersion).catch(() => { })

    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))

    invoke<InstallerUpdateInfo>("check_installer_update")
      .then((info) => { if (info.has_update) setInstallerUpdate(info) })
      .catch(() => { })

    invoke<string | null>("rime_get_data_dir")
      .then((d) => setDefaultDir(d ?? null))
      .catch(() => { })

    invoke<LocalSchemaInfo>("check_local_schema")
      .then(setLocalSchemaInfo)
      .catch(() => { })

    if (os === "macos") {
      invoke<{ installed: boolean }>("macos_ime_status")
        .then((s) => setImeInstalled(s.installed))
        .catch(() => { })
    }

    listen<InstallProgress>("install-progress", (e) => {
      setInstallProgress(e.payload)
      setExtProgress(e.payload)
    }).then((fn) => { unlistenInstallRef.current = fn })

    return () => {
      unlistenInstallRef.current?.()
      unlistenDeployRef.current?.()
    }
  }, [])

  const activePlatform = downloadSource === "gitee" ? releaseInfo?.gitee : releaseInfo?.github
  const downloadUrl = activePlatform?.download_urls?.[osType as keyof PlatformRelease["download_urls"]]
  const isBusy = isInstalling || isDeploying

  async function handleCheckLocalSchema() {
    setIsCheckingLocal(true)
    try {
      const info = await invoke<LocalSchemaInfo>("check_local_schema")
      setLocalSchemaInfo(info)
    } catch { }
    finally { setIsCheckingLocal(false) }
  }

  async function handleDeploy() {
    setIsDeploying(true)
    const steps: DeployStep[] = [{ msg: "正在部署 librime..." }]
    setDeploySteps([...steps])

    unlistenDeployRef.current?.()
    const unlisten = await listen<string>("deploy-progress", (e) => {
      steps.push({ msg: e.payload })
      setDeploySteps([...steps])
    })
    unlistenDeployRef.current = unlisten

    try {
      const result = await invoke<DeployResult>("rime_deploy_default")
      steps.push({ msg: result.message, done: true })
      setDeploySteps([...steps])
      addLogs([`[DEPLOY] ${result.message}`])
    } catch (e) {
      const msg = String(e)
      steps.push({ msg, error: true })
      setDeploySteps([...steps])
      addLogs([`[DEPLOY ERROR] ${msg}`])
    } finally {
      setIsDeploying(false)
    }
  }

  async function handleInstall() {
    if (!downloadUrl) return
    setIsInstalling(true)
    setInstallProgress(null)
    setInstallError(null)
    setDeploySteps([])

    try {
      const result = await invoke<InstallResult>("rime_install_to_default", { url: downloadUrl })
      addLogs(result.logs)
      if (result.verify.some((v) => !v.ok)) {
        addLogs(result.verify.filter((v) => !v.ok).map((v) => `[VERIFY FAIL] ${v.path}: ${v.note}`))
      }
      await handleCheckLocalSchema()
    } catch (e) {
      setInstallError(String(e))
      setIsInstalling(false)
      return
    }
    setIsInstalling(false)
    await handleDeploy()
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

  async function handleRefetchRelease() {
    setIsFetchingRelease(true)
    setReleaseError(null)
    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))
  }

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
        setSafUri(uri)
        setSelectedDir(safUriToDisplayPath(uri))
        setExtResult(null)
        setExtError(null)
        await loadFiles(undefined, uri)
      } catch (e) {
        setExtError(String(e))
      }
    } else {
      try {
        const dir = await invoke<string | null>("select_directory", { imType: null })
        if (dir) {
          setSelectedDir(dir)
          setSafUri(null)
          setExtResult(null)
          setExtError(null)
          await loadFiles(dir)
        }
      } catch (e) {
        setExtError(String(e))
      }
    }
  }

  async function handleInstallExt() {
    if (!selectedDir || !downloadUrl) return
    setIsInstallingExt(true)
    setExtResult(null)
    setExtError(null)
    setExtProgress(null)
    try {
      const tempPath = await invoke<string>("download_to_temp", { url: downloadUrl })
      let result: InstallResult
      if (osType === "android" && safUri) {
        result = await invoke<InstallResult>("android_smart_extract", { zipPath: tempPath, treeUri: safUri })
      } else {
        result = await invoke<InstallResult>("smart_install", { zipPath: tempPath, destPath: selectedDir })
      }
      setExtResult(result)
      addLogs(result.logs)
      await loadFiles(selectedDir ?? undefined, safUri ?? undefined)
    } catch (e) {
      setExtError(String(e))
    } finally {
      setIsInstallingExt(false)
      setExtProgress(null)
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

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="max-w-2xl mx-auto px-4 py-6 space-y-4">

        {/* Header */}
        <div className="flex items-center gap-3 pb-1">
          <img src="/logo.png" alt="键道输入法" className="h-12 w-12" />
          <div>
            <h1 className="text-xl font-bold tracking-tight leading-tight">
              键道输入法
              {appVersion && <span className="ml-2 text-sm font-normal text-muted-foreground">v{appVersion}</span>}
            </h1>
            <p className="text-xs text-muted-foreground">基于 librime 的跨平台原生输入法</p>
          </div>
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

        {/* Tab nav */}
        <div className="flex border-b border-border">
          {([
            { id: "install", label: "安装", icon: Download },
            { id: "extension", label: "扩展", icon: Settings },
          ] as const).map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setActiveTab(id)}
              className={`flex items-center gap-1.5 px-4 py-2.5 text-sm font-medium border-b-2 transition-colors -mb-px ${activeTab === id
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground hover:border-border"
                }`}
            >
              <Icon className="h-3.5 w-3.5" />
              {label}
            </button>
          ))}
        </div>

        {/* ══ 安装 Tab ══════════════════════════════════════════════════════ */}
        {activeTab === "install" && (
          <div className="space-y-4">
            {osType === "macos" && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Cpu className="h-4 w-4 text-muted-foreground" />
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
                        className="gap-1.5 text-destructive hover:text-destructive">卸载</Button>
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

            {osType !== "android" && osType !== "ios" && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Download className="h-4 w-4 text-muted-foreground" />
                    键道方案
                    <div className="ml-auto">{VersionPicker}</div>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {defaultDir && (
                    <div className="flex items-center gap-2 text-xs text-muted-foreground bg-muted/40 border border-border rounded-lg px-3 py-2">
                      <Info className="h-3.5 w-3.5 shrink-0" />
                      <span>目录：<code className="font-mono">{defaultDir}</code></span>
                    </div>
                  )}
                  {localSchemaInfo !== null && (
                    <div className={`flex items-center gap-2 text-xs rounded-lg px-3 py-2 border ${localSchemaInfo.installed
                      ? "bg-green-500/10 border-green-500/30 text-green-400"
                      : "bg-muted/40 border-border text-muted-foreground"
                      }`}>
                      {localSchemaInfo.installed
                        ? <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
                        : <Info className="h-3.5 w-3.5 shrink-0" />
                      }
                      <span>
                        {localSchemaInfo.installed
                          ? `已安装${localSchemaInfo.version ? ` ${localSchemaInfo.version}` : ""}`
                          : "未检测到已安装的键道方案"
                        }
                        {localSchemaInfo.installed && localSchemaInfo.schemas.length > 0 && (
                          <span className="ml-1 text-muted-foreground/80">({localSchemaInfo.schemas.join(", ")})</span>
                        )}
                      </span>
                    </div>
                  )}
                  {releaseError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>获取版本信息失败：{releaseError}</span>
                    </div>
                  )}
                  {installError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{installError}</span>
                    </div>
                  )}
                  {isInstalling && installProgress && (
                    <div className="space-y-1.5">
                      <Progress value={installProgress.percent} className="h-1.5" />
                      <p className="text-xs text-muted-foreground">{installProgress.message}</p>
                    </div>
                  )}
                  {deploySteps.length > 0 && (
                    <div className="rounded-lg border border-border bg-muted/20 px-3 py-2 space-y-1">
                      {deploySteps.map((step, i) => (
                        <div key={i} className="flex items-center gap-2 text-xs">
                          {step.done
                            ? <CheckCircle2 className="h-3 w-3 text-green-400 shrink-0" />
                            : step.error
                              ? <XCircle className="h-3 w-3 text-destructive shrink-0" />
                              : <Loader2 className={`h-3 w-3 shrink-0 text-muted-foreground ${isDeploying && i === deploySteps.length - 1 ? "animate-spin" : ""}`} />
                          }
                          <span className={step.error ? "text-destructive" : step.done ? "text-green-400" : "text-muted-foreground"}>
                            {step.msg}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="flex gap-2 flex-wrap">
                    <Button size="sm" onClick={handleInstall} disabled={isBusy || !downloadUrl} className="gap-1.5">
                      <Download className="h-4 w-4" />
                      {isInstalling ? "安装中..." : isDeploying ? "部署中..." : localSchemaInfo?.installed ? "更新方案" : "安装方案"}
                    </Button>
                    <Button variant="outline" size="sm" onClick={handleCheckLocalSchema}
                      disabled={isCheckingLocal || isBusy} className="gap-1.5">
                      <RefreshCw className={`h-3.5 w-3.5 ${isCheckingLocal ? "animate-spin" : ""}`} />
                      检查本地
                    </Button>
                    <Button variant="outline" size="sm" onClick={handleDeploy} disabled={isBusy} className="gap-1.5">
                      <Play className="h-3.5 w-3.5" />
                      部署
                    </Button>
                    {logBuffer.length > 0 && (
                      <Button variant="ghost" size="sm" onClick={() => setShowLogs(true)}
                        className="gap-1.5 text-muted-foreground ml-auto">
                        <ScrollText className="h-3.5 w-3.5" />
                        日志 ({logBuffer.length})
                      </Button>
                    )}
                  </div>
                  <textarea
                    className="w-full rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm font-mono resize-none focus:outline-none focus:ring-1 focus:ring-primary"
                    rows={3}
                    placeholder="在此测试输入法…"
                  />
                </CardContent>
              </Card>
            )}
          </div>
        )}

        {/* ══ 扩展 Tab ══════════════════════════════════════════════════════ */}
        {activeTab === "extension" && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <FolderOpen className="h-4 w-4 text-muted-foreground" />
                安装到自定义目录
                <div className="ml-auto">{VersionPicker}</div>
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <p className="text-xs text-muted-foreground">将方案安装到指定的输入法数据目录，安装完成后请手动重新部署输入法。</p>
              <div className="flex gap-2 flex-wrap">
                <Button variant="outline" size="sm" onClick={handleSelectDir} disabled={isInstallingExt} className="gap-1.5">
                  <FolderOpen className="h-4 w-4" />
                  {selectedDir ? "重新选择目录" : "选择目录"}
                </Button>
                {selectedDir && downloadUrl && (
                  <Button variant="secondary" size="sm" onClick={handleInstallExt} disabled={isInstallingExt} className="gap-1.5">
                    <Download className="h-4 w-4" />
                    {isInstallingExt ? "安装中..." : "立即安装"}
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
                  <FileList
                    files={files}
                    loading={isLoadingFiles}
                    onRefresh={() => loadFiles(selectedDir ?? undefined, safUri ?? undefined)}
                    disabled={isInstallingExt}
                  />
                </div>
              )}
              {isInstallingExt && extProgress && (
                <div className="space-y-1.5">
                  <Progress value={extProgress.percent} className="h-1.5" />
                  <p className="text-xs text-muted-foreground">{extProgress.message}</p>
                </div>
              )}
              {extError && (
                <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                  <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                  <span>{extError}</span>
                </div>
              )}
              {extResult && (
                <div className="rounded-lg border border-green-500/30 bg-green-500/10 px-3 py-2.5 space-y-1.5">
                  <div className="flex items-center gap-2 text-sm text-green-400">
                    <CheckCircle2 className="h-4 w-4 shrink-0" />
                    安装完成，请手动重新部署输入法
                  </div>
                  {extResult.verify.some((v) => !v.ok) && (
                    <p className="text-xs text-destructive">⚠ 有 {extResult.verify.filter((v) => !v.ok).length} 个文件校验失败</p>
                  )}
                  <details className="text-xs">
                    <summary className="cursor-pointer text-muted-foreground hover:text-foreground select-none py-1">
                      安装日志（{extResult.logs.length} 条）
                    </summary>
                    <div className="mt-1 max-h-48 overflow-y-auto rounded-md bg-muted/60 border border-border p-2 space-y-0.5">
                      {extResult.logs.map((line, i) => (
                        <div key={i} className={`font-mono text-[11px] leading-5 ${line.startsWith("[ERROR]") ? "text-destructive"
                          : line.startsWith("[WARN]") ? "text-yellow-400"
                            : line.startsWith("[MERGED]") || line.startsWith("[RENAMED]") ? "text-primary"
                              : "text-muted-foreground"
                          }`}>{line}</div>
                      ))}
                    </div>
                  </details>
                </div>
              )}
            </CardContent>
          </Card>
        )}

        {/* ── 日志弹窗 ──────────────────────────────────────────────────── */}
        <Dialog open={showLogs} onOpenChange={setShowLogs}>
          <DialogContent className="max-w-lg">
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <ScrollText className="h-4 w-4" />
                操作日志
              </DialogTitle>
              <DialogDescription asChild>
                <div className="space-y-2 pt-1">
                  <div className="flex justify-end">
                    <button onClick={() => setLogBuffer([])}
                      className="text-xs text-muted-foreground hover:text-destructive transition-colors">
                      清空日志
                    </button>
                  </div>
                  <div className="max-h-[60vh] overflow-y-auto rounded-md bg-muted/60 border border-border p-2 space-y-0.5">
                    {logBuffer.length === 0
                      ? <p className="text-xs text-muted-foreground py-4 text-center">暂无日志</p>
                      : logBuffer.map((line, i) => (
                        <div key={i} className={`font-mono text-[11px] leading-5 ${line.includes("[DEPLOY ERROR]") || line.includes("[ERROR]") ? "text-destructive"
                          : line.includes("[WARN]") ? "text-yellow-400"
                            : line.includes("[DEPLOY]") ? "text-green-400"
                              : line.includes("[MERGED]") || line.includes("[RENAMED]") ? "text-primary"
                                : "text-muted-foreground"
                          }`}>{line}</div>
                      ))}
                  </div>
                </div>
              </DialogDescription>
            </DialogHeader>
            <Button onClick={() => setShowLogs(false)} className="w-full mt-2">关闭</Button>
          </DialogContent>
        </Dialog>

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

      </div>
    </div>
  )
}
