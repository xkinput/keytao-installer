import { useState, useRef, useEffect, useCallback } from "react"
import { invoke } from "@tauri-apps/api/core"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { RefreshCw, ChevronLeft, ChevronRight, RotateCcw, Copy, Check, Cpu } from "lucide-react"

interface ImeState {
  preedit: string
  cursor: number
  candidates: ImeCandidate[]
  page: number
  is_last_page: boolean
  committed: string | null
  select_keys: string | null
}

interface ImeCandidate {
  text: string
  comment: string | null
}

// X11 keysym table for special keys
const SPECIAL_KEYSYMS: Record<string, number> = {
  Enter: 0xff0d,
  Backspace: 0xff08,
  Escape: 0xff1b,
  Delete: 0xffff,
  Tab: 0xff09,
  ArrowLeft: 0xff51,
  ArrowUp: 0xff52,
  ArrowRight: 0xff53,
  ArrowDown: 0xff54,
  Home: 0xff50,
  End: 0xff57,
  PageUp: 0xff55,
  PageDown: 0xff56,
  " ": 0x20,
}

function toRimeKey(e: KeyboardEvent): [number, number] | null {
  let mask = 0
  if (e.shiftKey) mask |= 0x0001
  if (e.ctrlKey) mask |= 0x0004
  if (e.altKey) mask |= 0x0008

  if (e.key in SPECIAL_KEYSYMS) {
    return [SPECIAL_KEYSYMS[e.key], mask]
  }
  if (e.key.length === 1) {
    const code = e.key.codePointAt(0)!
    if (code >= 0x20 && code <= 0x7e) {
      return [code, mask]
    }
  }
  return null
}

const EMPTY_STATE: ImeState = {
  preedit: "",
  cursor: 0,
  candidates: [],
  page: 0,
  is_last_page: true,
  committed: null,
  select_keys: null,
}

interface ImePanelProps {
  userDataDir?: string
}

export function ImePanel({ userDataDir }: ImePanelProps) {
  const [ready, setReady] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [imeState, setImeState] = useState<ImeState>(EMPTY_STATE)
  const [output, setOutput] = useState("")
  const [copied, setCopied] = useState(false)
  const [memoryMb, setMemoryMb] = useState<number | null>(null)
  const inputRef = useRef<HTMLDivElement>(null)

  const appendCommit = useCallback((text: string | null) => {
    if (text) setOutput((prev) => prev + text)
  }, [])

  async function initRime() {
    setLoading(true)
    setError(null)
    try {
      await invoke("rime_setup", {
        userDataDir: userDataDir ?? "",
        sharedDataDir: null,
      })
      setReady(true)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (ready) inputRef.current?.focus()
  }, [ready])

  useEffect(() => {
    if (!ready) return
    const poll = () =>
      invoke<number>("rime_memory_usage")
        .then((bytes) => setMemoryMb(bytes / 1024 / 1024))
        .catch(() => { })
    poll()
    const id = setInterval(poll, 2000)
    return () => clearInterval(id)
  }, [ready])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (!ready) return
      const key = toRimeKey(e.nativeEvent)
      if (!key) return
      e.preventDefault()

      invoke<ImeState>("rime_process_key", { keycode: key[0], mask: key[1] })
        .then((state) => {
          setImeState(state)
          appendCommit(state.committed)
        })
        .catch((err) => setError(String(err)))
    },
    [ready, appendCommit],
  )

  async function handleCandidateClick(index: number) {
    try {
      const state = await invoke<ImeState>("rime_select_candidate", { index })
      setImeState(state)
      appendCommit(state.committed)
    } catch (e) {
      setError(String(e))
    }
  }

  async function handleChangePage(backward: boolean) {
    try {
      const state = await invoke<ImeState>("rime_change_page", { backward })
      setImeState(state)
    } catch (e) {
      setError(String(e))
    }
  }

  async function handleReset() {
    try {
      const state = await invoke<ImeState>("rime_reset")
      setImeState(state)
    } catch (e) {
      setImeState(EMPTY_STATE)
    }
  }

  async function handleCopyOutput() {
    if (!output) return
    await navigator.clipboard.writeText(output)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="space-y-3">
      {/* Init button */}
      {!ready && (
        <div className="flex flex-col items-center gap-3 py-4">
          <p className="text-sm text-muted-foreground text-center">
            点击下方按钮初始化 librime 引擎
            {userDataDir && (
              <span className="block text-xs mt-1 font-mono text-muted-foreground/70">
                数据目录：{userDataDir}
              </span>
            )}
          </p>
          <Button onClick={initRime} disabled={loading} className="gap-2">
            {loading && <RefreshCw className="h-4 w-4 animate-spin" />}
            {loading ? "初始化中…" : "启动引擎"}
          </Button>
        </div>
      )}

      {error && (
        <div className="text-xs text-destructive bg-destructive/10 border border-destructive/20 rounded-md px-3 py-2">
          {error}
        </div>
      )}

      {ready && (
        <>
          {/* Composition / input area */}
          <div
            ref={inputRef}
            tabIndex={0}
            onKeyDown={handleKeyDown}
            className="relative min-h-10 px-3 py-2 rounded-md border border-border bg-background cursor-text focus:outline-none focus:ring-2 focus:ring-ring select-none"
            aria-label="输入区域"
          >
            {imeState.preedit ? (
              <span className="text-sm font-mono text-primary">{imeState.preedit}</span>
            ) : (
              <span className="text-sm text-muted-foreground/50">点击此处输入…</span>
            )}
          </div>

          {/* Candidate bar */}
          {imeState.candidates.length > 0 && (
            <div className="flex items-center gap-1 flex-wrap rounded-md border border-border bg-muted/30 px-2 py-1.5">
              {/* Previous page */}
              <button
                onClick={() => handleChangePage(true)}
                disabled={imeState.page === 0}
                className="p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-30 transition-colors"
                title="上一页"
              >
                <ChevronLeft className="h-3.5 w-3.5" />
              </button>

              {imeState.candidates.map((c, i) => {
                const label =
                  imeState.select_keys && i < imeState.select_keys.length
                    ? imeState.select_keys[i]
                    : String(i + 1)
                return (
                  <button
                    key={i}
                    onClick={() => handleCandidateClick(i)}
                    className="group px-1.5 py-0.5 rounded text-sm hover:bg-accent transition-colors flex items-baseline gap-0.5"
                  >
                    <span className="text-xs text-muted-foreground group-hover:text-muted-foreground/70">
                      {label}.
                    </span>
                    <span>{c.text}</span>
                    {c.comment && (
                      <span className="text-xs text-muted-foreground/70">{c.comment}</span>
                    )}
                  </button>
                )
              })}

              {/* Next page */}
              <button
                onClick={() => handleChangePage(false)}
                disabled={imeState.is_last_page}
                className="p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-30 transition-colors"
                title="下一页"
              >
                <ChevronRight className="h-3.5 w-3.5" />
              </button>

              {/* Page indicator */}
              <span className="ml-auto text-xs text-muted-foreground/60">
                第 {imeState.page + 1} 页
              </span>
            </div>
          )}

          {/* Controls */}
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" className="gap-1.5 h-7 text-xs" onClick={handleReset}>
              <RotateCcw className="h-3 w-3" />
              清除
            </Button>
            {memoryMb !== null && (
              <Badge variant="outline" className="gap-1 h-5 text-[10px] font-mono text-muted-foreground">
                <Cpu className="h-2.5 w-2.5" />
                {memoryMb.toFixed(1)} MB
              </Badge>
            )}
            {output && (
              <>
                <Button
                  variant="ghost"
                  size="sm"
                  className="gap-1.5 h-7 text-xs ml-auto"
                  onClick={handleCopyOutput}
                >
                  {copied ? (
                    <Check className="h-3 w-3 text-green-500" />
                  ) : (
                    <Copy className="h-3 w-3" />
                  )}
                  复制
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="gap-1.5 h-7 text-xs text-destructive hover:text-destructive"
                  onClick={() => setOutput("")}
                >
                  清空文本
                </Button>
              </>
            )}
          </div>

          {/* Committed output */}
          {output && (
            <div className="px-3 py-2 bg-muted/40 rounded-md border border-border/60 min-h-10">
              <p className="text-sm font-mono break-all whitespace-pre-wrap leading-relaxed">
                {output}
              </p>
            </div>
          )}

          {/* Keyboard hint */}
          <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground/60">
            {[
              ["Enter", "上屏"],
              ["Space", "选第一候选"],
              ["Backspace", "删除"],
              ["Esc", "清除"],
              ["= / -", "翻页"],
              ["1-9", "选候选"],
            ].map(([key, desc]) => (
              <span key={key}>
                <kbd className="font-mono bg-muted px-1 rounded text-[10px]">{key}</kbd>
                <span className="ml-1">{desc}</span>
              </span>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
