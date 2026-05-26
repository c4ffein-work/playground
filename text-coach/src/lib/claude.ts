import type { Suggestion } from "./types"

const API_URL = "https://api.anthropic.com/v1/messages"
const MODEL = "claude-haiku-4-5-20251001"

const TOOL = {
  name: "propose_edits",
  description:
    "Return a list of surgical edits to the user's text. Each edit refers to character offsets in the ORIGINAL text.",
  input_schema: {
    type: "object" as const,
    properties: {
      suggestions: {
        type: "array",
        items: {
          type: "object",
          properties: {
            start: { type: "integer", description: "0-indexed character start offset in the original text" },
            end: { type: "integer", description: "Character end offset, exclusive. Equals start for pure insertion." },
            replacement: { type: "string", description: "Text to replace the span with. Empty string = deletion." },
            rationale: { type: "string", description: "One short sentence explaining the fix." },
            category: {
              type: "string",
              enum: ["grammar", "clarity", "tone", "concision", "style"],
            },
          },
          required: ["start", "end", "replacement", "rationale", "category"],
        },
      },
    },
    required: ["suggestions"],
  },
}

type ToolUseBlock = {
  type: "tool_use"
  name: string
  input: { suggestions: Omit<Suggestion, "id" | "state">[] }
}

type MessageResponse = {
  content: ({ type: "text"; text: string } | ToolUseBlock)[]
}

export async function fetchSuggestions(
  apiKey: string,
  systemPrompt: string,
  text: string,
  signal: AbortSignal,
): Promise<Suggestion[]> {
  const res = await fetch(API_URL, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-api-key": apiKey,
      "anthropic-version": "2023-06-01",
      "anthropic-dangerous-direct-browser-access": "true",
    },
    body: JSON.stringify({
      model: MODEL,
      max_tokens: 1024,
      system: systemPrompt,
      tools: [TOOL],
      tool_choice: { type: "tool", name: "propose_edits" },
      messages: [
        {
          role: "user",
          content: `Here is the text to review. Reply by calling the propose_edits tool.\n\n--- BEGIN TEXT ---\n${text}\n--- END TEXT ---`,
        },
      ],
    }),
    signal,
  })

  if (!res.ok) {
    const body = await res.text().catch(() => "")
    throw new Error(`Anthropic API ${res.status}: ${body.slice(0, 200)}`)
  }

  const data = (await res.json()) as MessageResponse
  const toolUse = data.content.find((b): b is ToolUseBlock => b.type === "tool_use" && b.name === "propose_edits")
  if (!toolUse) return []

  return toolUse.input.suggestions
    .filter((s) => s.start >= 0 && s.end >= s.start && s.end <= text.length)
    .map((s, i) => ({
      ...s,
      id: `${Date.now()}-${i}`,
      state: "pending" as const,
    }))
}
