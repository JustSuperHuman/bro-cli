/**
 * Anthropic Messages API adapter (/v1/messages).
 *
 * Parses an Anthropic Messages request and formats the normalized turn stream
 * back into Anthropic's non-streaming Message object or its SSE event sequence.
 */

import type { FlatMessage } from "./shared.ts";
import { buildPrompt, resolveModel, estimateTokens } from "./shared.ts";
import type { ClaudeModelAlias, TurnEvent } from "../subprocess/claude.ts";

interface AnthContentBlock { type: string; text?: string }
interface AnthMessage { role: string; content: string | AnthContentBlock[] }
export interface AnthropicRequest {
  model?: string;
  system?: string | AnthContentBlock[];
  messages: AnthMessage[];
  stream?: boolean;
  metadata?: { user_id?: string };
}

function flatten(content: string | AnthContentBlock[]): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .filter((b) => b.type === "text")
      .map((b) => b.text ?? "")
      .join("\n");
  }
  return "";
}

export interface ParsedAnthropic {
  prompt: string;
  model: ClaudeModelAlias;
  requestedModel: string;
  stream: boolean;
  sessionKey?: string;
}

export function parseAnthropic(body: AnthropicRequest): ParsedAnthropic {
  const flat: FlatMessage[] = [];
  if (body.system) flat.push({ role: "system", text: flatten(body.system) });
  for (const m of body.messages ?? []) {
    flat.push({
      role: m.role === "assistant" ? "assistant" : "user",
      text: flatten(m.content),
    });
  }
  const prompt = buildPrompt(flat);
  return {
    prompt,
    model: resolveModel(body.model),
    requestedModel: body.model ?? "claude-sonnet-5",
    stream: Boolean(body.stream),
    sessionKey: body.metadata?.user_id,
  };
}

function msgId(): string {
  return `msg_${crypto.randomUUID().replace(/-/g, "").slice(0, 24)}`;
}

export async function collectAnthropic(
  events: AsyncGenerator<TurnEvent>,
  parsed: ParsedAnthropic,
): Promise<{ status: number; body: unknown }> {
  let text = "";
  let inputTokens = estimateTokens(parsed.prompt);
  let outputTokens = 0;

  for await (const ev of events) {
    if (ev.kind === "text") text += ev.text;
    else if (ev.kind === "text_block_boundary") text += "\n\n";
    else if (ev.kind === "done") {
      if (ev.usage.input_tokens) inputTokens = ev.usage.input_tokens;
      outputTokens = ev.usage.output_tokens || estimateTokens(text);
    } else if (ev.kind === "error") {
      return {
        status: ev.rateLimited ? 429 : 502,
        body: {
          type: "error",
          error: {
            type: ev.rateLimited ? "rate_limit_error" : "api_error",
            message: ev.message,
          },
        },
      };
    }
  }

  return {
    status: 200,
    body: {
      id: msgId(),
      type: "message",
      role: "assistant",
      model: parsed.requestedModel,
      content: [{ type: "text", text }],
      stop_reason: "end_turn",
      stop_sequence: null,
      usage: { input_tokens: inputTokens, output_tokens: outputTokens },
    },
  };
}

/** Streaming: emit the Anthropic Messages SSE event sequence. */
export async function* streamAnthropic(
  events: AsyncGenerator<TurnEvent>,
  parsed: ParsedAnthropic,
): AsyncGenerator<string> {
  const id = msgId();
  const inputTokens = estimateTokens(parsed.prompt);

  yield sse("message_start", {
    type: "message_start",
    message: {
      id,
      type: "message",
      role: "assistant",
      model: parsed.requestedModel,
      content: [],
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: inputTokens, output_tokens: 0 },
    },
  });
  yield sse("content_block_start", {
    type: "content_block_start",
    index: 0,
    content_block: { type: "text", text: "" },
  });

  let outputTokens = 0;
  let errored = false;

  for await (const ev of events) {
    if (ev.kind === "text") {
      yield sse("content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: { type: "text_delta", text: ev.text },
      });
    } else if (ev.kind === "text_block_boundary") {
      yield sse("content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: { type: "text_delta", text: "\n\n" },
      });
    } else if (ev.kind === "done") {
      outputTokens = ev.usage.output_tokens || 0;
    } else if (ev.kind === "error") {
      errored = true;
      yield sse("error", {
        type: "error",
        error: {
          type: ev.rateLimited ? "rate_limit_error" : "api_error",
          message: ev.message,
        },
      });
    }
  }

  yield sse("content_block_stop", { type: "content_block_stop", index: 0 });
  yield sse("message_delta", {
    type: "message_delta",
    delta: { stop_reason: errored ? "end_turn" : "end_turn", stop_sequence: null },
    usage: { output_tokens: outputTokens },
  });
  yield sse("message_stop", { type: "message_stop" });
}

function sse(event: string, data: unknown): string {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
}
