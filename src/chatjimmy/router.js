// Jev (TypeSafe's calibrated decision model) as ChatJimmy's step router — the llama3.1-8B+jev model.
//
// An 8B model writes tool arguments well enough but often picks the wrong kind of step: Glob when
// the question is about file *contents*, Bash to "print" an answer, an edit when the user only asked.
// Choosing the next step is a typed decision, which is what Jev is built for. Each turn Jev is asked
// — in parallel with ChatJimmy, so agreement costs no time — which step comes next; when ChatJimmy
// chose differently and Jev is confident, ChatJimmy is told which tool to use and asked again.
// ChatJimmy still writes every argument and every word of the answer.
//
// Keys: a TypeSafe key (bro's Jev Router key, JEV_API_KEY / TYPESAFE_API_KEY) calls TypeSafe
// directly; otherwise an OpenRouter key calls Jev through OpenRouter's decisions API.

const ENDPOINTS = {
  typesafe: { url: 'https://api.typesafe.ai/v1/systemone', model: 'jev-latest' },
  openrouter: { url: 'https://openrouter.ai/api/alpha/decisions', model: '~typesafe/jev-latest' }
};

// What each step means, phrased for the decision.
const STEP_CRITERIA = {
  answer: 'Reply to the user now: the request is done, or it needs no files, commands or calculation (general knowledge, explanations)',
  Read: 'Read a file whose path is known, to see its contents',
  Glob: 'Find files by their name or path pattern (e.g. list the files in a folder)',
  Grep: 'Search inside files for text (e.g. which files use, import or mention something)',
  Bash: 'Run a program or command: node, npm, git, tests, scripts — and any arithmetic or calculation, however simple',
  PowerShell: 'Run a PowerShell command',
  Write: 'Create a new file (or replace a whole file)',
  Edit: 'Change part of an existing file that has already been read'
};

const clip = (s, n) => {
  const t = String(s ?? '').replace(/\s+/g, ' ').trim();
  return t.length > n ? t.slice(0, n - 1) + '…' : t;
};

/** The recent steps of the task, compactly, for Jev's state. */
export function stepsSummary(messages, limit = 4) {
  const calls = new Map();
  const steps = [];
  for (const m of messages || []) {
    const content = typeof m.content === 'string' ? [] : m.content || [];
    for (const b of content) {
      if (m.role === 'assistant' && b.type === 'tool_use') {
        calls.set(b.id, b);
        steps.push({ tool: b.name, target: clip(b.input?.file_path || b.input?.pattern || b.input?.command || '', 120), result: null });
      } else if (m.role === 'user' && b.type === 'tool_result') {
        const step = steps.findLast((s) => s.result === null && calls.get(b.tool_use_id)?.name === s.tool);
        const text = typeof b.content === 'string' ? b.content : (b.content || []).map((x) => x.text || '').join('\n');
        if (step) step.result = `${b.is_error ? 'ERROR: ' : ''}${clip(text, 240)}`;
      }
    }
  }
  return steps.slice(-limit);
}

/**
 * @param {object} o
 * @param {{kind:'typesafe'|'openrouter', key:string}} o.credentials
 * @returns {(args:{task:string, cwd?:string, steps:object[], tools:string[]}) => Promise<{choice:string, p:number, probabilities:object, latencyMs:number}|null>}
 */
export function createJevRouter({ credentials, fetchImpl = fetch, timeoutMs = 8000 }) {
  const ep = ENDPOINTS[credentials.kind];
  return async function route({ task, cwd, steps, tools }) {
    const options = ['answer', ...tools.filter((t) => STEP_CRITERIA[t])];
    const t0 = Date.now();
    try {
      const res = await fetchImpl(ep.url, {
        method: 'POST',
        headers: { authorization: `Bearer ${credentials.key}`, 'content-type': 'application/json' },
        body: JSON.stringify({
          model: ep.model,
          state: { request: task, working_directory: cwd || '(unknown)', steps_so_far: steps.length ? steps : 'none yet' },
          questions: {
            next: {
              type: 'choice',
              // Jev judges steps for a capable agent unless told otherwise: without the note it sent
              // "What is 17 * 3?" to a direct answer (0.98), which the 8B model then got wrong.
              instructions: "A small coding agent (an 8B model that often gets arithmetic wrong, so every calculation must be run with Bash) is working on the user's request, using the steps so far. What should it do next?",
              criteria: Object.fromEntries(options.map((o) => [o, STEP_CRITERIA[o]]))
            }
          }
        }),
        signal: AbortSignal.timeout(timeoutMs)
      });
      if (!res.ok) return null;
      const json = await res.json();
      const a = json.answers?.next;
      if (!a?.choice) return null;
      return { choice: a.choice, p: a.probabilities?.[a.choice] ?? a.confidence ?? 0, probabilities: a.probabilities || {}, latencyMs: Date.now() - t0, model: json.model };
    } catch {
      return null; // routing is an improvement, never a dependency
    }
  };
}

/** Find credentials for Jev: a TypeSafe key first, then OpenRouter. */
export function jevCredentials({ typesafeKey, openrouterKey }) {
  if (typesafeKey) return { kind: 'typesafe', key: typesafeKey };
  if (openrouterKey) return { kind: 'openrouter', key: openrouterKey };
  return null;
}

/** The directive that steers ChatJimmy to Jev's choice. */
export function directiveFor(choice) {
  if (choice === 'answer') return 'Do not call a tool for this step. Answer the user now in plain text.';
  const hints = {
    Grep: ' Use Grep to search inside the files (for example the text or import you are looking for).',
    Glob: ' Use Glob with a name pattern.',
    Bash: ' Use Bash to run the command.',
    Read: ' Use Read on the file.',
    Edit: ' Use Edit on the file you read.',
    Write: ' Use Write to create the file.'
  };
  return `For this step, use the ${choice} tool.${hints[choice] || ''} Reply with only the JSON call.`;
}
