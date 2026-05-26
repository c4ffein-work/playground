import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Settings, loadApiKey } from "./components/Settings"
import { SuggestionList } from "./components/SuggestionList"
import { DEFAULT_PRESET, PRESETS } from "./lib/presets"
import { fetchSuggestions } from "./lib/claude"
import { applySuggestion, markAllStale, rejectSuggestion } from "./lib/offsets"
import type { PresetId, Suggestion } from "./lib/types"

const DEBOUNCE_MS = 1500
const MIN_CHARS = 10

function App() {
  const [apiKey, setApiKey] = useState(loadApiKey())
  const [presetId, setPresetId] = useState<PresetId>(DEFAULT_PRESET)
  const [text, setText] = useState("")
  const [suggestions, setSuggestions] = useState<Suggestion[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [, setHoveredId] = useState<string | null>(null)

  const abortRef = useRef<AbortController | null>(null)
  const debounceRef = useRef<number | null>(null)
  const requestedTextRef = useRef<string>("")

  const preset = useMemo(() => PRESETS.find((p) => p.id === presetId)!, [presetId])

  const runQuery = useCallback(
    (textToReview: string) => {
      if (!apiKey) {
        setError("Set your Anthropic API key first.")
        return
      }
      if (textToReview.trim().length < MIN_CHARS) return

      abortRef.current?.abort()
      const ctrl = new AbortController()
      abortRef.current = ctrl
      requestedTextRef.current = textToReview

      setLoading(true)
      setError(null)

      fetchSuggestions(apiKey, preset.systemPrompt, textToReview, ctrl.signal)
        .then((newSugg) => {
          if (ctrl.signal.aborted) return
          if (requestedTextRef.current !== textToReview) return
          setSuggestions((prev) => {
            const historical = prev.filter((s) => s.state !== "pending" && s.state !== "stale")
            return [...historical, ...newSugg]
          })
          setLoading(false)
        })
        .catch((err: unknown) => {
          if (ctrl.signal.aborted) return
          setError(err instanceof Error ? err.message : String(err))
          setLoading(false)
        })
    },
    [apiKey, preset.systemPrompt],
  )

  useEffect(() => {
    if (debounceRef.current) window.clearTimeout(debounceRef.current)
    setSuggestions((prev) => markAllStale(prev))
    if (text.trim().length < MIN_CHARS) {
      setLoading(false)
      return
    }
    debounceRef.current = window.setTimeout(() => {
      runQuery(text)
    }, DEBOUNCE_MS)
    return () => {
      if (debounceRef.current) window.clearTimeout(debounceRef.current)
    }
  }, [text, presetId, runQuery])

  const onApply = useCallback(
    (s: Suggestion) => {
      const { text: newText, suggestions: newSugg } = applySuggestion(text, s, suggestions)
      setText(newText)
      setSuggestions(newSugg)
    },
    [text, suggestions],
  )

  const onReject = useCallback((id: string) => {
    setSuggestions((prev) => rejectSuggestion(id, prev))
  }, [])

  return (
    <div className="mx-auto flex h-full max-w-6xl flex-col gap-4 p-6">
      <header className="flex items-center justify-between">
        <h1 className="text-lg font-medium">
          <span className="text-violet-400">text</span>-coach
        </h1>
        <Settings apiKey={apiKey} onChange={setApiKey} />
      </header>

      <div className="flex items-center gap-2">
        <label className="text-xs text-zinc-400">Style:</label>
        <select
          value={presetId}
          onChange={(e) => setPresetId(e.target.value as PresetId)}
          className="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-sm"
        >
          {PRESETS.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
        <span className="text-xs text-zinc-500">{preset.description}</span>
      </div>

      <div className="grid flex-1 grid-cols-1 gap-4 md:grid-cols-[1.6fr_1fr] min-h-0">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Start typing. Suggestions will appear on the right after you pause."
          className="h-full min-h-[300px] resize-none rounded border border-zinc-700 bg-zinc-950 p-3 font-mono text-sm leading-relaxed focus:border-violet-500 focus:outline-none"
          spellCheck={false}
        />

        <div className="flex flex-col min-h-0 overflow-y-auto">
          <SuggestionList
            text={text}
            suggestions={suggestions}
            onApply={onApply}
            onReject={onReject}
            onHover={setHoveredId}
            loading={loading}
            error={error}
          />
        </div>
      </div>

      <footer className="text-[11px] text-zinc-600">
        Calls go directly to api.anthropic.com from your browser. Key stays in localStorage. Debounce: {DEBOUNCE_MS}ms.
      </footer>
    </div>
  )
}

export default App
