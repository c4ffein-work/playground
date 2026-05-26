export type Category = "grammar" | "clarity" | "tone" | "concision" | "style"

export type SuggestionState = "pending" | "applied" | "rejected" | "superseded" | "stale"

export type Suggestion = {
  id: string
  start: number
  end: number
  replacement: string
  rationale: string
  category: Category
  state: SuggestionState
}

export type PresetId = "discord" | "official" | "email" | "tweet" | "commit" | "plain"

export type Preset = {
  id: PresetId
  label: string
  description: string
  systemPrompt: string
}
