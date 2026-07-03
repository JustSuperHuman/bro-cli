/**
 * Shared helpers for both the OpenAI and Anthropic adapters: model-alias
 * resolution and flattening a chat history into a single prompt string (the
 * Claude CLI's `--print` mode takes one prompt, so we serialize the turns).
 */

import type { ClaudeModelAlias } from "../subprocess/claude.ts";

const MODEL_MAP: Record<string, ClaudeModelAlias> = {
  opus: "opus",
  sonnet: "sonnet",
  haiku: "haiku",
  "claude-opus-4-8": "opus",
  "claude-opus-4-7": "opus",
  "claude-opus-4-6": "opus",
  "claude-opus-4-5": "opus",
  "claude-opus-4-1": "opus",
  "claude-opus-4": "opus",
  "claude-sonnet-5": "sonnet",
  "claude-sonnet-4-6": "sonnet",
  "claude-sonnet-4-5": "sonnet",
  "claude-sonnet-4": "sonnet",
  "claude-haiku-4-5": "haiku",
  "claude-haiku-4": "haiku",
};

export function resolveModel(model: string | undefined): ClaudeModelAlias {
  if (!model) return "sonnet";
  const direct = MODEL_MAP[model];
  if (direct) return direct;
  const stripped = model.replace(/^(?:claude-max-pool|claude-code-cli|claude-max)\//, "");
  if (MODEL_MAP[stripped]) return MODEL_MAP[stripped]!;
  const lower = model.toLowerCase();
  if (lower.includes("opus")) return "opus";
  if (lower.includes("haiku")) return "haiku";
  return "sonnet";
}

/** A provider-neutral chat message once content is flattened to text. */
export interface FlatMessage {
  role: "system" | "user" | "assistant";
  text: string;
}

export function buildPrompt(messages: FlatMessage[]): string {
  const parts: string[] = [];
  for (const m of messages) {
    const text = m.text.trim();
    if (!text) continue;
    if (m.role === "system") parts.push(`<system>\n${text}\n</system>`);
    else if (m.role === "assistant") parts.push(`<previous_response>\n${text}\n</previous_response>`);
    else parts.push(text);
  }
  return parts.join("\n\n").trim();
}

/** Rough token estimate used only when the CLI doesn't report input tokens. */
export function estimateTokens(text: string): number {
  return Math.max(1, Math.ceil(text.length / 4));
}
