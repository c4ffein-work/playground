import type { Suggestion } from "./types"

export function overlaps(a: { start: number; end: number }, b: { start: number; end: number }): boolean {
  // Treat zero-width spans (pure insertions) as overlapping only if at the same point INSIDE a real span.
  if (a.start === a.end && b.start === b.end) return a.start === b.start
  if (a.start === a.end) return a.start > b.start && a.start < b.end
  if (b.start === b.end) return b.start > a.start && b.start < a.end
  return a.start < b.end && a.end > b.start
}

export type ApplyResult = {
  text: string
  suggestions: Suggestion[]
}

export function applySuggestion(text: string, applied: Suggestion, all: Suggestion[]): ApplyResult {
  const newText = text.slice(0, applied.start) + applied.replacement + text.slice(applied.end)
  const delta = applied.replacement.length - (applied.end - applied.start)

  const updated: Suggestion[] = all.map((s) => {
    if (s.id === applied.id) {
      return { ...s, state: "applied" as const }
    }
    if (s.state !== "pending") {
      return s
    }
    if (overlaps(s, applied)) {
      return { ...s, state: "superseded" as const }
    }
    if (s.start >= applied.end) {
      return { ...s, start: s.start + delta, end: s.end + delta }
    }
    return s
  })

  return { text: newText, suggestions: updated }
}

export function rejectSuggestion(id: string, all: Suggestion[]): Suggestion[] {
  return all.map((s) => (s.id === id ? { ...s, state: "rejected" as const } : s))
}

export function markAllStale(all: Suggestion[]): Suggestion[] {
  return all.map((s) => (s.state === "pending" ? { ...s, state: "stale" as const } : s))
}
