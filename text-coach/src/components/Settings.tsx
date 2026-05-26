import { useEffect, useState } from "react"

type Props = {
  apiKey: string
  onChange: (key: string) => void
}

const STORAGE_KEY = "text-coach.apiKey"

export function loadApiKey(): string {
  return localStorage.getItem(STORAGE_KEY) ?? ""
}

export function saveApiKey(key: string): void {
  if (key) localStorage.setItem(STORAGE_KEY, key)
  else localStorage.removeItem(STORAGE_KEY)
}

export function Settings({ apiKey, onChange }: Props) {
  const [open, setOpen] = useState(!apiKey)
  const [draft, setDraft] = useState(apiKey)

  useEffect(() => {
    setDraft(apiKey)
  }, [apiKey])

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="text-xs text-zinc-400 hover:text-zinc-200 underline-offset-2 hover:underline"
      >
        {apiKey ? "API key set ✓" : "Set API key"}
      </button>
      {open && (
        <div className="absolute right-0 top-6 z-10 w-96 rounded border border-zinc-700 bg-zinc-900 p-3 shadow-lg">
          <label className="block text-xs text-zinc-400 mb-1">Anthropic API key</label>
          <input
            type="password"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="sk-ant-..."
            className="w-full rounded border border-zinc-700 bg-zinc-950 px-2 py-1 text-sm font-mono"
            autoFocus
          />
          <p className="text-[11px] text-zinc-500 mt-1">
            Stored in your browser&apos;s localStorage. Calls go directly from your browser to Anthropic.
          </p>
          <div className="flex justify-end gap-2 mt-2">
            <button
              type="button"
              onClick={() => {
                setDraft("")
                saveApiKey("")
                onChange("")
              }}
              className="text-xs text-zinc-400 hover:text-zinc-200"
            >
              Clear
            </button>
            <button
              type="button"
              onClick={() => {
                saveApiKey(draft.trim())
                onChange(draft.trim())
                setOpen(false)
              }}
              className="text-xs rounded bg-violet-600 px-2 py-1 text-white hover:bg-violet-500"
            >
              Save
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
