import type { Suggestion } from "../lib/types"

type Props = {
  text: string
  suggestions: Suggestion[]
  onApply: (s: Suggestion) => void
  onReject: (id: string) => void
  onHover: (id: string | null) => void
  loading: boolean
  error: string | null
}

const CATEGORY_COLORS: Record<Suggestion["category"], string> = {
  grammar: "bg-red-500/15 text-red-300 border-red-500/30",
  clarity: "bg-amber-500/15 text-amber-300 border-amber-500/30",
  tone: "bg-violet-500/15 text-violet-300 border-violet-500/30",
  concision: "bg-sky-500/15 text-sky-300 border-sky-500/30",
  style: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
}

function snippet(text: string, s: Suggestion): string {
  const original = text.slice(s.start, s.end)
  if (original.length === 0) return `insert "${s.replacement}"`
  if (s.replacement.length === 0) return `delete "${original}"`
  return `"${original}" → "${s.replacement}"`
}

export function SuggestionList({ text, suggestions, onApply, onReject, onHover, loading, error }: Props) {
  const pending = suggestions.filter((s) => s.state === "pending")
  const otherStates = suggestions.filter((s) => s.state !== "pending" && s.state !== "stale")

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-zinc-300">Suggestions</h2>
        <div className="text-xs text-zinc-500">
          {loading ? "thinking…" : `${pending.length} pending`}
        </div>
      </div>

      {error && (
        <div className="rounded border border-red-500/40 bg-red-500/10 px-2 py-1 text-xs text-red-300">
          {error}
        </div>
      )}

      {pending.length === 0 && !loading && !error && (
        <div className="text-xs text-zinc-500 italic">No suggestions yet. Keep typing.</div>
      )}

      <ul className="flex flex-col gap-2">
        {pending.map((s) => (
          <li
            key={s.id}
            onMouseEnter={() => onHover(s.id)}
            onMouseLeave={() => onHover(null)}
            className="rounded border border-zinc-700 bg-zinc-900/70 p-2"
          >
            <div className="flex items-start justify-between gap-2">
              <span className={`text-[10px] uppercase tracking-wide rounded border px-1.5 py-0.5 ${CATEGORY_COLORS[s.category]}`}>
                {s.category}
              </span>
              <div className="flex gap-1">
                <button
                  type="button"
                  onClick={() => onApply(s)}
                  className="text-xs rounded bg-emerald-600/80 hover:bg-emerald-500 px-2 py-0.5 text-white"
                >
                  Apply
                </button>
                <button
                  type="button"
                  onClick={() => onReject(s.id)}
                  className="text-xs rounded bg-zinc-700 hover:bg-zinc-600 px-2 py-0.5 text-zinc-200"
                >
                  Dismiss
                </button>
              </div>
            </div>
            <div className="mt-1.5 text-sm text-zinc-200 break-words">{snippet(text, s)}</div>
            <div className="mt-1 text-xs text-zinc-400">{s.rationale}</div>
          </li>
        ))}
      </ul>

      {otherStates.length > 0 && (
        <details className="mt-2 text-xs text-zinc-500">
          <summary className="cursor-pointer hover:text-zinc-300">History ({otherStates.length})</summary>
          <ul className="mt-1 flex flex-col gap-1">
            {otherStates.map((s) => (
              <li key={s.id} className="opacity-60">
                <span className="text-[10px] uppercase tracking-wide mr-1">[{s.state}]</span>
                {snippet(text, s)}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  )
}
