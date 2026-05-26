import { describe, expect, test } from "bun:test"
import { applySuggestion, overlaps } from "./offsets"
import type { Suggestion } from "./types"

function s(id: string, start: number, end: number, replacement: string): Suggestion {
  return { id, start, end, replacement, rationale: "", category: "grammar", state: "pending" }
}

describe("overlaps", () => {
  test("disjoint ranges do not overlap", () => {
    expect(overlaps({ start: 0, end: 5 }, { start: 5, end: 10 })).toBe(false)
    expect(overlaps({ start: 0, end: 5 }, { start: 10, end: 15 })).toBe(false)
  })
  test("intersecting ranges overlap", () => {
    expect(overlaps({ start: 0, end: 5 }, { start: 3, end: 8 })).toBe(true)
    expect(overlaps({ start: 0, end: 10 }, { start: 3, end: 5 })).toBe(true)
  })
  test("pure insertion inside a span counts as overlap", () => {
    expect(overlaps({ start: 3, end: 3 }, { start: 0, end: 5 })).toBe(true)
  })
  test("insertion at boundary does not overlap", () => {
    expect(overlaps({ start: 5, end: 5 }, { start: 0, end: 5 })).toBe(false)
    expect(overlaps({ start: 0, end: 0 }, { start: 0, end: 5 })).toBe(false)
  })
})

describe("applySuggestion", () => {
  const text = "the cat sat on the mat"
  //            0123456789012345678901
  //            0         1         2

  test("applies a replacement", () => {
    const a = s("a", 4, 7, "dog")
    const res = applySuggestion(text, a, [a])
    expect(res.text).toBe("the dog sat on the mat")
    expect(res.suggestions[0].state).toBe("applied")
  })

  test("equal-length replacement leaves later offsets unchanged", () => {
    const a = s("a", 4, 7, "dog") // 3 -> 3 chars, delta 0
    const b = s("b", 19, 22, "rug")
    const res = applySuggestion(text, a, [a, b])
    expect(res.text).toBe("the dog sat on the mat")
    const after = res.suggestions.find((x) => x.id === "b")!
    expect(after.start).toBe(19)
    expect(after.end).toBe(22)
  })

  test("positive delta shifts forward", () => {
    const a = s("a", 4, 7, "puppy") // 3 -> 5 chars, delta +2
    const b = s("b", 19, 22, "rug")
    const res = applySuggestion(text, a, [a, b])
    expect(res.text).toBe("the puppy sat on the mat")
    const after = res.suggestions.find((x) => x.id === "b")!
    expect(after.start).toBe(21)
    expect(after.end).toBe(24)
  })

  test("negative delta shifts backward", () => {
    const a = s("a", 4, 7, "x") // 3 -> 1, delta -2
    const b = s("b", 19, 22, "rug")
    const res = applySuggestion(text, a, [a, b])
    expect(res.text).toBe("the x sat on the mat")
    const after = res.suggestions.find((x) => x.id === "b")!
    expect(after.start).toBe(17)
    expect(after.end).toBe(20)
  })

  test("overlapping suggestions are marked superseded, not removed", () => {
    const a = s("a", 4, 7, "dog")
    const b = s("b", 5, 8, "rat")
    const res = applySuggestion(text, a, [a, b])
    const sup = res.suggestions.find((x) => x.id === "b")!
    expect(sup.state).toBe("superseded")
    expect(sup.start).toBe(5)
  })

  test("suggestions before the edit are untouched", () => {
    const a = s("a", 19, 22, "rug")
    const b = s("b", 4, 7, "dog")
    const res = applySuggestion(text, a, [a, b])
    const before = res.suggestions.find((x) => x.id === "b")!
    expect(before.start).toBe(4)
    expect(before.end).toBe(7)
  })

  test("non-pending suggestions are not touched", () => {
    const a = s("a", 4, 7, "dog")
    const b: Suggestion = { ...s("b", 19, 22, "rug"), state: "rejected" }
    const res = applySuggestion(text, a, [a, b])
    const rej = res.suggestions.find((x) => x.id === "b")!
    expect(rej.state).toBe("rejected")
    expect(rej.start).toBe(19)
  })
})
