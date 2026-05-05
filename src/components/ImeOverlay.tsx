import { useState, useRef, useEffect, useCallback } from "react"
import { invoke } from "@tauri-apps/api/core"
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow"
import { ChevronLeft, ChevronRight } from "lucide-react"

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

  if (e.key in SPECIAL_KEYSYMS) return [SPECIAL_KEYSYMS[e.key], mask]
  if (e.key.length === 1) {
    const code = e.key.codePointAt(0)!
    if (code >= 0x20 && code <= 0x7e) return [code, mask]
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

export function ImeOverlay() {
  const [ready, setReady] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [imeState, setImeState] = useState<ImeState>(EMPTY_STATE)
  const inputRef = useRef<HTMLDivElement>(null)
  const overlayRef = useRef<HTMLDivElement>(null)

  // Auto-initialize rime engine when the overlay mounts
  useEffect(() => {
    invoke("rime_setup", { userDataDir: "", sharedDataDir: null })
      .then(() => {
        setReady(true)
        setLoading(false)
      })
      .catch((e) => {
        setError(String(e))
        setLoading(false)
      })
  }, [])

  useEffect(() => {
    if (ready) inputRef.current?.focus()
  }, [ready])

  const commitAndInject = useCallback((text: string) => {
    invoke("rime_inject_text", { text }).catch(() => { })
    setImeState(EMPTY_STATE)
  }, [])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (!ready) return

      // Escape with no preedit = dismiss overlay
      if (e.key === "Escape" && !imeState.preedit) {
        getCurrentWebviewWindow().hide()
        return
      }

      const key = toRimeKey(e.nativeEvent)
      if (!key) return
      e.preventDefault()

      invoke<ImeState>("rime_process_key", { keycode: key[0], mask: key[1] })
        .then((state) => {
          setImeState(state)
          if (state.committed) commitAndInject(state.committed)
        })
        .catch((err) => setError(String(err)))
    },
    [ready, imeState.preedit, commitAndInject],
  )

  async function handleCandidateClick(index: number) {
    const state = await invoke<ImeState>("rime_select_candidate", { index })
    setImeState(state)
    if (state.committed) commitAndInject(state.committed)
  }

  async function handleChangePage(backward: boolean) {
    const state = await invoke<ImeState>("rime_change_page", { backward })
    setImeState(state)
  }

  return (
    <div className="h-screen w-screen flex items-end justify-center pb-2 select-none">
      <div
        ref={overlayRef}
        className="w-[580px] rounded-xl border border-border/60 bg-background/95 backdrop-blur-sm shadow-2xl overflow-hidden"
      >
        {/* Preedit row */}
        <div
          ref={inputRef}
          tabIndex={0}
          onKeyDown={handleKeyDown}
          className="px-3 py-2 min-h-9 focus:outline-none cursor-text"
          aria-label="IME 输入"
        >
          {loading ? (
            <span className="text-xs text-muted-foreground animate-pulse">引擎加载中…</span>
          ) : error ? (
            <span className="text-xs text-destructive">{error}</span>
          ) : ready && imeState.preedit ? (
            <span className="text-sm font-mono text-primary">{imeState.preedit}</span>
          ) : (
            <span className="text-xs text-muted-foreground/40">
              {ready ? "键入拼音/码 · Esc 关闭" : ""}
            </span>
          )}
        </div>

        {/* Candidate bar — only shown when there are candidates */}
        {imeState.candidates.length > 0 && (
          <div className="flex items-center gap-1 border-t border-border/40 bg-muted/30 px-2 py-1.5">
            <button
              onClick={() => handleChangePage(true)}
              disabled={imeState.page === 0}
              className="p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-30 transition-colors"
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
                  <span className="text-xs text-muted-foreground">{label}.</span>
                  <span>{c.text}</span>
                  {c.comment && (
                    <span className="text-xs text-muted-foreground/60">{c.comment}</span>
                  )}
                </button>
              )
            })}

            <button
              onClick={() => handleChangePage(false)}
              disabled={imeState.is_last_page}
              className="p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-30 transition-colors"
            >
              <ChevronRight className="h-3.5 w-3.5" />
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
