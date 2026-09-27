// Rebuild an Anthropic Messages request as a ChatJimmy conversation that fits its window.
//
// ChatJimmy holds ~6.1k tokens *including* the reply, so a request is rewritten rather than
// forwarded: Claude Code's system blocks and <system-reminder>s are reduced to the facts that
// matter (working directory, platform, date), tools become one-line summaries (tools.js), past
// tool calls are replayed in the same JSON the model is asked to write, and tool output is
// trimmed — hardest for the oldest — until the whole prompt fits the budget.

import { toolProtocol, toolCallText, examples } from './tools.js';
import { recentCalls, latestRequest } from './policy.js';

export const SOFT_STEPS = 12;
export const HARD_STEPS = 20;

export const DEFAULT_PROMPT_BUDGET = 4000; // tokens of prompt; the rest of the ~6.1k window is left for the reply

// Llama 3 averages ~3.9 characters per token on prose and less on code/JSON; 3.2 keeps estimates on the safe side.
export const estimateTokens = (s) => Math.ceil(String(s ?? '').length / 3.2);
const messageTokens = (m) => estimateTokens(m.content) + 6;

const REMINDER_RE = /<system-reminder>[\s\S]*?<\/system-reminder>/g;

// ---------------------------------------------------------------- environment

/** The facts Claude Code spreads over its system prompt, system messages and reminders. */
export function extractEnvironment(texts) {
  const all = texts.filter(Boolean).join('\n');
  const grab = (re) => all.match(re)?.[1]?.trim();
  return {
    cwd: grab(/Primary working directory:\s*([^\n]+)/) || grab(/Working directory:\s*([^\n]+)/),
    platform: grab(/Platform:\s*([^\n]+)/),
    shell: grab(/Shell:\s*([^\n]+)/),
    os: grab(/OS Version:\s*([^\n]+)/),
    date: grab(/Today's date is\s*([^\n.]+(?:\.\d+)?)/),
    git: grab(/Is a git repository:\s*([^\n]+)/)
  };
}

const systemTexts = (system) =>
  typeof system === 'string' ? [system] : Array.isArray(system) ? system.map((b) => b?.text || '') : [];

// bro launches Claude Code with this as its whole system prompt (--system-prompt); the endpoint
// writes the real one, so it is recognised and dropped here.
export const CLAUDE_SYSTEM_PROMPT = 'You are a coding assistant. (bro ChatJimmy mode: the endpoint supplies the working prompt.)';

// Claude Code's own identity/billing blocks carry nothing ChatJimmy can use.
const isBoilerplate = (t) =>
  t === CLAUDE_SYSTEM_PROMPT ||
  /^x-anthropic-billing-header:/.test(t) ||
  /^You are (a Claude agent|Claude Code|an interactive CLI tool|an agent for Claude Code)/.test(t);

// ---------------------------------------------------------------- content normalisation

function blocks(content) {
  if (typeof content === 'string') return [{ type: 'text', text: content }];
  return Array.isArray(content) ? content : [];
}

const resultText = (content) =>
  typeof content === 'string'
    ? content
    : blocks(content).map((b) => (b.type === 'text' ? b.text : b.type === 'image' ? '[image omitted]' : '')).join('\n');

function describeCall(name, input = {}) {
  const v = input.file_path || input.path || input.pattern || input.command || input.url || '';
  const s = String(v).replace(/\s+/g, ' ');
  return s ? ` ${s.length > 80 ? s.slice(0, 77) + '…' : s}` : '';
}

/**
 * Anthropic messages → [{ role, content, kind }] for ChatJimmy. kind 'result' marks a tool result
 * (trimmed first), 'task' the first real user message (kept).
 */
export function convertMessages(messages, { cwd } = {}) {
  const calls = new Map(); // tool_use id → { name, input }
  // Replayed calls show paths the way the model should write them: relative to the working directory.
  const shownInput = (input) => Object.fromEntries(Object.entries(input || {}).map(([k, v]) => [k, typeof v === 'string' ? relativisePaths(v, cwd) || '.' : v]));
  const out = [];
  const systemNotes = [];
  const push = (role, content, kind = 'text') => {
    if (!content.trim()) return;
    const last = out[out.length - 1];
    if (last && last.role === role && last.kind !== 'result' && kind !== 'result') last.content += '\n\n' + content;
    else out.push({ role, content, kind });
  };

  for (const m of messages || []) {
    if (m.role === 'system') { systemNotes.push(resultText(m.content)); continue; }
    if (m.role === 'assistant') {
      const parts = [];
      for (const b of blocks(m.content)) {
        if (b.type === 'text' && b.text) parts.push(b.text.trim());
        else if (b.type === 'tool_use') {
          const shown = shownInput(b.input);
          calls.set(b.id, { name: b.name, input: shown });
          parts.push(toolCallText(b.name, shown));
        }
      }
      push('assistant', parts.filter(Boolean).join('\n'));
      continue;
    }
    // user: tool results become their own turns; text (minus reminders) follows them.
    const texts = [];
    for (const b of blocks(m.content)) {
      if (b.type === 'tool_result') {
        const call = calls.get(b.tool_use_id) || { name: 'tool' };
        let body = relativisePaths(resultText(b.content).replace(REMINDER_RE, '').trimEnd().replace(/^\n+/, ''), cwd, call.name) || '(no output)';
        if (call.name === 'Read') {
          // Read numbers the empty line after a final newline; the model then counts one line too many.
          body = body.replace(/(?:\n[ \t]*\d+(?:\t|→)?[ \t]*)+$/, '');
          // The line-number column costs tokens, and the model copies it into Write/Edit text.
          body = stripLineNumbers(body);
        }
        const head = `${b.is_error ? 'Error from' : 'Result of'} ${call.name}${describeCall(call.name, call.input)}:`;
        push('user', `${head}\n${body}`, 'result');
        out[out.length - 1].tool = call.name;
      } else if (b.type === 'text' && b.text) {
        systemNotes.push(...(b.text.match(REMINDER_RE) || []));
        texts.push(b.text.replace(REMINDER_RE, '').trim());
      } else if (b.type === 'image') texts.push('[image omitted — this model cannot see images]');
    }
    const text = texts.filter(Boolean).join('\n\n');
    if (text) push('user', text, out.some((x) => x.kind === 'task') ? 'text' : 'task');
  }
  return { turns: out, systemNotes };
}

/** Remove Read's "     12\t" / "12→" line-number column when every non-empty line has it. */
export function stripLineNumbers(text) {
  const lines = text.split('\n');
  const numbered = /^\s*\d+(?:\t|→)/;
  if (!lines.filter((l) => l.trim()).every((l) => numbered.test(l))) return text;
  return lines.map((l) => l.replace(numbered, '')).join('\n');
}

/** Identifiers a request is about: calls (getLimit()), camelCase/snake_case words, quoted text. */
export function focusTerms(task) {
  const terms = new Set();
  for (const m of task.matchAll(/\b([A-Za-z_$][\w$]*)\s*\(/g)) terms.add(m[1]);
  for (const m of task.matchAll(/\b([a-z]+[A-Z][\w$]*|[A-Za-z]+_[\w$]+)\b/g)) terms.add(m[1]);
  for (const m of task.matchAll(/["'`]([^"'`\n]{3,40})["'`]/g)) terms.add(m[1]);
  return [...terms].filter((t) => t.length >= 3 && !/^(if|for|while|return|console|import|export)$/.test(t));
}

/**
 * A long file shown whole buries the part the request is about (seen: "there is no getLimit()" with
 * getLimit on screen, 300 lines in). When it mentions a focus term, show those places with context.
 */
export function focusWindows(text, terms, { minLines = 60, context = 4 } = {}) {
  const lines = text.split('\n');
  if (lines.length < minLines || !terms.length) return text;
  const hits = lines.map((l, i) => (terms.some((t) => l.includes(t)) ? i : -1)).filter((i) => i !== -1);
  if (!hits.length || hits.length > 12) return text;
  const keep = new Set();
  for (const h of hits) for (let i = Math.max(0, h - context); i <= Math.min(lines.length - 1, h + context * 2); i++) keep.add(i);
  const out = [];
  let skipped = 0;
  lines.forEach((l, i) => {
    if (keep.has(i)) {
      if (skipped) out.push(`…[${skipped} lines not shown]…`);
      skipped = 0;
      out.push(l);
    } else skipped++;
  });
  if (skipped) out.push(`…[${skipped} lines not shown]…`);
  return `(showing the parts of this ${lines.length}-line file that mention ${terms.filter((t) => hits.some((h) => lines[h].includes(t))).join(', ')})\n${out.join('\n')}`;
}

// Tool output quotes absolute paths (Glob and Grep list them). The model copies whatever it sees,
// and absolute Windows paths then break in Bash — so paths under the working directory are shown
// relative to it, and Glob/Grep listings use forward slashes.
export function relativisePaths(text, cwd, tool) {
  if (!cwd) return text;
  let out = text;
  const variants = new Set([cwd, cwd.replace(/\\/g, '/'), cwd.replace(/\//g, '\\')]);
  for (const v of variants) {
    const esc = v.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    out = out.replace(new RegExp(esc + '[\\\\/]?', 'gi'), '');
  }
  if (tool === 'Glob' || tool === 'Grep') out = out.split('\n').map((l) => (/^[\w.\-\\/ ]+(:\d+)?/.test(l) ? l.replace(/\\/g, '/') : l)).join('\n');
  return out;
}

// ---------------------------------------------------------------- trimming

function trimTo(text, maxTokens, label = 'lines') {
  if (estimateTokens(text) <= maxTokens) return text;
  const maxChars = Math.max(80, Math.floor(maxTokens * 3.2));
  const head = text.slice(0, Math.floor(maxChars * 0.7));
  const tail = text.slice(text.length - Math.floor(maxChars * 0.25));
  const omitted = text.slice(head.length, text.length - tail.length);
  const n = label === 'lines' ? `${omitted.split('\n').length} lines` : `${omitted.length} characters`;
  return `${head}\n…[${n} omitted]…\n${tail}`;
}

/**
 * Fit system + turns into `budget` tokens. Keeps the task and the latest turns, trims tool results
 * (older ones harder), then drops the oldest middle steps.
 * @returns {{ system:string, messages:{role:string, content:string}[], tokens:number, dropped:number }}
 */
export function fitToBudget(system, turns, budget) {
  const msgs = turns.map((t) => ({ ...t }));
  const lastResult = msgs.map((m) => m.kind).lastIndexOf('result');
  msgs.forEach((m, i) => {
    if (m.kind === 'result') {
      const age = msgs.slice(i + 1).filter((x) => x.kind === 'result').length; // results since this one
      m.content = trimTo(m.content, i === lastResult ? Math.floor(budget * 0.45) : age <= 2 ? 300 : 90);
    } else if (m.kind === 'task') m.content = trimTo(m.content, Math.floor(budget * 0.3));
    else m.content = trimTo(m.content, Math.floor(budget * 0.2));
  });

  const total = () => estimateTokens(system) + msgs.reduce((a, m) => a + messageTokens(m), 0);
  let dropped = 0;
  // Drop the oldest steps after the task, but always keep the last two turns.
  const taskIdx = Math.max(0, msgs.findIndex((m) => m.kind === 'task'));
  while (total() > budget && msgs.length > taskIdx + 3) { msgs.splice(taskIdx + 1, 1); dropped++; }
  // Still too big: squeeze the latest result, then the task itself.
  for (let cap = Math.floor(budget * 0.3); total() > budget && cap >= 120; cap = Math.floor(cap / 2)) {
    if (lastResult >= 0) {
      const m = msgs.filter((x) => x.kind === 'result').at(-1);
      if (m) m.content = trimTo(m.content, cap);
    }
    const t = msgs.find((x) => x.kind === 'task');
    if (t && total() > budget) t.content = trimTo(t.content, cap);
  }

  if (dropped) {
    const note = `[${dropped} earlier step${dropped === 1 ? '' : 's'} omitted to fit the context window.]`;
    const t = msgs[taskIdx];
    if (t) t.content += `\n\n${note}`;
  }
  // ChatJimmy wants user/assistant alternation starting and ending with the user.
  const merged = [];
  for (const m of msgs) {
    const last = merged[merged.length - 1];
    if (last && last.role === m.role) last.content += '\n\n' + m.content;
    else merged.push({ role: m.role, content: m.content });
  }
  if (merged[0]?.role !== 'user') merged.unshift({ role: 'user', content: '(continue)' });
  if (merged.at(-1)?.role !== 'user') merged.push({ role: 'user', content: 'Continue.' });
  return { system, messages: merged, tokens: estimateTokens(system) + merged.reduce((a, m) => a + messageTokens(m), 0), dropped };
}

// ---------------------------------------------------------------- the two prompt shapes

/**
 * Build the ChatJimmy request for an Anthropic Messages request.
 * With tools: an agent prompt with the tool protocol. Without: the caller's own system prompt
 * (side requests such as titles, summaries, or bash-prefix checks), trimmed.
 */
export function buildPrompt(body, { budget = DEFAULT_PROMPT_BUDGET } = {}) {
  const sys = systemTexts(body.system);
  // The environment (cwd above all) is needed while converting, to show paths relative to it.
  const env = extractEnvironment([...sys, ...(body.messages || []).flatMap((m) =>
    m.role === 'system' ? [resultText(m.content)] : blocks(m.content).filter((b) => b.type === 'text').flatMap((b) => b.text.match(REMINDER_RE) || []))]);
  const { turns } = convertMessages(body.messages, { cwd: env.cwd });
  const focus = focusTerms(latestRequest(body.messages));
  for (const t of turns) if (t.kind === 'result' && t.tool === 'Read') t.content = focusWindows(t.content, focus);
  const tools = (body.tools || []).filter((t) => t && t.name && !t.type?.startsWith?.('web_search')); // server tools can't be emulated
  const toolChoice = body.tool_choice?.type;
  // Step budget: an 8B model can wander (seen: 100+ calls on a one-grep task). After SOFT_STEPS it is
  // told to wrap up; after HARD_STEPS the tools are withdrawn so the next reply is an answer.
  const steps = tools.length ? recentCalls(body.messages).length : 0;
  const exhausted = steps >= HARD_STEPS;
  const useTools = tools.length > 0 && toolChoice !== 'none' && !exhausted;

  const extra = sys.filter((t) => t.trim() && !isBoilerplate(t.trim())).join('\n\n').trim();
  let system;
  if (exhausted) {
    system = [
      `You are a coding agent in the user's terminal${env.cwd ? `, working in ${env.cwd}` : ''}.`,
      `You have used ${steps} tool calls on this request, which is the limit. Tools are no longer available.`,
      'Answer the user now in plain text: say what you found or did, and what is still unfinished.'
    ].join('\n');
  } else if (useTools) {
    const bash = tools.some((t) => t.name === 'Bash');
    const where = [env.cwd && `working in ${env.cwd}`, env.platform && `on ${env.platform}${bash ? ' (Bash runs in bash)' : ''}`].filter(Boolean).join(' ');
    system = [
      `You are a coding agent in the user's terminal${where ? `, ${where}` : ''}.${env.date ? ` Today is ${env.date}.` : ''}`,
      'Complete the user\'s request. Use tools to look at and change files and to run commands.',
      '',
      toolProtocol(tools),
      '',
      'Rules:',
      '- One tool call per reply. You will get its result, then make the next call or answer.',
      '- Read files with Read, find them with Glob or Grep, change them with Edit (part) or Write (whole file). Read a file before editing it.',
      '- Look things up with tools instead of guessing. Use paths relative to the working directory.',
      '- If a call fails, do not repeat it: fix the arguments or use a different tool.',
      '- Tools are for this computer\'s files and programs. Answer general knowledge yourself, without a tool. To calculate, run node -e "console.log(17 * 3)" with Bash rather than working it out in your head. Your text reply is shown to the user as it is — never use a tool to print or show an answer.',
      '- Only change files when the user asks for a change. A question gets an answer, not an edit.',
      '- Once the request is done, reply to the user in plain text (no JSON), briefly stating what you found or did.',
      '',
      examples(tools),
      toolChoice === 'any' || toolChoice === 'tool' ? `- You must call ${body.tool_choice?.name ? `the ${body.tool_choice.name} tool` : 'a tool'} now.` : '',
      extra && estimateTokens(extra) < 400 ? `\nAdditional instructions:\n${extra}` : ''
    ].filter((l) => l !== '').join('\n');
  } else {
    system = trimTo(extra || 'You are a helpful assistant.', Math.floor(budget * 0.35), 'chars');
  }

  const fitted = fitToBudget(system, turns, budget);
  if (useTools && steps >= SOFT_STEPS) {
    const last = fitted.messages.at(-1);
    last.content += `\n\n[You have made ${steps} tool calls for this request. Answer the user now with what you have found; call a tool only if it is essential.]`;
  }
  return { ...fitted, env, steps, tools: useTools ? tools : [] };
}
