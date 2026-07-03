/**
 * Types for the Claude Code CLI's `--output-format stream-json` output.
 *
 * The CLI emits newline-delimited JSON. Each line is one of the shapes below.
 * Notably, when run with `--include-partial-messages`, it wraps native
 * Anthropic streaming events inside a `stream_event` envelope — which is why we
 * can serve an Anthropic-compatible endpoint almost for free.
 */

export interface CliUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

export interface CliInit {
  type: "system";
  subtype: "init";
  session_id: string;
  model: string;
  [key: string]: unknown;
}

export interface CliTextContent {
  type: "text";
  text: string;
}

export interface CliToolUseContent {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export type CliAssistantContent = CliTextContent | CliToolUseContent;

export interface CliAssistant {
  type: "assistant";
  message: {
    model: string;
    id: string;
    role: "assistant";
    content: CliAssistantContent[];
    stop_reason: string | null;
    usage: CliUsage;
  };
  session_id: string;
}

export interface CliResult {
  type: "result";
  subtype: "success" | "error" | string;
  is_error: boolean;
  duration_ms: number;
  num_turns: number;
  result: string;
  session_id: string;
  total_cost_usd?: number;
  usage: CliUsage;
}

export interface CliStreamEvent {
  type: "stream_event";
  event: {
    type:
      | "message_start"
      | "content_block_start"
      | "content_block_delta"
      | "content_block_stop"
      | "message_delta"
      | "message_stop";
    index?: number;
    delta?:
      | { type: "text_delta"; text: string }
      | { type: "input_json_delta"; partial_json: string }
      | { type: "thinking_delta"; thinking: string }
      | { type: string; [key: string]: unknown };
    content_block?:
      | { type: "text"; text: string }
      | { type: "tool_use"; id: string; name: string }
      | { type: string; [key: string]: unknown };
    usage?: Partial<CliUsage>;
  };
  session_id: string;
}

export interface CliSystemMessage {
  type: "system";
  subtype: string;
  [key: string]: unknown;
}

export type CliMessage =
  | CliInit
  | CliAssistant
  | CliResult
  | CliStreamEvent
  | CliSystemMessage;

export function isAssistant(m: CliMessage): m is CliAssistant {
  return m.type === "assistant";
}
export function isResult(m: CliMessage): m is CliResult {
  return m.type === "result";
}
export function isStreamEvent(m: CliMessage): m is CliStreamEvent {
  return m.type === "stream_event";
}
