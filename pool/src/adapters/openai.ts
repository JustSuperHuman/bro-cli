/**
 * OpenAI Chat Completions adapter.
 *
 * Parses an OpenAI /v1/chat/completions request into a prompt + model, and
 * formats a normalized turn stream back into either a single JSON completion or
 * an SSE stream of chat.completion.chunk objects.
 */

import type { FlatMessage } from "./shared.ts";
import { buildPrompt, resolveModel, estimateTokens } from "./shared.ts";
import type { ClaudeModelAlias, TurnEvent } from "../subprocess/claude.ts";

interface OAContentBlock { type: string; text?: string }
interface OAMessage { role: string; content: string | OAContentBlock[] }
export interface OpenAIChatRequest {
  model?: string;
  messages: OAMessage[];
  stream?: boolean;
  user?: string;
}

function flattenContent(content: string | OAContentBlock[]): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .filter((b) => b.type === "text" || b.type === "input_text")
      .map((b) => b.text ?? "")
      .join("\n");
  }
  return "";
}

export interface ParsedOpenAI {
  prompt: string;
  model: ClaudeModelAlias;
  requestedModel: string;
  stream: boolean;
  sessionKey?: string;
  promptChars: number;
}

export function parseOpenAI(body: OpenAIChatRequest): ParsedOpenAI {
  const flat: FlatMessage[] = (body.messages ?? []).map((m) => ({
    role: m.role === "system" || m.role === "assistant" ? m.role : "user",
    text: flattenContent(m.content),
  }));
  const prompt = buildPrompt(flat);
  return {
    prompt,
    model: resolveModel(body.model),
    requestedModel: body.model ?? "sonnet",
    stream: Boolean(body.stream),
    sessionKey: body.user,
    promptChars: prompt.length,
  };
}

function id(): string {
  return `chatcmpl-${crypto.randomUUID().replace(/-/g, "").slice(0, 24)}`;
}

/** Non-streaming: collect the whole turn and return one completion object. */
export async function collectOpenAI(
  events: AsyncGenerator<TurnEvent>,
  parsed: ParsedOpenAI,
): Promise<{ status: number; body: unknown }> {
  let text = "";
  let promptTokens = estimateTokens(parsed.prompt);
  let completionTokens = 0;
  let finish = "stop";

  for await (const ev of events) {
    if (ev.kind === "text") text += ev.text;
    else if (ev.kind === "text_block_boundary") text += "\n\n";
    else if (ev.kind === "done") {
      if (ev.usage.input_tokens) promptTokens = ev.usage.input_tokens;
      completionTokens = ev.usage.output_tokens || estimateTokens(text);
    } else if (ev.kind === "error") {
      return {
        status: ev.rateLimited ? 429 : 502,
        body: { error: { message: ev.message, type: ev.rateLimited ? "rate_limit_error" : "upstream_error" } },
      };
    }
  }

  return {
    status: 200,
    body: {
      id: id(),
      object: "chat.completion",
      created: Math.floor(Date.now() / 1000),
      model: parsed.requestedModel,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: text },
          finish_reason: finish,
        },
      ],
      usage: {
        prompt_tokens: promptTokens,
        completion_tokens: completionTokens,
        total_tokens: promptTokens + completionTokens,
      },
    },
  };
}

/**
 * Streaming: yield SSE lines for OpenAI chat.completion.chunk. The caller writes
 * these to the response body. Errors are surfaced as a final chunk + a data
 * line so OpenAI clients don't hang.
 */
export async function* streamOpenAI(
  events: AsyncGenerator<TurnEvent>,
  parsed: ParsedOpenAI,
): AsyncGenerator<string> {
  const cmplId = id();
  const created = Math.floor(Date.now() / 1000);
  const base = { id: cmplId, object: "chat.completion.chunk", created, model: parsed.requestedModel };

  // Opening chunk with the assistant role.
  yield sse({ ...base, choices: [{ index: 0, delta: { role: "assistant" }, finish_reason: null }] });

  let errored = false;
  for await (const ev of events) {
    if (ev.kind === "text") {
      yield sse({ ...base, choices: [{ index: 0, delta: { content: ev.text }, finish_reason: null }] });
    } else if (ev.kind === "text_block_boundary") {
      yield sse({ ...base, choices: [{ index: 0, delta: { content: "\n\n" }, finish_reason: null }] });
    } else if (ev.kind === "error") {
      errored = true;
      yield sse({
        ...base,
        choices: [{ index: 0, delta: {}, finish_reason: ev.rateLimited ? "length" : "stop" }],
        error: { message: ev.message, type: ev.rateLimited ? "rate_limit_error" : "upstream_error" },
      });
    }
  }

  if (!errored) {
    yield sse({ ...base, choices: [{ index: 0, delta: {}, finish_reason: "stop" }] });
  }
  yield "data: [DONE]\n\n";
}

function sse(obj: unknown): string {
  return `data: ${JSON.stringify(obj)}\n\n`;
}
