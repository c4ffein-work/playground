// The tool providers every run gets, shared by the CLI and the tests.
import { fsTools } from "./fstools.ts";
import { webProvider } from "./net.ts";
import { skillsProvider } from "./skills.ts";
import type { ToolProvider } from "./tools.ts";

export function standardProviders(opts: { skillDirs?: string[]; onNet?: (agent: string, host: string, allowed: boolean) => void; bashProxy?: boolean } = {}): ToolProvider[] {
  return [fsTools, webProvider({ onDecision: opts.onNet, bashProxy: opts.bashProxy }), skillsProvider({ dirs: opts.skillDirs ?? [] })];
}
