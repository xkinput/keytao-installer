import { useState, useEffect, useRef } from "react"
import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import { platform } from "@tauri-apps/plugin-os"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
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
} from "lucide-react"

type OSType = "windows" | "macos" | "linux" | "android" | "ios" | "unknown"

interface ReleaseInfo {
  version: string
  name: string
  published_at: string
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

interface RimeInfo {
  name: string
  url: string
  configPath: string
  commands?: string[]
  note?: string
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
      <div className="text-sm">{children}</div>
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

export default function App() {
  const [osType, setOsType] = useState<OSType>("unknown")
  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo | null>(null)
  const [releaseError, setReleaseError] = useState<string | null>(null)
  const [selectedDir, setSelectedDir] = useState<string | null>(null)
  const [progress, setProgress] = useState<InstallProgress | null>(null)
  const [installSuccess, setInstallSuccess] = useState(false)
  const [installError, setInstallError] = useState<string | null>(null)
  const [isInstalling, setIsInstalling] = useState(false)
  const [isFetchingRelease, setIsFetchingRelease] = useState(true)
  const unlistenRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    const p = platform()
    const map: Record<string, OSType> = {
      macos: "macos", windows: "windows", linux: "linux",
      android: "android", ios: "ios",
    }
    setOsType(map[p] ?? "unknown")

    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))

    listen<InstallProgress>("install-progress", (e) => {
      setProgress(e.payload)
    }).then((fn) => { unlistenRef.current = fn })

    return () => unlistenRef.current?.()
  }, [])

  const osMeta = OS_META[osType]
  const downloadUrl = releaseInfo?.download_urls[osType as keyof typeof releaseInfo.download_urls]

  async function handleSelectDir() {
    const dir = await invoke<string | null>("select_directory")
    if (dir) {
      setSelectedDir(dir)
      setInstallSuccess(false)
      setInstallError(null)
    }
  }

  async function handleInstall() {
    if (!selectedDir || !downloadUrl) return
    setIsInstalling(true)
    setInstallSuccess(false)
    setInstallError(null)
    setProgress(null)
    try {
      await invoke("download_and_install", { url: downloadUrl, destPath: selectedDir })
      setInstallSuccess(true)
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
        <div className="text-center space-y-1 pb-2">
          <h1 className="text-2xl font-bold tracking-tight">键道安装器</h1>
          <p className="text-sm text-muted-foreground">
            自动下载最新版键道输入方案并安装到 Rime 配置目录
          </p>
        </div>

        {/* Step 1: System Info + Rime Guide */}
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
                    className="text-primary hover:underline inline-flex items-center gap-1"
                  >
                    {osMeta.rime.url}
                    <ExternalLink className="h-3 w-3" />
                  </a>
                </InfoRow>
                <InfoRow label="配置目录">
                  <code className="text-xs bg-muted px-2 py-0.5 rounded font-mono">
                    {osMeta.rime.configPath}
                  </code>
                </InfoRow>
              </div>
            </CardContent>
          </Card>
        )}

        {/* Step 2: Install KeyTao */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-sm font-semibold flex items-center gap-2">
              <span className="flex items-center justify-center w-5 h-5 rounded-full bg-primary text-primary-foreground text-xs font-bold">2</span>
              安装键道方案
              {releaseInfo && (
                <Badge variant="secondary" className="ml-auto font-mono text-xs">
                  {releaseInfo.version}
                </Badge>
              )}
              {isFetchingRelease && (
                <RefreshCw className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />
              )}
              {releaseError && !isFetchingRelease && (
                <Button
                  variant="ghost"
                  size="icon"
                  className="ml-auto h-6 w-6"
                  onClick={handleRefetchRelease}
                  title="重试获取版本信息"
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                </Button>
              )}
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
              <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
              <span>安装将<strong>覆盖目标目录中的同名文件</strong>，请提前备份个人配置与词库</span>
            </div>

            {/* Directory Selection */}
            <div className="space-y-2">
              <div className="flex gap-2">
                <Button
                  variant="default"
                  size="sm"
                  onClick={handleSelectDir}
                  disabled={isInstalling}
                  className="gap-1.5"
                >
                  <FolderOpen className="h-4 w-4" />
                  {selectedDir ? "重新选择目录" : "选择安装目录"}
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

              {selectedDir && (
                <div className="flex items-center gap-2 bg-muted/40 border border-border rounded-lg px-3 py-2">
                  <CheckCircle2 className="h-4 w-4 text-green-500 shrink-0" />
                  <code className="text-xs font-mono text-muted-foreground break-all">
                    {selectedDir}
                  </code>
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

            {/* Result */}
            {installSuccess && (
              <div className="flex items-start gap-2 text-sm text-green-400 bg-green-400/10 border border-green-400/20 rounded-lg px-3 py-2.5">
                <CheckCircle2 className="h-4 w-4 shrink-0 mt-0.5" />
                <span>安装完成！请在输入法中点击<strong>重新部署</strong>以生效</span>
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

      </div>
    </div>
  )
}
